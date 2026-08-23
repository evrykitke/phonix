//! Exchange, queue and binding declarations.
//!
//! Declared from code at every startup rather than from a broker definitions
//! file. Declarations are idempotent, so this is safe to re-run, and it keeps
//! the topology in version control instead of inside a Docker volume.

use lapin::options::{ExchangeDeclareOptions, QueueBindOptions, QueueDeclareOptions};
use lapin::types::{AMQPValue, FieldTable, ShortString};
use lapin::{Channel, ExchangeKind};

use crate::{Messaging, MessagingError};

/// Declare the full topology: main exchange, dead-letter path, and one queue
/// per configured consumer.
pub async fn declare(messaging: &Messaging) -> Result<(), MessagingError> {
    let cfg = messaging.config();
    let channel = messaging.create_channel().await?;

    let durable = ExchangeDeclareOptions {
        durable: true,
        ..Default::default()
    };

    // Main topic exchange.
    channel
        .exchange_declare(
            cfg.exchange.as_str().into(),
            exchange_kind(&cfg.exchange_kind),
            durable,
            FieldTable::default(),
        )
        .await?;

    // Dead-letter exchange. Fanout, because a rejected message should reach the
    // dead-letter queue regardless of the routing key it failed under.
    channel
        .exchange_declare(
            cfg.dead_letter_exchange.as_str().into(),
            ExchangeKind::Fanout,
            durable,
            FieldTable::default(),
        )
        .await?;

    // Dead-letter queue. No DLX of its own: a message that fails here has
    // nowhere further to go and must be inspected by a human.
    channel
        .queue_declare(
            cfg.dead_letter_queue.as_str().into(),
            QueueDeclareOptions {
                durable: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await?;

    channel
        .queue_bind(
            cfg.dead_letter_queue.as_str().into(),
            cfg.dead_letter_exchange.as_str().into(),
            "".into(),
            QueueBindOptions::default(),
            FieldTable::default(),
        )
        .await?;

    for consumer in &cfg.consumers {
        declare_consumer_queue(
            &channel,
            &consumer.queue,
            &cfg.exchange,
            &cfg.dead_letter_exchange,
            &consumer.routing_keys,
            consumer.durable,
        )
        .await?;
    }

    tracing::info!(
        exchange = %cfg.exchange,
        dlx = %cfg.dead_letter_exchange,
        queues = cfg.consumers.len(),
        "rabbitmq topology declared"
    );

    channel.close(200, "topology declared".into()).await?;
    Ok(())
}

async fn declare_consumer_queue(
    channel: &Channel,
    queue: &str,
    exchange: &str,
    dead_letter_exchange: &str,
    routing_keys: &[String],
    durable: bool,
) -> Result<(), MessagingError> {
    // Rejected messages are routed to the DLX instead of being dropped.
    let mut args = FieldTable::default();
    args.insert(
        ShortString::from("x-dead-letter-exchange"),
        AMQPValue::LongString(dead_letter_exchange.into()),
    );
    // Quorum queues replicate and expose x-delivery-count, which is what lets a
    // consumer tell a first attempt from a fifth one.
    args.insert(
        ShortString::from("x-queue-type"),
        AMQPValue::LongString("quorum".into()),
    );

    channel
        .queue_declare(
            queue.into(),
            QueueDeclareOptions {
                durable,
                ..Default::default()
            },
            args,
        )
        .await?;

    for routing_key in routing_keys {
        channel
            .queue_bind(
                queue.into(),
                exchange.into(),
                routing_key.as_str().into(),
                QueueBindOptions::default(),
                FieldTable::default(),
            )
            .await?;

        tracing::debug!(queue, exchange, routing_key, "queue bound");
    }

    Ok(())
}

fn exchange_kind(kind: &str) -> ExchangeKind {
    match kind {
        "direct" => ExchangeKind::Direct,
        "fanout" => ExchangeKind::Fanout,
        "headers" => ExchangeKind::Headers,
        // Validated in phonix-config, so anything else cannot reach here.
        _ => ExchangeKind::Topic,
    }
}
