//! What every Desk handler needs.
//!
//! Built once at startup and cloned into each request. Deliberately small: a
//! catalog pool, the configuration, and the two pieces of crypto machinery the
//! services layer expects. No cache, no message broker, no object store - Desk
//! has to work on the day those do not.

use std::sync::Arc;

use phonix_config::AppConfig;
use phonix_db::sqlx::PgPool;
use phonix_db::tenancy::Catalog;
use phonix_services::Security;
use phonix_services::crypto::Hasher;
use phonix_services::crypto::vault::SecretVault;

#[derive(Clone)]
pub struct DeskState {
    /// The catalog pool. Desk opens a tenant connection only to migrate one,
    /// and never to sign anybody in.
    pub catalog: Catalog,
    pub config: Arc<AppConfig>,
    hasher: Arc<Hasher>,
    vault: Arc<SecretVault>,
}

impl DeskState {
    pub fn new(
        catalog: Catalog,
        config: Arc<AppConfig>,
        hasher: Hasher,
        vault: SecretVault,
    ) -> Self {
        Self {
            catalog,
            config,
            hasher: Arc::new(hasher),
            vault: Arc::new(vault),
        }
    }

    /// The catalog pool, which is what every desk use case takes.
    pub fn pool(&self) -> &PgPool {
        self.catalog.pool()
    }

    /// The bundle `phonix-services` use cases expect.
    ///
    /// Borrowed rather than stored: `Security` holds references, and a struct
    /// that owns one would have to name a lifetime in every handler signature.
    pub fn security(&self) -> Security<'_> {
        Security {
            config: &self.config.security,
            hasher: &self.hasher,
            vault: &self.vault,
        }
    }

    pub fn desk(&self) -> &phonix_config::DeskConfig {
        &self.config.desk
    }

    pub fn environment(&self) -> &str {
        &self.config.app.environment
    }
}
