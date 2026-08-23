//! Publishing tenant-scoped events.

use std::sync::Arc;

use lapin::options::BasicPublishOptions;
use lapin::types::{AMQPValue, FieldTable, ShortString};
use lapin::{BasicProperties, Channel};
use phonix_core::TenantSlug;
use serde::Serialize;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{Messaging, MessagingError, tenant_routing_key};

/// A message ready to publish.
pub struct OutgoingMessage {
    /// Appended to `tenant.<slug>.` to form the routing key.
    pub routing_suffix: String,
    pub payload: Vec<u8>,
    pub content_type: &'static str,
    /// Deduplication key, echoed as the AMQP `message_id`.
    pub event_id: Uuid,
}

impl OutgoingMessage {
    /// Build a JSON message.
    pub fn json<T: Serialize>(
        routing_suffix: impl Into<String>,
        payload: &T,
    ) -> Result<Self, MessagingError> {
        Ok(Self {
            routing_suffix: routing_suffix.into(),
            payload: serde_json::to_vec(payload)?,
            content_type: "application/json",
            event_id: Uuid::now_v7(),
        })
    }
}

/// Publishes to the configured topic exchange.
///
/// Holds one channel behind a mutex rather than a channel pool: lapin serialises
/// frames internally, and a single confirm-mode channel keeps publisher confirms
/// simple to reason about. The channel is recreated transparently if the broker
/// drops it.
#[derive(Clone)]
pub struct Publisher {
    messaging: Messaging,
    channel: Arc<Mutex<Channel>>,
}

impl Publisher {
    pub(crate) async fn new(messaging: Messaging) -> Result<Self, MessagingError> {
        let channel = Self::open_channel(&messaging).await?;
        Ok(Self {
            messaging,
            channel: Arc::new(Mutex::new(channel)),
        })
    }

    async fn open_channel(messaging: &Messaging) -> Result<Channel, MessagingError> {
        let channel = messaging.create_channel().await?;

        if messaging.config().publisher_confirms {
            // Without this the broker acknowledges nothing and `basic_publish`
            // returns as soon as the bytes are written to the socket, which is
            // not the same as the message being safely stored.
            channel
                .confirm_select(lapin::options::ConfirmSelectOptions::default())
                .await?;
        }

        Ok(channel)
    }

    /// Return the live channel, reopening it if the broker closed it.
    async fn channel(&self) -> Result<Channel, MessagingError> {
        let mut guard = self.channel.lock().await;

        if !guard.status().connected() {
            tracing::warn!("publisher channel was closed; reopening");
            *guard = Self::open_channel(&self.messaging).await?;
        }

        Ok(guard.clone())
    }

    /// Publish an event for a tenant.
    ///
    /// With `publisher_confirms` enabled this waits for the broker's ack, so a
    /// returned `Ok` means the message is durably stored - not merely sent.
    pub async fn publish(
        &self,
        tenant: &TenantSlug,
        message: OutgoingMessage,
    ) -> Result<(), MessagingError> {
        let cfg = self.messaging.config();
        let routing_key = tenant_routing_key(tenant.as_str(), &message.routing_suffix);
        let channel = self.channel().await?;

        // The tenant travels in a header as well as the routing key so consumers
        // do not have to parse the key to know which database to open.
        let mut headers = FieldTable::default();
        headers.insert(
            ShortString::from("x-tenant"),
            AMQPValue::LongString(tenant.as_str().into()),
        );

        let properties = BasicProperties::default()
            // 2 = persistent: survives a broker restart. Pointless without a
            // durable exchange and queue, which topology.rs declares.
            .with_delivery_mode(2)
            .with_content_type(message.content_type.into())
            .with_message_id(message.event_id.to_string().into())
            .with_timestamp(chrono::Utc::now().timestamp() as u64)
            .with_headers(headers);

        let confirm = channel
            .basic_publish(
                cfg.exchange.as_str().into(),
                routing_key.as_str().into(),
                BasicPublishOptions::default(),
                &message.payload,
                properties,
            )
            .await?;

        if cfg.publisher_confirms {
            let confirmation = confirm.await?;

            if confirmation.is_nack() {
                return Err(MessagingError::PublishNotConfirmed { routing_key });
            }
        }

        tracing::debug!(
            tenant = %tenant,
            routing_key = %routing_key,
            event_id = %message.event_id,
            bytes = message.payload.len(),
            "published event"
        );

        Ok(())
    }

    /// Convenience wrapper for JSON payloads.
    pub async fn publish_json<T: Serialize>(
        &self,
        tenant: &TenantSlug,
        routing_suffix: impl Into<String>,
        payload: &T,
    ) -> Result<(), MessagingError> {
        let message = OutgoingMessage::json(routing_suffix, payload)?;
        self.publish(tenant, message).await
    }
}
