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
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    common::error::AppError,
    deribit::{channel::Channel, client::DeribitClient},
};

pub enum OnDrop {
    Unsubscribe { client: Arc<DeribitClient> },
    KeepAlive,
}

pub struct SubscriptionStream<T> {
    pub connection_id: Uuid,
    pub channel: Channel,
    inner: Pin<Box<dyn Stream<Item = Result<T, AppError>> + Send>>,
    on_drop: OnDrop,
}

impl<T> SubscriptionStream<T>
where
    T: Clone + Send + 'static,
{
    pub fn new(
        rx: broadcast::Receiver<T>,
        channel: Channel,
        connection_id: Uuid,
        on_drop: OnDrop,
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
                    Some(Err(AppError::InternalError(format!("Stream lagged, dropped {} messages", n))))
                }
            }
        });

        Self {
            inner: Box::pin(inner),
            channel,
            connection_id,
            on_drop,
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
        debug!(connection_id = %self.connection_id, channel = %&self.channel, "Dropping stream");

        if let OnDrop::Unsubscribe { client } = &self.on_drop {
            debug!(connection_id = %self.connection_id, channel = %&self.channel, "Unsubscribing from stream");
            let client = Arc::clone(client);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;
    use tokio_stream::StreamExt;

    use crate::deribit::channel::Channel;

    fn make_stream(
        rx: broadcast::Receiver<i32>,
        filter: impl Fn(&i32) -> bool + Send + Sync + 'static,
    ) -> SubscriptionStream<i32> {
        SubscriptionStream::new(
            rx,
            Channel::ticker("TEST"),
            Uuid::new_v4(),
            OnDrop::KeepAlive,
            filter,
        )
    }

    #[tokio::test]
    async fn filter_passes_matching_items() {
        let (tx, rx) = broadcast::channel::<i32>(16);
        let mut stream = make_stream(rx, |v| *v % 2 == 0);

        tx.send(1).unwrap(); // filtered out
        tx.send(2).unwrap(); // passes
        tx.send(3).unwrap(); // filtered out
        tx.send(4).unwrap(); // passes
        drop(tx);

        assert_eq!(stream.next().await.unwrap().unwrap(), 2);
        assert_eq!(stream.next().await.unwrap().unwrap(), 4);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn stream_ends_when_sender_dropped() {
        let (tx, rx) = broadcast::channel::<i32>(16);
        let mut stream = make_stream(rx, |_| true);
        drop(tx);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn lag_returns_error() {
        let (tx, rx) = broadcast::channel::<i32>(2); // capacity 2
        let mut stream = make_stream(rx, |_| true);

        // overflow the buffer — receiver misses oldest message
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        tx.send(3).unwrap();

        let first = stream.next().await.unwrap();
        assert!(first.is_err());
    }
}
