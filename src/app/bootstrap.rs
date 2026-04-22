use crate::app::app_state::AppState;
use crate::app::router::create_router;
use crate::common::config::Config;
use crate::common::error::{AppError, AppResult};
use crate::deribit::connection::WsManager;
use crate::deribit::service::DeribitService;
use axum::Router;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

pub fn setup_tracing() {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=debug".into()),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_file(true)
                .with_line_number(true)
                .with_target(true),
        )
        .init();
}

pub async fn create_app() -> AppResult<(Router, Arc<AppState>)> {
    setup_tracing();

    let config = Config::from_env().map_err(|e| AppError::InternalError(e.to_string()))?;
    let deribit_url = config.deribit_url.clone();
    let ws_manager = WsManager::connect(deribit_url).await?;

    let app_state = Arc::new(AppState::new(
        config,
        Arc::new(DeribitService::new(ws_manager)),
    ));

    let app = create_router(Arc::clone(&app_state));

    Ok((app, app_state))
}
