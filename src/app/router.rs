use std::{collections::HashMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        IntoResponse, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use futures::Stream;
use tokio_stream::StreamExt;
use tower_http::trace::TraceLayer;
use tracing::{debug, info};
use uuid::Uuid;

use crate::{
    app::{app_state::AppState, models::BookQuery},
    common::error::{AppError, AppResult},
    deribit::{
        channel::Channel,
        models::{Instrument, Ticker},
    },
    order_book::{book::Book, book_manager::BookManager},
};

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/instruments", get(get_instruments))
        .route("/ticker/{instrument_name}", get(get_ticker))
        .route("/ticker/{instrument_name}/stream", get(stream_ticker))
        .route("/start-book/{instrument_name}", post(start_book))
        .route("/stop-book/{instrument_name}", post(stop_book))
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
        .subscribe_ticker(&Channel::ticker(&instrument_name), Uuid::new_v4())
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

async fn start_book(
    State(state): State<Arc<AppState>>,
    Path(instrument_name): Path<String>,
) -> AppResult<Json<()>> {
    let channel = Channel::book(&instrument_name);
    info!(%channel, "Starting order book");
    if state.order_book_managers.contains_key(&channel) {
        return Ok(Json(()));
    }
    let book_manager =
        Arc::new(BookManager::new(Arc::clone(&state.deribit_client), channel.clone()).await?);

    state.order_book_managers.insert(channel, book_manager);

    Ok(Json(()))
}

async fn stop_book(
    State(state): State<Arc<AppState>>,
    Path(instrument_name): Path<String>,
) -> AppResult<Json<()>> {
    let channel = Channel::book(&instrument_name);
    info!(%channel, "Stopping order book");
    if let Some(_) = state.order_book_managers.remove(&channel) {
        info!(%channel, "Order book stopped");
    }
    Ok(Json(()))
}

async fn get_book(
    State(state): State<Arc<AppState>>,
    Path(instrument_name): Path<String>,
    Query(query): Query<BookQuery>,
) -> AppResult<Json<Book>> {
    let channel = Channel::book(&instrument_name);
    info!(%channel, "Fetching order book");

    let manager = BookManager::new(Arc::clone(&state.deribit_client), channel).await?;
    let book = manager.get_book().await;
    debug!("Waiting 5 sec to update order book");
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    debug!("Returning order book");
    Ok(Json(book))
}

async fn stream_book(
    State(state): State<Arc<AppState>>,
    Path(instrument_name): Path<String>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, AppError>>>> {
    let channel = Channel::book(&instrument_name);
    let connection_id = Uuid::new_v4();
    info!(%channel, %connection_id, "Streaming order book");

    let book_manager = match state.order_book_managers.get(&channel) {
        Some(manager) => Arc::clone(&*manager),
        None => {
            let manager = Arc::new(
                BookManager::new(Arc::clone(&state.deribit_client), channel.clone()).await?,
            );
            state
                .order_book_managers
                .insert(channel, Arc::clone(&manager));
            manager
        }
    };

    let stream = book_manager
        .subscribe_book(connection_id)?
        .map(move |result| {
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
