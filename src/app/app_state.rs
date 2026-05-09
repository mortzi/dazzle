use std::sync::Arc;

use dashmap::DashMap;

use crate::{
    common::{config::Config, error::AppResult},
    deribit::{channel::Channel, client::DeribitClient},
    order_book::book_manager::BookManager,
};

pub struct AppState {
    pub config: Config,
    pub deribit_client: Arc<DeribitClient>,
    order_book_managers: DashMap<Channel, Arc<BookManager>>,
}

impl AppState {
    pub fn new(config: Config, deribit_client: Arc<DeribitClient>) -> Self {
        Self {
            config,
            deribit_client,
            order_book_managers: DashMap::new(),
        }
    }

    pub async fn get_or_create_book_manager(
        self: &Arc<Self>,
        channel: Channel,
    ) -> AppResult<Arc<BookManager>> {
        if let Some(manager) = self.order_book_managers.get(&channel) {
            return Ok(Arc::clone(&*manager));
        }
        let manager =
            Arc::new(BookManager::new(Arc::clone(&self.deribit_client), channel.clone()).await?);
        self.order_book_managers
            .entry(channel)
            .or_insert_with(|| Arc::clone(&manager));
        Ok(manager)
    }

    pub fn remove_book_manager(&self, channel: &Channel) -> bool {
        self.order_book_managers.remove(channel).is_some()
    }
}
