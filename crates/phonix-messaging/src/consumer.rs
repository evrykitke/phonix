//! Consuming tenant-scoped events.
//!
//! Delivery is at-least-once, so handlers must be idempotent. The tenant
//! databases carry a `processed_events` table for exactly this purpose.

use std::sync::Arc;

use futures::StreamExt;
use lapin::message::Delivery;
use lapin::options::{BasicAckOptions, BasicConsumeOptions, BasicNackOptions, BasicQosOptions};
use lapin::types::{AMQPValue, FieldTable};
use phonix_config::ConsumerConfig;
use phonix_core::TenantSlug;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{Messaging, MessagingError};

/// Cooperative shutdown signal shared by every consumer task.
pub type ShutdownSignal = CancellationToken;

/// A message handed to a [`MessageHandler`].
pub struct IncomingMessage {
    /// Tenant from the `x-tenant` header, when present and valid.
    pub tenant: Option<TenantSlug>,
    pub routing_key: String,
    pub payload: Vec<u8>,
    /// AMQP `message_id`, used for idempotency checks.
    pub event_id: Option<String>,
    /// How many times the broker has delivered this message, starting at 1.
    pub delivery_attempt: u32,
}

impl IncomingMessage {
    /// Deserialise the payload as JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(&self.payload)
    }
}

/// Application logic for one queue.
#[async_trait::async_trait]
pub trait MessageHandler: Send + Sync + 'static {
    /// Handle a message.
    ///
    /// `Ok(())` acknowledges. `Err(_)` retries until the configured attempt
    /// limit, after which the message is dead-lettered.
    async fn handle(&self, message: IncomingMessage) -> Result<(), HandlerError>;
}

#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    /// Transient: retry.
    #[error("transient failure: {0}")]
    Retry(String),

    /// Permanent: dead-letter immediately, no retries. Use for malformed
    /// payloads, which will never succeed however many times they are retried.
    #[error("permanent failure: {0}")]
    Reject(String),
}

/// Handle to a running consumer task.
pub struct ConsumerHandle {
    pub name: String,
    task: JoinHandle<()>,
    shutdown: ShutdownSignal,
}

impl ConsumerHandle {
    /// Signal the consumer to stop and wait for the in-flight message to finish.
    pub async fn shutdown(self) {
        self.shutdown.cancel();
        if let Err(err) = self.task.await {
            tracing::warn!(consumer = %self.name, error = %err, "consumer task ended abnormally");
        }
    }
}

/// Start a consumer for one configured queue.
pub async fn spawn(
    messaging: &Messaging,
    cfg: &ConsumerConfig,
    handler: Arc<dyn MessageHandler>,
) -> Result<ConsumerHandle, MessagingError> {
    let channel = messaging.create_channel().await?;

    // Without a prefetch limit the broker pushes the whole queue at one
    // consumer, which defeats concurrency and can exhaust memory.
    let prefetch = messaging.config().prefetch_count;
    channel
        .basic_qos(prefetch, BasicQosOptions::default())
        .await?;

    let consumer_tag = format!("{}-{}", cfg.name, uuid::Uuid::now_v7());
    let consumer = channel
        .basic_consume(
            cfg.queue.as_str().into(),
            consumer_tag.as_str().into(),
            BasicConsumeOptions {
                no_ack: cfg.auto_ack,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await?;

    tracing::info!(
        consumer = %cfg.name,
        queue = %cfg.queue,
        prefetch,
        "consumer started"
    );

    let shutdown = ShutdownSignal::new();
    let task = tokio::spawn(run(
        consumer,
        handler,
        cfg.clone(),
        messaging.config().max_delivery_attempts,
        messaging.config().retry_initial_backoff_ms,
        messaging.config().retry_max_backoff_ms,
        shutdown.clone(),
    ));

    Ok(ConsumerHandle {
        name: cfg.name.clone(),
        task,
        shutdown,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run(
    mut consumer: lapin::Consumer,
    handler: Arc<dyn MessageHandler>,
    cfg: ConsumerConfig,
    max_attempts: u32,
    initial_backoff_ms: u64,
    max_backoff_ms: u64,
    shutdown: ShutdownSignal,
) {
    loop {
        let delivery = tokio::select! {
            // Biased so a pending shutdown wins over a ready message, letting
            // the process drain rather than pulling in more work.
            biased;
            _ = shutdown.cancelled() => {
                tracing::info!(consumer = %cfg.name, "consumer shutting down");
                return;
            }
            next = consumer.next() => next,
        };

        let delivery = match delivery {
            Some(Ok(delivery)) => delivery,
            Some(Err(err)) => {
                tracing::error!(consumer = %cfg.name, error = %err, "delivery error");
                continue;
            }
            // The stream ends when the channel or connection closes.
            None => {
                tracing::warn!(consumer = %cfg.name, "consumer stream ended");
                return;
            }
        };

        handle_delivery(
            &cfg,
            &handler,
            delivery,
            max_attempts,
            initial_backoff_ms,
            max_backoff_ms,
        )
        .await;
    }
}

async fn handle_delivery(
    cfg: &ConsumerConfig,
    handler: &Arc<dyn MessageHandler>,
    delivery: Delivery,
    max_attempts: u32,
    initial_backoff_ms: u64,
    max_backoff_ms: u64,
) {
    let attempt = delivery_attempt(&delivery);
    let message = IncomingMessage {
        tenant: tenant_from(&delivery),
        routing_key: delivery.routing_key.to_string(),
        payload: delivery.data.clone(),
        event_id: delivery
            .properties
            .message_id()
            .as_ref()
            .map(|id| id.to_string()),
        delivery_attempt: attempt,
    };

    let span = tracing::info_span!(
        "consume",
        consumer = %cfg.name,
        routing_key = %message.routing_key,
        attempt,
    );
    let _guard = span.enter();

    match handler.handle(message).await {
        Ok(()) => {
            if !cfg.auto_ack
                && let Err(err) = delivery.ack(BasicAckOptions::default()).await
            {
                tracing::error!(error = %err, "failed to ack message");
            }
        }

        // Permanent failure: straight to the dead-letter exchange.
        Err(HandlerError::Reject(reason)) => {
            tracing::error!(reason, "rejecting message permanently");
            nack(&delivery, false).await;
        }

        Err(HandlerError::Retry(reason)) if attempt >= max_attempts => {
            tracing::error!(
                reason,
                attempt,
                max_attempts,
                "retry limit reached; dead-lettering message"
            );
            nack(&delivery, false).await;
        }

        Err(HandlerError::Retry(reason)) => {
            // Backoff before requeueing: an immediate requeue would spin the
            // consumer against a dependency that is still down.
            let backoff = backoff_for(attempt, initial_backoff_ms, max_backoff_ms);
            tracing::warn!(reason, attempt, backoff_ms = backoff, "retrying message");
            tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
            nack(&delivery, true).await;
        }
    }
}

async fn nack(delivery: &Delivery, requeue: bool) {
    let options = BasicNackOptions {
        requeue,
        multiple: false,
    };
    if let Err(err) = delivery.nack(options).await {
        tracing::error!(error = %err, requeue, "failed to nack message");
    }
}

/// Exponential backoff, capped.
fn backoff_for(attempt: u32, initial_ms: u64, max_ms: u64) -> u64 {
    initial_ms
        .saturating_mul(2u64.saturating_pow(attempt.saturating_sub(1).min(16)))
        .min(max_ms)
}

/// Read the tenant from the `x-tenant` header.
///
/// An invalid value is treated as absent rather than as an error: a handler that
/// needs a tenant will reject the message itself, with better context.
fn tenant_from(delivery: &Delivery) -> Option<TenantSlug> {
    let headers = delivery.properties.headers().as_ref()?;
    let value = headers.inner().get("x-tenant")?;

    let raw = match value {
        AMQPValue::LongString(s) => s.to_string(),
        AMQPValue::ShortString(s) => s.to_string(),
        _ => return None,
    };

    TenantSlug::parse(raw).ok()
}

/// Which delivery attempt this is, starting at 1.
///
/// Quorum queues maintain `x-delivery-count`; on other queue types only the
/// `redelivered` flag is available, which distinguishes a first attempt from a
/// later one but cannot count them.
fn delivery_attempt(delivery: &Delivery) -> u32 {
    let from_header = delivery
        .properties
        .headers()
        .as_ref()
        .and_then(|headers| headers.inner().get("x-delivery-count").cloned())
        .and_then(|value| match value {
            AMQPValue::LongLongInt(n) => u32::try_from(n).ok(),
            AMQPValue::LongInt(n) => u32::try_from(n).ok(),
            AMQPValue::ShortInt(n) => u32::try_from(n).ok(),
            _ => None,
        });

    match from_header {
        Some(count) => count + 1,
        None if delivery.redelivered => 2,
        None => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_then_saturates() {
        assert_eq!(backoff_for(1, 500, 30_000), 500);
        assert_eq!(backoff_for(2, 500, 30_000), 1_000);
        assert_eq!(backoff_for(3, 500, 30_000), 2_000);
        assert_eq!(backoff_for(10, 500, 30_000), 30_000);
        // Must not overflow for an absurd attempt count.
        assert_eq!(backoff_for(u32::MAX, 500, 30_000), 30_000);
    }
}
