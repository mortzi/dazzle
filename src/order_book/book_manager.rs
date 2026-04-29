use std::{collections::BTreeMap, sync::Arc, time::Duration};

use backoff::{ExponentialBackoffBuilder, SystemClock, backoff::Backoff};
use futures_util::StreamExt;
use tokio::{
    sync::{RwLock, broadcast},
    task::JoinHandle,
};
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

use crate::{
    common::error::AppResult,
    deribit::{
        channel::Channel,
        client::DeribitClient,
        models::{BookLevel, BookUpdateType, OrderBookUpdateMessage},
        subscription_stream::{OnDrop, SubscriptionStream},
    },
    order_book::book::{Book, LevelAmount, PriceLevel},
};

enum BookStreamReason {
    StreamEnded, // resubscribe and retry
    Shutdown,    // drop channel, stop
}

pub struct BookManager {
    channel: Channel,
    book_tx: broadcast::Sender<Book>,
    book: Arc<RwLock<Book>>,
    client: Arc<DeribitClient>,
    task: JoinHandle<()>,
}

impl BookManager {
    pub async fn new(client: Arc<DeribitClient>, channel: Channel) -> AppResult<Self> {
        let book = Arc::new(RwLock::new(Book::new()));
        let (book_tx, _) = broadcast::channel(128);
        let task = tokio::spawn(Self::maintain_book_state(
            Arc::clone(&client),
            channel.clone(),
            Arc::clone(&book),
            book_tx.clone(),
        ));
        Ok(Self {
            channel,
            book_tx,
            book,
            client,
            task,
        })
    }

    async fn maintain_book_state(
        client: Arc<DeribitClient>,
        channel: Channel,
        book: Arc<RwLock<Book>>,
        book_tx: broadcast::Sender<Book>,
    ) {
        let mut backoff = ExponentialBackoffBuilder::new()
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

            let mut stream = match client.subscribe_order_book(&channel, Uuid::new_v4()).await {
                Ok(s) => s,
                Err(e) => {
                    delay = Some(backoff.next_backoff().unwrap_or(Duration::from_secs(10)));
                    error!(%channel, "Failed to subscribe to order book: {}. Retrying in {:?}", e, delay);
                    continue;
                }
            };

            match Self::handle_book_stream(&channel, &book, &book_tx, &mut backoff, &mut stream)
                .await
            {
                BookStreamReason::StreamEnded => {
                    delay = Some(backoff.next_backoff().unwrap_or(Duration::from_secs(10)));
                    warn!(%channel, "Order book stream ended. Resubscribing in {:?}", delay);
                    continue;
                }
                BookStreamReason::Shutdown => {
                    info!(%channel, "Order book manager shutting down");
                    return;
                }
            }
        }
    }

    async fn handle_book_stream(
        channel: &Channel,
        book: &Arc<RwLock<Book>>,
        book_tx: &broadcast::Sender<Book>,
        backoff: &mut backoff::exponential::ExponentialBackoff<SystemClock>,
        stream: &mut SubscriptionStream<OrderBookUpdateMessage>,
    ) -> BookStreamReason {
        loop {
            match tokio::time::timeout(Duration::from_secs(30), stream.next()).await {
                Err(_elapsed) => {
                    warn!(%channel, "Order book stream timed out — no message in 30s");
                    return BookStreamReason::StreamEnded;
                }
                Ok(None) => {
                    // stream.next() returned None — sender dropped
                    return BookStreamReason::Shutdown;
                }
                Ok(Some(result)) => match result {
                    Ok(update_message) => match update_message {
                        OrderBookUpdateMessage::Data(update) => match update.update_type {
                            BookUpdateType::Snapshot => {
                                {
                                    let mut book = book.write().await;
                                    *book = Book::from_snapshot(update);
                                }
                                backoff.reset();
                                info!(%channel, "Order book snapshot received, streaming updates");
                            }
                            BookUpdateType::Change => {
                                let mut book = book.write().await;

                                if let Some(prev_id) = update.prev_change_id {
                                    if prev_id != book.change_id {
                                        warn!(
                                            %channel,
                                            expected = book.change_id,
                                            got = prev_id,
                                            "Order book gap detected, resubscribing"
                                        );
                                        return BookStreamReason::StreamEnded;
                                    }
                                }

                                book.change_id = update.change_id;
                                for level in &update.asks {
                                    apply_level(&mut book.asks, level);
                                }
                                for level in &update.bids {
                                    apply_level(&mut book.bids, level);
                                }
                                debug!(%channel, change_id = update.change_id, "Order book updated");

                                if book_tx.receiver_count() > 0 {
                                    if let Err(e) = book_tx.send(book.clone()) {
                                        warn!(%channel, "Failed to broadcast update: {}", e);
                                    }
                                }
                            }
                        },
                        OrderBookUpdateMessage::ConnectionLost => {
                            warn!(%channel, "Order book connection lost");
                            return BookStreamReason::StreamEnded;
                        }
                    },
                    Err(e) => {
                        error!(%channel, "Stream error: {}", e);
                        return BookStreamReason::StreamEnded;
                    }
                },
            }
        }
    }

    pub async fn get_book(&self) -> Book {
        self.book.read().await.clone()
    }

    pub fn subscribe_book(
        self: &Arc<Self>,
        connection_id: Uuid,
    ) -> AppResult<SubscriptionStream<Book>> {
        Ok(SubscriptionStream::new(
            self.book_tx.subscribe(),
            self.channel.clone(),
            connection_id,
            OnDrop::KeepAlive,
            |_| true,
        ))
    }
}

impl Drop for BookManager {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn apply_level(levels: &mut BTreeMap<PriceLevel, LevelAmount>, level: &BookLevel) {
    let price = level.price as u64;
    match level.action.as_str() {
        "new" | "change" => {
            levels.insert(price, level.size as u64);
        }
        "delete" => {
            levels.remove(&price);
        }
        _ => warn!("Unknown order book action: {}", level.action),
    }
}
