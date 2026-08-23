//! Creating and migrating tenant databases.

use phonix_config::DatabaseConfig;
use phonix_core::TenantSlug;
use sqlx::Connection;

use super::catalog::{Catalog, NewTenant, TenantOrigin, TenantRecord};
use crate::error::DbError;

/// Create a tenant's database, migrate it, and mark the tenant active.
///
/// Safe to call for a tenant that already exists: the catalog insert is the
/// serialisation point, and both `CREATE DATABASE` and the migrations are
/// applied conditionally.
///
/// Creates no users. A workspace with a database but nobody in it is exactly
/// what auto-provisioning and operator tooling want; the onboarding flow adds
/// the owner afterwards (see [`crate::onboarding`]).
pub async fn provision_tenant(
    catalog: &Catalog,
    cfg: &DatabaseConfig,
    slug: &TenantSlug,
    display_name: &str,
    origin: TenantOrigin,
    owner_email: Option<&str>,
) -> Result<TenantRecord, DbError> {
    let database_name = slug.database_name(&cfg.tenant_database_prefix);

    // Guard against a prefix + slug combination that exceeds Postgres' 63-byte
    // identifier limit, which would otherwise be silently truncated.
    if database_name.len() > 63 {
        return Err(DbError::CorruptCatalogRow {
            slug: slug.to_string(),
            reason: format!(
                "derived database name '{database_name}' is {} bytes, over the \
                 63-byte Postgres identifier limit",
                database_name.len()
            ),
        });
    }

    let new = NewTenant {
        slug,
        display_name,
        database_name: &database_name,
        origin,
        owner_email,
    };

    let record = match catalog.insert(new).await {
        Ok(record) => record,
        // Another request got there first. Fall through and make sure the
        // database and migrations are actually in place before returning.
        Err(DbError::TenantExists(_)) => catalog
            .find_by_slug(slug)
            .await?
            .ok_or_else(|| DbError::UnknownTenant(slug.to_string()))?,
        Err(err) => return Err(err),
    };

    create_database_if_absent(cfg, &record.database_name).await?;
    migrate_tenant(cfg, &record.database_name).await?;

    catalog
        .mark_active(slug, &latest_tenant_schema_version())
        .await?;

    catalog
        .find_by_slug(slug)
        .await?
        .ok_or_else(|| DbError::UnknownTenant(slug.to_string()))
}

/// Bring every tenant database up to the current schema.
///
/// Without this, adding a migration reaches new workspaces only. Existing ones
/// keep the schema they were created with, and the first query that needs a new
/// column fails at runtime, per tenant, in production - which is the worst
/// possible place to discover a migration was written.
///
/// Runs on boot behind `database.migrate_on_start`, the same flag that governs
/// the catalog. Sequential rather than concurrent: a migration takes locks, and
/// a hundred tenants racing for connections at boot is a thundering herd for no
/// gain on a step that runs once per deploy.
///
/// One tenant failing does not stop the rest. The error is logged with its
/// slug and the count comes back, because a workspace that cannot be migrated
/// is a problem for that workspace, and refusing to boot at all would take out
/// every other one with it.
pub async fn migrate_outdated_tenants(
    catalog: &Catalog,
    cfg: &DatabaseConfig,
) -> Result<MigrationSweep, DbError> {
    let latest = latest_tenant_schema_version();
    let mut sweep = MigrationSweep::default();

    for tenant in catalog.list().await? {
        if tenant.schema_version.as_deref() == Some(latest.as_str()) {
            sweep.current += 1;
            continue;
        }

        tracing::info!(
            slug = %tenant.slug,
            from = tenant.schema_version.as_deref().unwrap_or("none"),
            to = %latest,
            "migrating tenant database"
        );

        match migrate_tenant(cfg, &tenant.database_name).await {
            Ok(()) => {
                catalog.mark_active(&tenant.slug, &latest).await?;
                sweep.migrated += 1;
            }
            Err(err) => {
                tracing::error!(slug = %tenant.slug, %err, "tenant migration failed");
                sweep.failed.push(tenant.slug.to_string());
            }
        }
    }

    Ok(sweep)
}

/// What [`migrate_outdated_tenants`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationSweep {
    /// Already at the current schema.
    pub current: usize,
    pub migrated: usize,
    /// Slugs whose migration failed. Logged individually as they happen.
    pub failed: Vec<String>,
}

/// The highest version in `migrations/tenant/`, zero-padded.
///
/// Read from the embedded migrator rather than written down, so adding a
/// migration cannot leave the catalog claiming an older schema than the tenant
/// databases actually have.
pub fn latest_tenant_schema_version() -> String {
    let version = crate::TENANT_MIGRATIONS
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or(0);
    format!("{version:04}")
}

/// `CREATE DATABASE`, skipped when it already exists.
///
/// The name is interpolated rather than bound because Postgres does not accept
/// parameters for identifiers. That is safe here only because the name is
/// derived from a [`TenantSlug`], which is restricted to `[a-z0-9-]` at
/// construction; the assertion below refuses anything else outright.
async fn create_database_if_absent(cfg: &DatabaseConfig, database: &str) -> Result<(), DbError> {
    assert_safe_identifier(database)?;

    let mut conn = crate::connect::maintenance_connection(cfg).await?;

    let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM pg_database WHERE datname = $1")
        .bind(database)
        .fetch_optional(&mut conn)
        .await
        .map_err(DbError::Query)?;

    if exists.is_some() {
        tracing::debug!(database, "tenant database already exists");
        conn.close().await.ok();
        return Ok(());
    }

    tracing::info!(database, "creating tenant database");

    // CREATE DATABASE cannot run inside a transaction block, which is why this
    // uses a bare connection rather than the pool's transaction helpers.
    //
    // `AssertSqlSafe` is required because sqlx 0.9 rejects runtime-built SQL by
    // default. It is justified here and only here: Postgres has no bind
    // parameter for an identifier, and `database` has just been through
    // `assert_safe_identifier`, which allows only `[a-z0-9_]`.
    let sql = format!(r#"CREATE DATABASE "{database}" ENCODING 'UTF8'"#);
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(&mut conn)
        .await
        .map_err(DbError::Query)?;

    conn.close().await.ok();
    Ok(())
}

/// Apply the tenant migrations to one tenant database.
pub async fn migrate_tenant(cfg: &DatabaseConfig, database: &str) -> Result<(), DbError> {
    assert_safe_identifier(database)?;

    let pool = crate::connect::tenant_pool(cfg, database);

    let result = crate::TENANT_MIGRATIONS
        .run(&pool)
        .await
        .map_err(|source| DbError::Migrate {
            target: database.to_owned(),
            source,
        });

    // The pool exists only for this migration; hold no connections afterwards.
    pool.close().await;
    result?;

    tracing::info!(database, "tenant migrations applied");
    Ok(())
}

/// Drop a tenant database. Irreversible; intended for tests and tooling.
pub async fn drop_tenant_database(cfg: &DatabaseConfig, database: &str) -> Result<(), DbError> {
    assert_safe_identifier(database)?;

    let mut conn = crate::connect::maintenance_connection(cfg).await?;

    tracing::warn!(database, "dropping tenant database");

    // See the note in `create_database_if_absent` for why this is asserted safe.
    let sql = format!(r#"DROP DATABASE IF EXISTS "{database}" WITH (FORCE)"#);
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(&mut conn)
        .await
        .map_err(DbError::Query)?;

    conn.close().await.ok();
    Ok(())
}

/// Last line of defence before a name reaches DDL.
///
/// Everything upstream already constrains these names, so a failure here means
/// a bug or a tampered catalog row rather than bad user input.
fn assert_safe_identifier(name: &str) -> Result<(), DbError> {
    let valid = !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_');

    if valid {
        Ok(())
    } else {
        Err(DbError::CorruptCatalogRow {
            slug: name.to_owned(),
            reason: "database name is not a safe bare Postgres identifier".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_names_that_would_break_out_of_the_quotes() {
        for bad in [
            "",
            "phonix_tenant_acme\"; DROP DATABASE postgres; --",
            "phonix tenant",
            "Phonix_Tenant",
            "1phonix",
            "phonix-tenant-acme",
            &"a".repeat(64),
        ] {
            assert!(assert_safe_identifier(bad).is_err(), "{bad:?} must fail");
        }
    }

    #[test]
    fn accepts_generated_names() {
        assert!(assert_safe_identifier("phonix_tenant_acme").is_ok());
        assert!(assert_safe_identifier("phonix_tenant_north_wind").is_ok());
    }
}
