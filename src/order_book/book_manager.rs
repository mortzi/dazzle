use std::{
    cell::Cell,
    collections::BTreeMap,
    sync::{Arc, mpsc},
};

use futures_util::StreamExt;
use tokio::{
    sync::{RwLock, broadcast},
    task::JoinHandle,
};
use tracing::{debug, trace, warn};
use uuid::Uuid;

use crate::{
    common::error::{AppError, AppResult},
    deribit::{
        channel::Channel,
        client::DeribitClient,
        models::{BookLevel, OrderBookUpdate},
        subscription_stream::SubscriptionStream,
    },
    order_book::book::{Book, LevelAmount, PriceLevel},
};
use crate::deribit::subscription_stream::OnDrop;

pub struct BookManager {
    channel: Channel,
    book_tx: broadcast::Sender<Book>,
    book: Arc<RwLock<Book>>,
    client: Arc<DeribitClient>,
    task: JoinHandle<()>,
}

impl BookManager {
    pub async fn new(client: Arc<DeribitClient>, channel: Channel) -> AppResult<Self> {
        let mut stream = client
            .subscribe_order_book(&channel, Uuid::new_v4())
            .await?;
        let snapshot = stream.next().await.ok_or_else(|| {
            AppError::InternalError("Failed to get initial order book snapshot".to_string())
        })??;
        let book = Arc::new(RwLock::new(Book::from_snapshot(snapshot)));
        let (book_tx, _) = broadcast::channel(128);
        let task = tokio::spawn(Self::maintain_book_state(
            channel.clone(),
            Arc::clone(&book),
            book_tx.clone(),
            stream,
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
        channel: Channel,
        book: Arc<RwLock<Book>>,
        book_tx: broadcast::Sender<Book>,
        mut stream: SubscriptionStream<OrderBookUpdate>,
    ) {
        while let Some(Ok(update)) = stream.next().await {
            let mut book = book.write().await;

            if let Some(prev_id) = update.prev_change_id {
                if prev_id != book.change_id {
                    warn!(
                        expected = book.change_id,
                        got = prev_id,
                        "Order book gap detected"
                    );
                    break;
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
                debug!(%channel, "Streaming order book update");
                if let Err(e) = book_tx.send(book.clone()) {
                    warn!(%channel, "Failed to send order book update: {}", e);
                }
            } else {
                debug!(%channel, "No active receiver, skipping order book update stream");
            }
        }

        warn!("Order book maintain task exiting");
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
