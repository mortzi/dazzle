use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use futures::Stream;
use tokio::sync::broadcast;
use tokio_stream::{
    StreamExt,
    wrappers::{BroadcastStream, errors::BroadcastStreamRecvError},
};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{
    common::error::AppError,
    deribit::{channel_name::ChannelName, client::DeribitClient},
};

pub struct SubscriptionStream<T> {
    pub connection_id: Uuid,
    pub channel: ChannelName,
    inner: Pin<Box<dyn Stream<Item = Result<T, AppError>> + Send>>,
    client: Arc<DeribitClient>,
}

impl<T> SubscriptionStream<T>
where
    T: Clone + Send + 'static,
{
    pub fn new(
        rx: broadcast::Receiver<T>,
        channel: ChannelName,
        client: Arc<DeribitClient>,
        connection_id: Uuid,
        filter: impl Fn(&T) -> bool + Send + Sync + 'static,
    ) -> Self
    where
        T: Clone + Send + 'static,
    {
        let inner = BroadcastStream::new(rx).filter_map(move |msg| {
            match msg {
                Ok(item) if filter(&item) => Some(Ok(item)),
                Ok(_) => None,
                Err(BroadcastStreamRecvError::Lagged(n)) => {
                    warn!("Stream lagged, dropped {} messages", n);
                    None // skip, keep stream alive
                }
            }
        });

        Self {
            inner: Box::pin(inner),
            channel,
            client,
            connection_id,
        }
    }
}

impl<T> Stream for SubscriptionStream<T> {
    type Item = Result<T, AppError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl<T> Drop for SubscriptionStream<T> {
    fn drop(&mut self) {
        info!(connection_id = %self.connection_id, channel = %&self.channel, "Dropping stream");
        let client = Arc::clone(&self.client);
        let channel = self.channel.clone();
        let connection_id = self.connection_id;
        tokio::spawn(async move {
            match client.unsubscribe(channel).await {
                Ok(()) => info!(connection_id = %connection_id, "Unsubscribed from stream"),
                Err(e) => {
                    error!(connection_id = %connection_id, "Failed to unsubscribe from stream: {}", e)
                }
            }
        });
    }
}
