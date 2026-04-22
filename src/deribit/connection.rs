use crate::common::error::{AppError, AppResult};
use crate::deribit::models::Request;
use backoff::backoff::Backoff;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectionState {
    Stopped,
    Connecting,
    Connected,
    Failed,
}

pub struct WsManager {
    task: JoinHandle<()>,
    state_tx: watch::Sender<ConnectionState>,

    pub state_rx: watch::Receiver<ConnectionState>,
    pub sender: mpsc::Sender<Utf8Bytes>,
    pub receiver: mpsc::Receiver<Utf8Bytes>,
}

struct WsRunner {
    url: String,
    state_tx: watch::Sender<ConnectionState>,
    inbound_tx: mpsc::Sender<Utf8Bytes>,    // WS → app
    outbound_rx: mpsc::Receiver<Utf8Bytes>, // app → WS
}

enum DisconnectReason {
    ConnectionLost, // reconnect
    Shutdown,       // app dropped outbound_tx, stop cleanly
}

impl WsManager {
    pub async fn connect(url: String) -> AppResult<Self> {
        let (outbound_tx, outbound_rx) = mpsc::channel::<Utf8Bytes>(32);
        let (inbound_tx, inbound_rx) = mpsc::channel::<Utf8Bytes>(32);
        let (state_tx, mut state_rx) = watch::channel(ConnectionState::Stopped);

        let runner = WsRunner {
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
            state_rx,
            state_tx,
            sender: outbound_tx,
            receiver: inbound_rx,
            task,
        })
    }

    pub async fn stop(self) {
        self.task.abort();
        let _ = self.state_tx.send(ConnectionState::Stopped);
    }
}

impl WsRunner {
    async fn maintain_connection(mut self) {
        let mut backoff = backoff::ExponentialBackoffBuilder::new()
            .with_initial_interval(Duration::from_millis(500))
            .with_max_interval(Duration::from_secs(30))
            .with_randomization_factor(0.3)
            .with_max_elapsed_time(None)
            .build();

        loop {
            let _ = self.state_tx.send(ConnectionState::Connecting);
            info!("WebSocket connecting to {}", &self.url);

            match connect_async(&self.url).await {
                Err(e) => {
                    let delay = backoff.next_backoff().unwrap_or(Duration::from_secs(30));
                    error!("Connection failed: {}. Retrying in {:?}", e, delay);
                    tokio::time::sleep(delay).await;
                }
                Ok((ws, _)) => {
                    let _ = self.state_tx.send(ConnectionState::Connected);
                    info!("WebSocket connected");
                    backoff.reset();

                    match self.handle_connection(ws).await {
                        DisconnectReason::ConnectionLost => {
                            let _ = self.state_tx.send(ConnectionState::Connecting);
                            let delay = backoff.next_backoff().unwrap_or(Duration::from_secs(30));
                            warn!("Connection lost. Reconnecting in {:?}", delay);
                            tokio::time::sleep(delay).await;
                        }
                        DisconnectReason::Shutdown => {
                            let _ = self.state_tx.send(ConnectionState::Stopped);
                            info!("WebSocket connection closed");
                            return;
                        }
                    }
                }
            }
        }
    }

    async fn handle_connection(
        &mut self,
        ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    ) -> DisconnectReason {
        let (mut sink, mut stream) = ws.split();

        let msg = Request::new(0, "public/set_heartbeat", json!({ "interval": 30 }))
            .to_utf8bytes()
            .unwrap();

        if sink.send(Message::Text(msg)).await.is_err() {
            return DisconnectReason::ConnectionLost;
        }

        loop {
            tokio::select! {
                msg = stream.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            info!(preview = &text[0..text.len().min(100)], "WS inbound");
                            if self.inbound_tx.send(text).await.is_err() {
                                // app dropped inbound_rx, no point continuing
                                return DisconnectReason::Shutdown;
                            }
                        }
                        Some(Ok(Message::Ping(data))) => {
                            let _ = sink.send(Message::Pong(data)).await;
                        }
                        Some(Ok(Message::Close(frame))) => {
                            if let Some(f) = frame {
                                info!("WS connection closed: {} (code: {})", f.reason, f.code);
                            } else {
                                info!("WS connection closed without reason");
                            }
                            return DisconnectReason::ConnectionLost;
                        },
                        Some(Err(e)) => {
                            error!("WS read error: {}", e);
                            return DisconnectReason::ConnectionLost;
                        }
                        None => return DisconnectReason::ConnectionLost,
                        _ => {}
                    }
                }
                msg = self.outbound_rx.recv() => {
                    match msg {
                        Some(text) => {
                            info!(preview = &text[0..text.len().min(100)], "WS outbound");
                            if let Err(e) = sink.send(Message::Text(text)).await {
                                error!("WS write error: {}", e);
                                return DisconnectReason::ConnectionLost;
                            }
                        }
                        None => return DisconnectReason::Shutdown,
                    }
                }
            }
        }
    }
}
