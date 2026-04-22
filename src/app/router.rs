use crate::app::app_state::AppState;
use crate::common::error::{AppError, AppResult};
use crate::deribit::models::{Instrument, Ticker};
use axum::extract::{Path, Query, State};
use axum::response::Sse;
use axum::response::sse::{Event, KeepAlive};
use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get};
use futures::Stream;
use std::collections::HashMap;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing::info;

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/instruments", get(get_instruments))
        .route("/ticker/{instrument_name}", get(get_ticker))
        .route("/ticker/{instrument_name}/stream", get(stream_ticker))
        .fallback(fallback)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health_check() -> &'static str {
    "OK\n"
}

async fn get_ticker(
    State(state): State<Arc<AppState>>,
    Path(instrument_name): Path<String>,
) -> AppResult<Json<Ticker>> {
    info!(instrument_name, "Fetching ticker");
    let ticker = state.deribit_service.get_ticker(&instrument_name).await?;
    Ok(Json(ticker))
}

async fn stream_ticker(
    State(state): State<Arc<AppState>>,
    Path(instrument_name): Path<String>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, AppError>>>> {
    info!(instrument_name, "Streaming ticker");

    let stream = state
        .deribit_service
        .subscribe_instrument(&instrument_name)
        .await?;

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn get_instruments(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> AppResult<Json<Vec<Instrument>>> {
    let currency = params.get("currency").map(|s| s.as_str()).unwrap_or("BTC");
    let kind = params.get("kind").map(|s| s.as_str());

    info!(currency, ?kind, "Fetching instruments");
    let instruments = state
        .deribit_service
        .get_instruments(currency, kind)
        .await?;
    Ok(Json(instruments))
}

async fn fallback() -> impl IntoResponse {
    (StatusCode::NOT_FOUND, "Not Found")
}
