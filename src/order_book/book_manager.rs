use std::{collections::BTreeMap, sync::Arc};

use futures_util::StreamExt;
use tokio::{sync::RwLock, task::JoinHandle};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::{
    common::error::{AppError, AppResult},
    deribit::{
        channel_name::ChannelName,
        client::DeribitClient,
        models::{BookLevel, OrderBookUpdate},
        subscription_stream::SubscriptionStream,
    },
    order_book::book::{Book, LevelAmount, PriceLevel},
};

pub struct BookManager {
    channel: ChannelName,
    book: Arc<RwLock<Book>>,
    task: JoinHandle<()>,
}

impl BookManager {
    pub async fn new(client: Arc<DeribitClient>, channel: ChannelName) -> AppResult<Self> {
        let mut stream = client
            .subscribe_order_book(&channel, Uuid::new_v4())
            .await?;

        let snapshot = stream.next().await.ok_or_else(|| {
            AppError::InternalError("Failed to get initial order book snapshot".to_string())
        })??;
        let book = Arc::new(RwLock::new(Book::from_snapshot(snapshot)));
        let task = tokio::spawn(Self::maintain_book_state(
            channel.clone(),
            Arc::clone(&book),
            stream,
        ));
        Ok(Self {
            channel: channel,
            book,
            task,
        })
    }

    async fn maintain_book_state(
        channel: ChannelName,
        book: Arc<RwLock<Book>>,
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

            debug!(channel_name = %channel, change_id = update.change_id, "Order book updated");
        }

        warn!("Order book maintain task exiting");
    }

    pub fn get_book(&self) -> Arc<RwLock<Book>> {
        Arc::clone(&self.book)
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
            levels.insert(price, level.amount as u64);
        }
        "delete" => {
            levels.remove(&price);
        }
        _ => warn!("Unknown order book action: {}", level.action),
    }
}
