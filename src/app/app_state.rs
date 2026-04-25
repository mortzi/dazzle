use std::sync::Arc;

use crate::{
    common::config::Config, deribit::client::DeribitClient,
};

pub struct AppState {
    pub config: Config,
    pub deribit_client: Arc<DeribitClient>,
}

impl AppState {
    pub fn new(config: Config, deribit_client: Arc<DeribitClient>) -> Self {
        Self {
            config,
            deribit_client,
        }
    }
}
