use crate::common::error::AppError;
use crate::deribit::models::Ticker;
use crate::deribit::service::DeribitService;
use axum::response::sse::Event;
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tracing::{error, info, warn};
use uuid::Uuid;

pub struct TickerStream {
    pub connection_id: Uuid,
    inner: Pin<Box<dyn Stream<Item = Result<Event, AppError>> + Send>>,
    instrument: String,
    service: Arc<DeribitService>,
}

impl TickerStream {
    pub fn new(
        rx: broadcast::Receiver<Ticker>,
        instrument: String,
        service: Arc<DeribitService>,
        connection_id: Uuid,
    ) -> Self {
        let instrument_name = instrument.clone();
        let inner = BroadcastStream::new(rx)
            .filter_map(move |msg| match msg {
                Ok(ticker) if ticker.instrument_name == instrument_name => Some(Ok(ticker)),
                Ok(_) => None,
                Err(BroadcastStreamRecvError::Lagged(n)) => {
                    warn!("Ticker stream lagged, dropped {} messages", n);
                    None // skip, keep stream alive
                }
            })
            .map(|result: Result<Ticker, AppError>| {
                result.and_then(|ticker| {
                    Event::default().json_data(ticker).map_err(|e| {
                        AppError::InternalError(format!("SSE serialization failed: {}", e))
                    })
                })
            });

        Self {
            inner: Box::pin(inner),
            instrument,
            service,
            connection_id,
        }
    }
}

impl Stream for TickerStream {
    type Item = Result<Event, AppError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl Drop for TickerStream {
    fn drop(&mut self) {
        info!(connection_id = %self.connection_id, "Dropping ticker stream");
        let service = Arc::clone(&self.service);
        let instrument = self.instrument.clone();
        let connection_id = self.connection_id;
        tokio::spawn(async move {
            match service
                .unsubscribe_instrument(&instrument, connection_id)
                .await
            {
                Ok(()) => info!(connection_id = %connection_id, "Unsubscribed from ticker stream"),
                Err(e) => {
                    error!(connection_id = %connection_id, "Failed to unsubscribe from ticker stream: {}", e)
                }
            }
        });
    }
}
