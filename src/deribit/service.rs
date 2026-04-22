use crate::common::error::{AppError, AppResult};
use crate::deribit::connection::WsManager;
use crate::deribit::models::{Instrument, Request, Response, Ticker};
use crate::deribit::ticker_stream::TickerStream;
use dashmap::DashMap;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::string::ToString;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Utf8Bytes;
use tracing::{debug, warn};
use uuid::Uuid;

pub struct DeribitService {
    sender: mpsc::Sender<Utf8Bytes>,
    request_id: Arc<AtomicU64>,
    pending_requests: Arc<DashMap<u64, oneshot::Sender<Utf8Bytes>>>,
    subscriptions: DashMap<Uuid, String>,
    subscribed_instruments: DashMap<String, AtomicU32>,
    tickers_tx: broadcast::Sender<Ticker>,
    _dispatch_loop_task: JoinHandle<()>,
}

impl DeribitService {
    pub fn new(ws: WsManager) -> Self {
        let (tickers_tx, _) = broadcast::channel::<Ticker>(128);
        let pending_requests = Arc::new(DashMap::new());
        let request_id = Arc::new(AtomicU64::new(1));
        let task = tokio::spawn(Self::dispatch_loop(
            ws.receiver,
            ws.sender.clone(),
            Arc::clone(&request_id),
            Arc::clone(&pending_requests),
            tickers_tx.clone(),
        ));
        Self {
            sender: ws.sender,
            request_id,
            tickers_tx,
            pending_requests,
            _dispatch_loop_task: task,
            subscriptions: DashMap::new(),
            subscribed_instruments: DashMap::new(),
        }
    }

    async fn dispatch_loop(
        mut receiver: mpsc::Receiver<Utf8Bytes>,
        sender: mpsc::Sender<Utf8Bytes>,
        request_id: Arc<AtomicU64>,
        pending_requests: Arc<DashMap<u64, oneshot::Sender<Utf8Bytes>>>,
        tickers_sender: broadcast::Sender<Ticker>,
    ) {
        while let Some(msg) = receiver.recv().await {
            // Try to parse as a response with an id first
            let Ok(raw) = serde_json::from_slice::<serde_json::Value>(msg.as_ref()) else {
                warn!("Failed to parse WS message: {}", msg);
                continue;
            };

            // It's a response to a pending request
            if let Some(id) = raw.get("id").and_then(|v| v.as_u64()) {
                if let Some((_, tx)) = pending_requests.remove(&id) {
                    let _ = tx.send(msg);
                }
                continue;
            }

            // It's a subscription push (no id)
            let Some(method) = raw.get("method").and_then(|v| v.as_str()) else {
                warn!("Unhandled WS message: {}", raw);
                continue;
            };

            match method {
                "subscription" => {
                    let channel = raw
                        .pointer("/params/channel")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    if channel.starts_with("ticker.") {
                        match serde_json::from_value::<Ticker>(
                            raw.pointer("/params/data").cloned().unwrap_or_default(),
                        ) {
                            Ok(ticker) => {
                                let _ = tickers_sender.send(ticker);
                            }
                            Err(e) => warn!("Failed to parse ticker: {}", e),
                        }
                    }
                }
                "heartbeat" => match raw.pointer("/params/type").and_then(|v| v.as_str()) {
                    Some("test_request") => {
                        let id = request_id.fetch_add(1, Ordering::Relaxed);
                        let pong = Request::new(id, "public/test", json!({}))
                            .to_utf8bytes()
                            .unwrap();
                        if let Err(e) = sender.send(pong).await {
                            warn!("Failed to send heartbeat response: {}", e);
                        }
                    }
                    Some("heartbeat") => debug!("Heartbeat received"),
                    _ => {}
                },
                _ => warn!("Unhandled method: {}", method),
            }
        }

        warn!("Dispatch loop exiting — WS receiver closed");
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: &'static str,
        params: serde_json::Value,
    ) -> AppResult<Response<T>> {
        let id = self.next_id();

        let msg = Request::new(id, method, params).to_utf8bytes()?;

        let (tx, rx) = oneshot::channel();
        self.pending_requests.insert(id, tx);
        let _cleanup = scopeguard::guard(id, |id| {
            self.pending_requests.remove(&id);
        });

        self.sender
            .send(msg)
            .await
            .map_err(|e| AppError::InternalError(format!("WS channel closed: {}", e)))?;

        let response_bytes = tokio::time::timeout(Duration::from_secs(10), rx)
            .await
            .map_err(|_| AppError::InternalError(format!("Request timed out, id: {}", id)))?
            .map_err(|_| AppError::InternalError(format!("Sender dropped, id: {}", id)))?;

        serde_json::from_slice(response_bytes.as_ref()).map_err(|e| AppError::SerializationError(e))
    }

    fn next_id(&self) -> u64 {
        self.request_id.fetch_add(1, Ordering::Relaxed)
    }

    pub async fn subscribe_instrument(
        self: &Arc<Self>,
        instrument_name: &str,
    ) -> AppResult<TickerStream> {
        let connection_id = Uuid::new_v4();

        let params = json!({
            "channels": [get_channel_name(instrument_name)]
        });
        let instrument_name = instrument_name.to_string();

        let previous_count = {
            let entry = self
                .subscribed_instruments
                .entry(instrument_name.clone())
                .or_insert(AtomicU32::new(0));
            entry.fetch_add(1, Ordering::Relaxed)
        };

        if previous_count == 0 {
            if let Err(e) = self
                .request::<serde_json::Value>("public/subscribe", params)
                .await
            {
                warn!("Failed to subscribe an instrument: {}", e);
                self.subscribed_instruments
                    .get(&instrument_name)
                    .map(|c| c.fetch_sub(1, Ordering::Relaxed));

                return Err(e);
            }
        }

        self.subscriptions
            .insert(connection_id, instrument_name.clone());

        let ticker_stream = TickerStream::new(
            self.tickers_tx.subscribe(),
            instrument_name,
            Arc::clone(self),
            connection_id,
        );

        Ok(ticker_stream)
    }

    pub async fn unsubscribe_instrument(
        self: &Arc<Self>,
        instrument_name: &str,
        connection_id: Uuid,
    ) -> AppResult<()> {
        let instrument_name = instrument_name.to_string();
        self.subscriptions.remove(&connection_id);

        let should_unsubscribe = {
            self.subscribed_instruments
                .get(&instrument_name)
                .map(|c| c.fetch_sub(1, Ordering::Relaxed) == 1)
                .unwrap_or(false)
        };

        if should_unsubscribe {
            self.subscribed_instruments.remove(&instrument_name);

            let params = json!({
                "channels": [get_channel_name(instrument_name.as_str())],
            });
            self.request::<serde_json::Value>("public/unsubscribe", params)
                .await?;
        }

        Ok(())
    }

    pub async fn get_ticker(&self, instrument_name: &str) -> AppResult<Ticker> {
        let params = json!({ "instrument_name": instrument_name });
        let ticker = self
            .request::<Ticker>("public/ticker", params)
            .await?
            .result;

        Ok(ticker)
    }

    pub async fn get_instruments(
        &self,
        currency: &str,
        kind: Option<&str>,
    ) -> AppResult<Vec<Instrument>> {
        let mut params = json!({ "currency": currency });
        if let Some(kind) = kind {
            params["kind"] = json!(kind);
        }
        let instruments = self
            .request::<Vec<Instrument>>("public/get_instruments", params)
            .await?
            .result;

        Ok(instruments)
    }
}

fn get_channel_name(instrument_name: &str) -> String {
    format!("ticker.{}.100ms", instrument_name)
}
