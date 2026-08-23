//! RabbitMQ messaging.
//!
//! Topology (declared idempotently at startup, so the broker's data volume is
//! never the source of truth):
//!
//! ```text
//!   phonix.events (topic)  --routing key: tenant.<slug>.<aggregate>.<event>
//!        |
//!        +--> phonix.tenant-events   (durable, DLX -> phonix.dlx)
//!
//!   phonix.dlx (fanout) --> phonix.dlq
//! ```
//!
//! Every message carries its tenant in the routing key *and* in a `x-tenant`
//! header, so a consumer can route to the right database without parsing the
//! key.

pub mod consumer;
pub mod publisher;
pub mod topology;

pub use consumer::{ConsumerHandle, IncomingMessage, MessageHandler};
pub use publisher::{OutgoingMessage, Publisher};

use std::sync::Arc;

use lapin::uri::{AMQPScheme, AMQPUri};
use lapin::{Connection, ConnectionProperties};
use phonix_config::RabbitMqConfig;
use secrecy::ExposeSecret;

#[derive(Debug, thiserror::Error)]
pub enum MessagingError {
    #[error("could not connect to rabbitmq at {addr}: {source}")]
    Connect {
        addr: String,
        #[source]
        source: lapin::Error,
    },

    #[error("amqp operation failed: {0}")]
    Amqp(#[from] lapin::Error),

    #[error("could not serialise message payload: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("broker did not confirm the publish of '{routing_key}'")]
    PublishNotConfirmed { routing_key: String },

    #[error("messaging is disabled in configuration")]
    Disabled,
}

/// A live connection to RabbitMQ plus the configuration it was built from.
#[derive(Clone)]
pub struct Messaging {
    connection: Arc<Connection>,
    config: Arc<RabbitMqConfig>,
}

impl Messaging {
    /// Connect and declare the topology.
    #[allow(clippy::field_reassign_with_default)]
    pub async fn connect(cfg: Arc<RabbitMqConfig>) -> Result<Self, MessagingError> {
        let display_addr = format!("{}:{}/{}", cfg.host, cfg.port, cfg.vhost);

        // Built field-by-field rather than as an `amqp://` URL: the vhost and
        // the password would otherwise both need percent-encoding, and a vhost
        // of "/" is a notorious source of connection bugs. A struct literal is
        // not an option: `AMQPUri` is non-exhaustive and half of what is set
        // here lives two levels down inside `authority`.
        let mut uri = AMQPUri::default();
        uri.scheme = if cfg.use_tls {
            AMQPScheme::AMQPS
        } else {
            AMQPScheme::AMQP
        };
        uri.authority.host = cfg.host.clone();
        uri.authority.port = cfg.port;
        uri.authority.userinfo.username = cfg.username.clone();
        uri.authority.userinfo.password = cfg.password.expose_secret().to_owned();
        uri.vhost = cfg.vhost.clone();
        uri.query.heartbeat = Some(cfg.heartbeat_secs);
        uri.query.connection_timeout = Some(cfg.connect_timeout_secs * 1_000);

        // Auto-recovery reconnects and replays the declared topology after a
        // broker restart, which is why the app does not need its own retry loop.
        let properties = ConnectionProperties::default().enable_auto_recover();

        let connection = Connection::connect_uri(uri, properties)
            .await
            .map_err(|source| MessagingError::Connect {
                addr: display_addr.clone(),
                source,
            })?;

        tracing::info!(addr = %display_addr, "connected to rabbitmq");

        let messaging = Self {
            connection: Arc::new(connection),
            config: cfg,
        };

        topology::declare(&messaging).await?;

        Ok(messaging)
    }

    pub fn config(&self) -> &RabbitMqConfig {
        &self.config
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Whether the underlying connection is currently usable.
    pub fn is_connected(&self) -> bool {
        self.connection.status().connected()
    }

    /// Open a fresh channel.
    ///
    /// Channels are cheap but not free, and they are not safe to share across
    /// independent consumers, so each consumer gets its own.
    pub async fn create_channel(&self) -> Result<lapin::Channel, MessagingError> {
        Ok(self.connection.create_channel().await?)
    }

    /// Build a publisher over this connection.
    pub async fn publisher(&self) -> Result<Publisher, MessagingError> {
        Publisher::new(self.clone()).await
    }

    /// Close the connection during graceful shutdown.
    pub async fn close(&self) {
        if let Err(err) = self.connection.close(200, "shutting down".into()).await {
            tracing::warn!(error = %err, "error while closing the rabbitmq connection");
        }
    }
}

/// Routing key for a tenant-scoped event: `tenant.<slug>.<suffix>`.
pub fn tenant_routing_key(slug: &str, suffix: &str) -> String {
    format!("tenant.{slug}.{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_keys_are_tenant_scoped() {
        assert_eq!(
            tenant_routing_key("acme", "invoice.created"),
            "tenant.acme.invoice.created"
        );
    }
}
