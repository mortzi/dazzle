use std::sync::Arc;

use axum::Router;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    app::{app_state::AppState, router::create_router},
    common::{
        config::Config,
        error::{AppError, AppResult},
    },
    deribit::{client::DeribitClient, connection::ConnectionManager},
};

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
    let connection_manager = ConnectionManager::connect(deribit_url).await?;
    let deribit_client = Arc::new(DeribitClient::new(connection_manager));

    let app_state = Arc::new(AppState::new(config, deribit_client));

    let app = create_router(Arc::clone(&app_state));

    Ok((app, app_state))
}
