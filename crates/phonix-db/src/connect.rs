//! Building connect options and pools.
//!
//! Connections are configured field-by-field through [`PgConnectOptions`]
//! rather than by formatting a URL. A password such as `l0l0ting@2209` contains
//! characters that are structural in a URL, and percent-encoding it correctly
//! at every call site is a bug waiting to happen.

use std::time::Duration;

use phonix_config::{DatabaseConfig, PoolConfig, SslMode};
use secrecy::ExposeSecret;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgSslMode};
use sqlx::{Connection, PgConnection};

use crate::error::DbError;

/// The search path every connection to a tenant database is opened with.
///
/// `core` first, because the infrastructure tables are referenced unqualified
/// throughout this crate and there are some hundred and thirty of those.
/// `public` behind it, permanently rather than transitionally: pgcrypto is
/// installed there, so this is what keeps `gen_random_uuid()` resolving.
///
/// **App schemas are deliberately absent**, and that absence is the point. An
/// app's tables are always written `books.invoices`; a schema that is not on
/// the path cannot be reached by a query that forgot to say which app it meant,
/// so the mistake surfaces as a loud error rather than a silent read of the
/// wrong table. See `docs/adr/0001-core-boundary.md`.
///
/// No spaces: the value travels to the server inside a space-separated `-c`
/// options string, and `core, public` would arrive as two of them.
pub const TENANT_SEARCH_PATH: &str = "core,public";

/// Connect options for a named database on the configured server.
///
/// No `search_path`: this serves the catalog and maintenance databases, whose
/// tables live in `public` where Postgres already looks. Tenant connections go
/// through [`tenant_connect_options`] instead.
pub fn connect_options(cfg: &DatabaseConfig, database: &str) -> PgConnectOptions {
    let mut options = PgConnectOptions::new()
        .host(&cfg.host)
        .port(cfg.port)
        .username(&cfg.username)
        .password(cfg.password.expose_secret())
        .database(database)
        .ssl_mode(ssl_mode(cfg.ssl_mode))
        .application_name(&cfg.application_name);

    // A server-side statement timeout is the only backstop that survives a
    // client crash or a dropped connection, so it is set on the session rather
    // than relying on client-side cancellation alone.
    if cfg.statement_timeout_secs > 0 {
        options = options.options([(
            "statement_timeout",
            format!("{}s", cfg.statement_timeout_secs).as_str(),
        )]);
    }

    options
}

fn ssl_mode(mode: SslMode) -> PgSslMode {
    match mode {
        SslMode::Disable => PgSslMode::Disable,
        SslMode::Prefer => PgSslMode::Prefer,
        SslMode::Require => PgSslMode::Require,
        SslMode::VerifyCa => PgSslMode::VerifyCa,
        SslMode::VerifyFull => PgSslMode::VerifyFull,
    }
}

fn pool_options(pool: &PoolConfig) -> PgPoolOptions {
    let mut options = PgPoolOptions::new()
        .max_connections(pool.max_connections)
        .min_connections(pool.min_connections)
        .acquire_timeout(Duration::from_secs(pool.acquire_timeout_secs))
        .test_before_acquire(pool.test_before_acquire);

    // 0 means "no limit"; sqlx expresses that as None rather than a zero duration.
    if pool.idle_timeout_secs > 0 {
        options = options.idle_timeout(Duration::from_secs(pool.idle_timeout_secs));
    }
    if pool.max_lifetime_secs > 0 {
        options = options.max_lifetime(Duration::from_secs(pool.max_lifetime_secs));
    }

    options
}

/// Open the pool for the shared catalog database.
///
/// Connects eagerly so that a bad password or an unreachable server is reported
/// during startup rather than on the first request.
pub async fn catalog_pool(cfg: &DatabaseConfig) -> Result<PgPool, DbError> {
    let target = cfg.redacted_url(&cfg.catalog_database);
    tracing::info!(database = %cfg.catalog_database, url = %target, "connecting to catalog database");

    pool_options(&cfg.catalog_pool)
        .connect_with(connect_options(cfg, &cfg.catalog_database))
        .await
        .map_err(|source| DbError::Connect { target, source })
}

/// Connect options for a tenant database, opened on a given search path.
///
/// `options` appends rather than replaces, so the statement timeout set by
/// [`connect_options`] survives.
pub fn tenant_connect_options(
    cfg: &DatabaseConfig,
    database: &str,
    search_path: &str,
) -> PgConnectOptions {
    connect_options(cfg, database).options([("search_path", search_path)])
}

/// Open a pool for one tenant database.
///
/// Uses `connect_lazy_with`: the pool is handed back immediately and the first
/// query establishes the connection. Tenant pools are created inside request
/// handling, so paying a TCP+TLS handshake before returning would add latency
/// to a request that may not even touch the database.
pub fn tenant_pool(cfg: &DatabaseConfig, database: &str) -> PgPool {
    pool_options(&cfg.tenant_pool).connect_lazy_with(tenant_connect_options(
        cfg,
        database,
        TENANT_SEARCH_PATH,
    ))
}

/// A one-connection pool for running one app's migration stream.
///
/// Single connection because migrations are applied in order behind an advisory
/// lock, so a second one would only ever sit and wait for the first. The pool is
/// built per app rather than reused, because each app's migrations run on their
/// own search path - which is what puts `books._sqlx_migrations` inside the
/// `books` schema and keeps the streams independent.
pub fn schema_migration_pool(cfg: &DatabaseConfig, database: &str, search_path: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy_with(tenant_connect_options(cfg, database, search_path))
}

/// A single connection to the maintenance database (`postgres`).
///
/// `CREATE DATABASE` and `DROP DATABASE` cannot run inside a transaction or
/// from within the target database, so provisioning needs its own short-lived
/// connection somewhere else.
pub async fn maintenance_connection(cfg: &DatabaseConfig) -> Result<PgConnection, DbError> {
    let target = cfg.redacted_url(&cfg.maintenance_database);

    PgConnection::connect_with(&connect_options(cfg, &cfg.maintenance_database))
        .await
        .map_err(|source| DbError::Connect { target, source })
}
