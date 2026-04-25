use std::{collections::HashMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        IntoResponse, Sse,
        sse::{Event, KeepAlive},
    },
    routing::get,
};
use futures::Stream;
use tokio_stream::{
    StreamExt,
    wrappers::{BroadcastStream, errors::BroadcastStreamRecvError},
};
use tower_http::trace::TraceLayer;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{
    app::{app_state::AppState, models::BookQuery},
    common::error::{AppError, AppResult},
    deribit::{
        channel_name::ChannelName,
        models::{Instrument, OrderBook, OrderBookUpdate, Ticker},
    },
    order_book::{book::Book, book_manager::BookManager},
};

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/instruments", get(get_instruments))
        .route("/ticker/{instrument_name}", get(get_ticker))
        .route("/ticker/{instrument_name}/stream", get(stream_ticker))
        .route("/book/{instrument_name}", get(get_book))
        .route("/book/{instrument_name}/stream", get(stream_book))
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
    let ticker = state.deribit_client.get_ticker(&instrument_name).await?;
    Ok(Json(ticker))
}

async fn stream_ticker(
    State(state): State<Arc<AppState>>,
    Path(instrument_name): Path<String>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, AppError>>>> {
    info!(instrument_name, "Streaming ticker");

    let stream = state
        .deribit_client
        .subscribe_ticker(&ChannelName::ticker(&instrument_name), Uuid::new_v4())
        .await?
        .map(|result| {
            result.and_then(|item| {
                Event::default().json_data(item).map_err(|e| {
                    AppError::InternalError(format!(
                        "Error on ticker subscription {}",
                        e.to_string()
                    ))
                })
            })
        });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn get_book(
    State(state): State<Arc<AppState>>,
    Path(instrument_name): Path<String>,
    Query(query): Query<BookQuery>,
) -> AppResult<Json<Book>> {
    info!(instrument_name, "Fetching order book");
    let channel = ChannelName::book(&instrument_name);

    let manager = BookManager::new(Arc::clone(&state.deribit_client), channel).await?;
    let book = manager.get_book().read().await.clone();
    debug!("Waiting 5 sec to update order book");
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    debug!("Returning order book");
    Ok(Json(book))
}

async fn stream_book(
    State(state): State<Arc<AppState>>,
    Path(instrument_name): Path<String>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, AppError>>>> {
    info!(instrument_name, "Streaming order book");

    let stream = state
        .deribit_client
        .subscribe_order_book(&ChannelName::book(&instrument_name), Uuid::new_v4())
        .await?
        .map(|result| {
            result.and_then(|item| {
                Event::default().json_data(item).map_err(|e| {
                    AppError::InternalError(format!(
                        "Error on order book subscription {}",
                        e.to_string()
                    ))
                })
            })
        });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn get_instruments(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> AppResult<Json<Vec<Instrument>>> {
    let currency = params.get("currency").map(|s| s.as_str()).unwrap_or("BTC");
    let kind = params.get("kind").map(|s| s.as_str());

    info!(currency, ?kind, "Fetching instruments");
    let instruments = state.deribit_client.get_instruments(currency, kind).await?;
    Ok(Json(instruments))
}

async fn fallback() -> impl IntoResponse {
    (StatusCode::NOT_FOUND, "Not Found")
}
