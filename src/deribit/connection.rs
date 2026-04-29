use std::time::Duration;

use backoff::backoff::Backoff;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::{
    net::TcpStream,
    sync::{mpsc, watch},
    task::JoinHandle,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, Utf8Bytes},
};
use tracing::{debug, error, info, trace, warn};

use crate::{
    common::error::{AppError, AppResult},
    deribit::models::Request,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectionState {
    Stopped,
    Connecting,
    Connected,
    Failed,
}

pub struct ConnectionManager {
    task: JoinHandle<()>,
    state_tx: watch::Sender<ConnectionState>,

    pub sender: mpsc::Sender<Utf8Bytes>,
    pub receiver: mpsc::Receiver<InboundMessage>,
}

pub enum InboundMessage {
    Data(Utf8Bytes),
    ConnectionLost,
}

struct ConnectionRunner {
    url: String,
    state_tx: watch::Sender<ConnectionState>,
    inbound_tx: mpsc::Sender<InboundMessage>, // WS → app
    outbound_rx: mpsc::Receiver<Utf8Bytes>,   // app → WS
}

enum DisconnectReason {
    ConnectionLost, // reconnect
    Shutdown,       // app dropped outbound_tx, stop cleanly
}

impl ConnectionManager {
    pub async fn connect(url: String) -> AppResult<Self> {
        let (outbound_tx, outbound_rx) = mpsc::channel::<Utf8Bytes>(32);
        let (inbound_tx, inbound_rx) = mpsc::channel::<InboundMessage>(32);
        let (state_tx, mut state_rx) = watch::channel(ConnectionState::Stopped);

        let runner = ConnectionRunner {
            url,
            state_tx: state_tx.clone(),
            inbound_tx,
            outbound_rx,
        };

        let task = tokio::spawn(async move {
            runner.maintain_connection().await;
        });

        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if *state_rx.borrow() == ConnectionState::Connected {
                    return Ok::<_, AppError>(());
                }
                state_rx
                    .changed()
                    .await
                    .map_err(|_| AppError::InternalError("State channel closed".into()))?;
            }
        })
        .await
        .map_err(|_| AppError::InternalError("Timed out waiting for connection".into()))??;

        Ok(Self {
            state_tx,
            sender: outbound_tx,
            receiver: inbound_rx,
            task,
        })
    }

    pub async fn stop(self) {
        self.task.abort();
        if let Err(e) = self.state_tx.send(ConnectionState::Stopped) {
            error!("Failed to send ConnectionState {}", e);
        }
    }

    pub fn subscribe_connection_state(&self) -> watch::Receiver<ConnectionState> {
        self.state_tx.subscribe()
    }
}

impl ConnectionRunner {
    async fn maintain_connection(mut self) {
        let mut backoff = backoff::ExponentialBackoffBuilder::new()
            .with_initial_interval(Duration::from_millis(500))
            .with_max_interval(Duration::from_secs(30))
            .with_randomization_factor(0.3)
            .with_max_elapsed_time(None)
            .build();
        let mut delay = None;

        loop {
            if let Some(d) = delay {
                tokio::time::sleep(d).await;
            }
            let _ = self.state_tx.send(ConnectionState::Connecting);
            debug!("WebSocket connecting to {}", &self.url);

            match connect_async(&self.url).await {
                Err(e) => {
                    delay = Some(backoff.next_backoff().unwrap_or(Duration::from_secs(30)));
                    error!("Connection failed: {}. Retrying in {:?}", e, delay);
                }
                Ok((ws, _)) => {
                    let _ = self.state_tx.send(ConnectionState::Connected);
                    info!("WebSocket connected");
                    backoff.reset();

                    match self.handle_connection(ws).await {
                        DisconnectReason::ConnectionLost => {
                            let _ = self.state_tx.send(ConnectionState::Connecting);
                            delay = Some(backoff.next_backoff().unwrap_or(Duration::from_secs(30)));
                            warn!("Connection lost. Reconnecting in {:?}", delay);
                        }
                        DisconnectReason::Shutdown => {
                            let _ = self.state_tx.send(ConnectionState::Stopped);
                            info!("WebSocket connection closed");
                            return;
                        }
                    }
                }
            }

            if let Err(e) = self.inbound_tx.send(InboundMessage::ConnectionLost).await {
                error!("Failed to send Inbound message ConnectionLost: {}", e);
            }
        }
    }

    async fn handle_connection(
        &mut self,
        ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    ) -> DisconnectReason {
        let (mut sink, mut stream) = ws.split();

        let heartbeat_msg = Request::new(0, "public/set_heartbeat", json!({ "interval": 30 }))
            .to_utf8bytes()
            .unwrap();

        if sink.send(Message::Text(heartbeat_msg)).await.is_err() {
            return DisconnectReason::ConnectionLost;
        }

        loop {
            tokio::select! {
                result = tokio::time::timeout(Duration::from_secs(30), stream.next()) => {
                    match result {
                        Err(_elapsed) => {
                            warn!("WS read timed out — no message in 30s");
                            return DisconnectReason::ConnectionLost;
                        }
                        Ok(msg) => match msg {
                            Some(some_msg) => match some_msg {
                                Ok(ok_msg) => match ok_msg {
                                    Message::Text(text) => {
                                        trace!(preview = &text[0..text.len()], "WS inbound");
                                        if self.inbound_tx.send(InboundMessage::Data(text)).await.is_err() {
                                            return DisconnectReason::Shutdown;
                                        }
                                    }
                                    Message::Ping(data) => {
                                        sink.send(Message::Pong(data))
                                            .await
                                            .unwrap_or_else(|e| error!("WS ping error: {}", e));
                                    }
                                    Message::Close(frame) => {
                                        if let Some(f) = frame {
                                            warn!("WS connection closed: {} (code: {})", f.reason, f.code);
                                        } else {
                                            warn!("WS connection closed without reason");
                                        }
                                        return DisconnectReason::ConnectionLost;
                                    }
                                    _ => {
                                        warn!("Unexpected WS message: {:?}", ok_msg);
                                    }
                                },
                                Err(e) => {
                                    error!("WS read error: {}", e);
                                    return DisconnectReason::ConnectionLost;
                                }
                            },
                            None => {
                                debug!("WS message of None received, closing the connection");
                                return DisconnectReason::ConnectionLost;
                            }
                        }
                    }
                }
                msg = self.outbound_rx.recv() => {
                    match msg {
                        Some(text) => {
                            trace!(preview = &text[0..text.len()], "WS outbound");
                            if let Err(e) = sink.send(Message::Text(text)).await {
                                error!("WS write error: {}", e);
                                return DisconnectReason::ConnectionLost;
                            }
                        }
                        None => {
                            debug!("Outbound channel receiver closed, closing the connection");
                            return DisconnectReason::Shutdown;
                        }
                    }
                }
            }
        }
    }
}
