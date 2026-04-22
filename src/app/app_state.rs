use crate::common::config::Config;
use crate::deribit::service::DeribitService;
use std::sync::Arc;

pub struct AppState {
    pub config: Config,
    pub deribit_service: Arc<DeribitService>,
}

impl AppState {
    pub fn new(config: Config, deribit_service: Arc<DeribitService>) -> Self {
        Self {
            config,
            deribit_service,
        }
    }
}
