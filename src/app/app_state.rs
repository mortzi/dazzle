use std::sync::Arc;
use dashmap::DashMap;
use crate::{
    common::config::Config, deribit::client::DeribitClient,
};
use crate::deribit::channel::Channel;
use crate::order_book::book_manager::BookManager;

pub struct AppState {
    pub config: Config,
    pub deribit_client: Arc<DeribitClient>,
    pub order_book_managers: DashMap<Channel, Arc<BookManager>>
}

impl AppState {
    pub fn new(config: Config, deribit_client: Arc<DeribitClient>) -> Self {
        Self {
            config,
            deribit_client,
            order_book_managers: DashMap::new(),
        }
    }
}
