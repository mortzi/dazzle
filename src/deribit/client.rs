use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::Duration,
};

use dashmap::DashMap;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use tokio::{
    sync::{broadcast, mpsc, oneshot},
    task::JoinHandle,
};
use tokio_tungstenite::tungstenite::Utf8Bytes;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::{
    common::error::{AppError, AppResult},
    deribit::{
        channel_name::ChannelName,
        connection::ConnectionManager,
        models::{Instrument, OrderBook, OrderBookUpdate, Request, Response, Ticker},
        subscription_stream::SubscriptionStream,
    },
};

pub struct DeribitClient {
    sender: mpsc::Sender<Utf8Bytes>,
    request_id: Arc<AtomicU64>,
    pending_requests: Arc<DashMap<u64, oneshot::Sender<Utf8Bytes>>>,
    subscribed_channels: DashMap<String, AtomicU32>,
    tickers_tx: broadcast::Sender<Ticker>,
    book_change_tx: broadcast::Sender<OrderBookUpdate>,
    _dispatch_loop_task: JoinHandle<()>,
}

impl DeribitClient {
    pub fn new(connection_manager: ConnectionManager) -> Self {
        let (tickers_tx, _) = broadcast::channel::<Ticker>(128);
        let (book_change_tx, _) = broadcast::channel::<OrderBookUpdate>(128);
        let pending_requests = Arc::new(DashMap::new());
        let request_id = Arc::new(AtomicU64::new(1));
        let task = tokio::spawn(Self::dispatch_loop(
            connection_manager.receiver,
            connection_manager.sender.clone(),
            Arc::clone(&request_id),
            Arc::clone(&pending_requests),
            tickers_tx.clone(),
            book_change_tx.clone(),
        ));
        Self {
            sender: connection_manager.sender,
            request_id,
            tickers_tx,
            book_change_tx,
            pending_requests,
            _dispatch_loop_task: task,
            subscribed_channels: DashMap::new(),
        }
    }

    async fn dispatch_loop(
        mut receiver: mpsc::Receiver<Utf8Bytes>,
        sender: mpsc::Sender<Utf8Bytes>,
        request_id: Arc<AtomicU64>,
        pending_requests: Arc<DashMap<u64, oneshot::Sender<Utf8Bytes>>>,
        tickers_tx: broadcast::Sender<Ticker>,
        book_change_tx: broadcast::Sender<OrderBookUpdate>,
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

                    let data_params = raw.pointer("/params/data").cloned().unwrap_or_default();

                    match channel.split('.').next() {
                        Some("ticker") => match serde_json::from_value::<Ticker>(data_params) {
                            Ok(ticker) => {
                                let _ = tickers_tx.send(ticker);
                            }
                            Err(e) => warn!("Failed to parse ticker: {}", e),
                        },
                        Some("book") => {
                            match serde_json::from_value::<OrderBookUpdate>(data_params) {
                                Ok(ticker) => {
                                    let _ = book_change_tx.send(ticker);
                                }
                                Err(e) => warn!("Failed to parse ticker: {}", e),
                            }
                        }
                        _ => {
                            warn!("Unhandled subscription channel: {}", channel);
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

    async fn subscribe(self: &Arc<Self>, channel: &ChannelName) -> AppResult<()> {
        let channel_name_string = channel.to_string();

        let channel_name_string_clone = channel_name_string.clone();
        let previous_count = {
            let entry = self
                .subscribed_channels
                .entry(channel_name_string_clone)
                .or_insert(AtomicU32::new(0));
            entry.fetch_add(1, Ordering::Relaxed)
        };

        if previous_count == 0 {
            if let Err(e) = self
                .request::<serde_json::Value>(
                    "public/subscribe",
                    json!({
                        "channels": [channel],
                    }),
                )
                .await
            {
                warn!(channel = %channel, "Failed to subscribe channel: {}", e);
                self.subscribed_channels
                    .get(&channel_name_string)
                    .map(|c| c.fetch_sub(1, Ordering::Relaxed));

                return Err(e);
            }
        }

        Ok(())
    }

    pub async fn unsubscribe(self: &Arc<Self>, channel: ChannelName) -> AppResult<()> {
        let channel_name_string = channel.to_string();
        let should_unsubscribe = {
            self.subscribed_channels
                .get(&channel_name_string)
                .map(|c| c.fetch_sub(1, Ordering::Relaxed) == 1) // decrement count
                .unwrap_or(false)
        };

        if should_unsubscribe {
            self.subscribed_channels.remove(&channel_name_string);

            self.request::<serde_json::Value>(
                "public/unsubscribe",
                json!({
                    "channels": [channel],
                }),
            )
            .await?;
        }

        Ok(())
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

    pub async fn get_ticker(&self, instrument_name: &str) -> AppResult<Ticker> {
        let params = json!({ "instrument_name": instrument_name });
        let ticker = self
            .request::<Ticker>("public/ticker", params)
            .await?
            .result;

        Ok(ticker)
    }

    pub async fn subscribe_ticker(
        self: &Arc<Self>,
        channel: &ChannelName,
        connection_id: Uuid,
    ) -> AppResult<SubscriptionStream<Ticker>> {
        let instrument_name_filter = channel.instrument_name.clone();

        let filter = move |item: &Ticker| item.instrument_name == instrument_name_filter;
        self.subscribe(channel).await?;

        let stream = SubscriptionStream::new(
            self.tickers_tx.subscribe(),
            channel.clone(),
            Arc::clone(self),
            connection_id,
            filter,
        );

        Ok(stream)
    }

    pub async fn get_book_snapshot(
        &self,
        instrument_name: &str,
        depth: u32,
    ) -> AppResult<OrderBook> {
        let order_book = self
            .request::<OrderBook>(
                "public/get_order_book",
                json!({"instrument_name": instrument_name, "depth": depth}),
            )
            .await?
            .result;

        Ok(order_book)
    }

    pub async fn subscribe_order_book(
        self: &Arc<Self>,
        channel: &ChannelName,
        connection_id: Uuid,
    ) -> AppResult<SubscriptionStream<OrderBookUpdate>> {
        let instrument_name_filter = channel.instrument_name.clone();

        let filter = move |item: &OrderBookUpdate| item.instrument_name == instrument_name_filter;
        self.subscribe(channel).await?;

        let stream = SubscriptionStream::new(
            self.book_change_tx.subscribe(),
            channel.clone(),
            Arc::clone(self),
            connection_id,
            filter,
        );

        Ok(stream)
    }
}
