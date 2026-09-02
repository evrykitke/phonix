//! The tenant pool registry: slug -> live `PgPool`.
//!
//! Two caches, with deliberately different lifetimes:
//!
//! * **lookup cache** - catalog rows, short TTL. Keeps a suspended tenant from
//!   serving traffic for long after the change.
//! * **pool cache**   - live `PgPool`s, evicted by idleness and capacity. This
//!   is what bounds total Postgres connections: at most
//!   `max_cached_pools * tenant_pool.max_connections`.

use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;
use phonix_config::DatabaseConfig;
use phonix_core::TenantSlug;
use sqlx::PgPool;

use super::catalog::{Catalog, TenantRecord};
use crate::error::DbError;

/// A resolved tenant: its catalog row plus a pool to its database.
#[derive(Clone)]
pub struct TenantHandle {
    pub record: Arc<TenantRecord>,
    pub pool: PgPool,
}

#[derive(Clone)]
pub struct TenantRegistry {
    catalog: Catalog,
    config: Arc<DatabaseConfig>,
    lookups: Cache<String, Arc<TenantRecord>>,
    pools: Cache<String, PgPool>,
}

impl TenantRegistry {
    pub fn new(catalog: Catalog, config: Arc<DatabaseConfig>) -> Self {
        let registry = &config.tenant_registry;

        let lookups = Cache::builder()
            .max_capacity(registry.lookup_cache_capacity)
            .time_to_live(Duration::from_secs(registry.lookup_cache_ttl_secs))
            .build();

        let pools = Cache::builder()
            .max_capacity(registry.max_cached_pools)
            // Idle eviction, not TTL: a busy tenant should keep its pool
            // indefinitely, while a tenant nobody has touched should release
            // its connections back to Postgres.
            .time_to_idle(Duration::from_secs(registry.idle_evict_secs))
            .eviction_listener(|slug: Arc<String>, pool: PgPool, cause| {
                tracing::debug!(tenant = %slug, ?cause, "closing idle tenant pool");
                // `PgPool::close` is async and the listener is sync, so the
                // close is spawned. Dropping the pool alone would also release
                // the connections, but only once every clone is gone.
                tokio::spawn(async move { pool.close().await });
            })
            .build();

        Self {
            catalog,
            config,
            lookups,
            pools,
        }
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Resolve a slug to a usable pool, creating one if needed.
    ///
    /// Returns [`DbError::UnknownTenant`] for an unregistered slug,
    /// [`DbError::TenantInactive`] for a suspended or archived one, and
    /// [`DbError::TenantUnlicensed`] for one whose licence has lapsed or been
    /// withdrawn. The last two are both 403 and are deliberately different
    /// errors - see ADR 0005 section 7.
    pub async fn resolve(&self, slug: &TenantSlug) -> Result<TenantHandle, DbError> {
        let record = self.record_for(slug).await?;
        self.open(record).await
    }

    /// The same thing, for a caller that already holds the catalog row.
    ///
    /// [`resolve`](Self::resolve) starts from a slug, so it has to read the
    /// catalog to learn which database the tenant lives in. A caller that has
    /// *just* read the catalog itself would otherwise hand the slug back and
    /// have the identical row read a second time - and the background sweep in
    /// `phonix-server` reads every row at the top of every pass, so on an idle
    /// deployment that second read was the majority of all catalog traffic.
    ///
    /// The row is used as given. That is only correct for a row the caller read
    /// moments ago: a stale one names a database the tenant may since have been
    /// moved off. Anything holding a row of unknown age wants `resolve`.
    pub async fn resolve_record(&self, record: TenantRecord) -> Result<TenantHandle, DbError> {
        self.open(Arc::new(record)).await
    }

    /// Attach a pool to a row, refusing one that may not serve traffic.
    ///
    /// The shared tail of both entry points, so that the check cannot come to
    /// mean one thing for a request and another for a background pass.
    ///
    /// The licence is asked about first, and separately, so the refusal can
    /// name which half said no. `serves_traffic` then decides both together -
    /// neither half can widen the other.
    async fn open(&self, record: Arc<TenantRecord>) -> Result<TenantHandle, DbError> {
        if let Some(problem) = record.licence_problem() {
            return Err(DbError::TenantUnlicensed {
                slug: record.slug.to_string(),
                standing: problem.as_str().to_owned(),
                reason: problem.refusal().to_owned(),
            });
        }

        if !record.serves_traffic() {
            return Err(DbError::TenantInactive {
                slug: record.slug.to_string(),
                status: record.status.as_str().to_owned(),
            });
        }

        let pool = self.pool_for(&record).await;

        Ok(TenantHandle { record, pool })
    }

    /// Fetch a catalog row, going through the short-lived lookup cache.
    async fn record_for(&self, slug: &TenantSlug) -> Result<Arc<TenantRecord>, DbError> {
        if let Some(cached) = self.lookups.get(slug.as_str()).await {
            return Ok(cached);
        }

        let record = self
            .catalog
            .find_by_slug(slug)
            .await?
            .ok_or_else(|| DbError::UnknownTenant(slug.to_string()))?;

        // Negative results are intentionally NOT cached: an unknown slug is
        // usually a scan or a typo, and caching misses would let an attacker
        // fill the cache with junk keys.
        let record = Arc::new(record);
        self.lookups
            .insert(slug.to_string(), Arc::clone(&record))
            .await;

        Ok(record)
    }

    /// Get or create the pool for a tenant.
    ///
    /// `get_with` collapses concurrent first requests for the same tenant into
    /// a single initialisation, so a burst of traffic to a cold tenant creates
    /// one pool rather than one per request.
    async fn pool_for(&self, record: &TenantRecord) -> PgPool {
        let key = record.slug.to_string();
        let config = Arc::clone(&self.config);
        let database = record.database_name.clone();

        self.pools
            .get_with(key, async move {
                tracing::info!(database = %database, "opening tenant pool");
                crate::connect::tenant_pool(&config, &database)
            })
            .await
    }

    /// Drop a tenant's cached row and pool.
    ///
    /// Call after suspending, migrating or relocating a tenant so the next
    /// request re-reads the catalog instead of using stale routing.
    pub async fn invalidate(&self, slug: &TenantSlug) {
        self.lookups.invalidate(slug.as_str()).await;
        self.pools.invalidate(slug.as_str()).await;
        tracing::debug!(tenant = %slug, "invalidated tenant registry entry");
    }

    /// Number of live tenant pools, for health output and metrics.
    pub fn live_pools(&self) -> u64 {
        self.pools.entry_count()
    }

    /// Close every pool. Called during graceful shutdown.
    pub async fn close_all(&self) {
        for (_, pool) in self.pools.iter() {
            pool.close().await;
        }
        self.pools.invalidate_all();
        self.lookups.invalidate_all();
    }
}
