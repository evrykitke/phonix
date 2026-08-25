//! Creating and migrating tenant databases.

use phonix_config::DatabaseConfig;
use phonix_core::TenantSlug;
use sqlx::Connection;

use super::apps::{self, AppMigrations};
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
        .mark_active(slug, &apps::schema_fingerprint())
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
    let latest = apps::schema_fingerprint();
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

/// Bring one tenant database up to date, app by app.
///
/// Each app owns a schema and a migration stream, applied on a search path
/// rooted at that schema - so sqlx's own `_sqlx_migrations` bookkeeping lands
/// inside the app's schema and the streams stay independent without sqlx
/// needing to know that apps exist.
///
/// Sequential, and in registry order: core first, because `core.installed_apps`
/// is what the others record themselves in.
///
/// A failing app aborts the pass. Stopping is right here even though the boot
/// sweep goes on to the next *tenant*: apps installed after a failed one may
/// depend on it, and half-migrating a database is a worse place to be than not
/// starting.
pub async fn migrate_tenant(cfg: &DatabaseConfig, database: &str) -> Result<(), DbError> {
    assert_safe_identifier(database)?;

    for app in apps::APPS {
        migrate_app(cfg, database, app).await?;
    }

    tracing::info!(
        database,
        apps = apps::APPS.len(),
        "tenant migrations applied"
    );
    Ok(())
}

/// Create one app's schema if absent, run its stream, record the result.
async fn migrate_app(
    cfg: &DatabaseConfig,
    database: &str,
    app: &AppMigrations,
) -> Result<(), DbError> {
    // The app id reaches DDL as a schema name. A test in `apps` enforces the
    // same rule at build time; this is the check that runs against the value
    // actually used.
    assert_safe_identifier(app.app_id)?;

    let pool = crate::connect::schema_migration_pool(cfg, database, &app.search_path());

    // One future so the pool is closed on every path, including an early error.
    let result = async {
        create_schema_if_absent(&pool, app.app_id).await?;
        adopt_legacy_bookkeeping(&pool, app.app_id, database).await?;

        app.migrator
            .run(&pool)
            .await
            .map_err(|source| DbError::Migrate {
                target: format!("{database}.{}", app.app_id),
                source,
            })?;

        install_number_sequences(&pool, database, app.app_id).await?;
        sync_permission_tree(&pool, database, app.app_id).await?;

        apps::record_installed(&pool, app.app_id, &app.latest_version()).await
    }
    .await;

    // The pool exists only for this migration; hold no connections afterwards.
    pool.close().await;
    result?;

    tracing::debug!(database, app = app.app_id, "app migrations applied");
    Ok(())
}

/// Move `public._sqlx_migrations` into `core`, on a database that predates the
/// schema move.
///
/// # The bug this exists to stop
///
/// sqlx names its bookkeeping table **unqualified**, and this stream runs on a
/// search path rooted at the app's own schema. So the moment
/// [`create_schema_if_absent`] creates `core`, sqlx's
/// `CREATE TABLE IF NOT EXISTS _sqlx_migrations` lands *there* - `IF NOT
/// EXISTS` looks only at the target schema, and `public._sqlx_migrations`
/// might as well not exist.
///
/// On a database provisioned before migration 0014, that meant sqlx opened a
/// fresh, empty history, concluded that nothing had ever been applied, and
/// re-ran 0001 onwards into `core`. Those migrations are written to be safe
/// against re-running, so they succeeded - building a second, empty copy of
/// every table next to the real one - and then 0014 failed on
/// `relation "users" already exists in schema "core"`, every boot, forever.
///
/// Nothing was lost: the real rows stayed in `public` and the duplicates were
/// empty. But the tenant could not be migrated, and the failure looked like a
/// broken migration rather than a runner that had hidden the history from
/// itself.
///
/// # Why the fix is a move and not a read
///
/// 0014 relocates `_sqlx_migrations` along with everything else, which is
/// right - it is core's table and belongs in core's schema. The only problem
/// is *when*: it has to happen before sqlx looks, not while sqlx is looking.
/// So the runner does it first, and 0014 then finds it already moved.
///
/// Only core is ever adopted. No other app has a history that predates its own
/// schema, and one that appeared to would be a `public` table belonging to
/// somebody else.
async fn adopt_legacy_bookkeeping(
    pool: &sqlx::PgPool,
    app_id: &str,
    database: &str,
) -> Result<(), DbError> {
    if app_id != apps::CORE_APP_ID {
        return Ok(());
    }

    // `to_regclass` answers with NULL rather than raising, which is what makes
    // this a question rather than a `DO` block. The database refuses; it does
    // not act - see `phonix_db`'s own documentation.
    let (in_core, in_public): (bool, bool) = sqlx::query_as(
        "SELECT to_regclass('core._sqlx_migrations') IS NOT NULL,
                to_regclass('public._sqlx_migrations') IS NOT NULL",
    )
    .fetch_one(pool)
    .await
    .map_err(DbError::Query)?;

    // Already moved, or never there. Both are the ordinary case: a database
    // built today has its history in `core` from the first migration, and one
    // built yesterday has had it moved by this function or by 0014.
    if in_core || !in_public {
        return Ok(());
    }

    sqlx::query("ALTER TABLE public._sqlx_migrations SET SCHEMA core")
        .execute(pool)
        .await
        .map_err(DbError::Query)?;

    tracing::info!(
        database,
        "adopted the pre-0014 migration history into the core schema"
    );
    Ok(())
}

/// Write the compiled permission tree into this tenant's static roles.
///
/// # Why a migration pass and not only signup
///
/// `sync_static_roles` was written as "run once per tenant, right after its
/// migrations", and for a long time the only thing that ran it was onboarding.
/// So a workspace got the permissions that existed on the day it was created
/// and never another one. Every release that added a page left every existing
/// administrator unable to reach it, with no error to go looking for - the
/// navigation entry simply was not there, which reads as a feature that was
/// never shipped rather than a grant that was never written.
///
/// That is what happened to Sales and Master: the pages existed, the routes
/// answered, and `Pages.Sales.Invoices` was held by nobody.
///
/// Only the static roles are touched, and only `Admin` is replaced wholesale.
/// A role an organization defined is theirs - see `sync_static_roles`.
///
/// Runs for `core` alone, because permissions are core's table. It is a
/// separate step from the migration stream on purpose: the tree is compiled
/// Rust, so restating it in SQL would be a second source of truth, and the
/// database is not where this project puts logic.
async fn sync_permission_tree(
    pool: &sqlx::PgPool,
    database: &str,
    app_id: &str,
) -> Result<(), DbError> {
    if app_id != apps::CORE_APP_ID {
        return Ok(());
    }

    crate::authorization::sync_static_roles(pool).await?;

    tracing::debug!(database, "static role permissions synchronised");
    Ok(())
}

/// Create the number sequences this app's configuration file declares.
///
/// Part of installing an app, and it runs on every migration pass rather than
/// only the first: an upgrade that adds a document type has to reach the
/// workspaces that already have the app, and `install_from_config` is
/// `ON CONFLICT DO NOTHING`, so re-running it cannot put back a format the
/// tenant changed or reset a counter that has already issued numbers.
///
/// A missing file is not an error - most apps issue no numbered documents, and
/// `core` is one of them. A *malformed* one is: `series_for` validates the mask,
/// the label key and the document type when it reads the file, so a format typo
/// stops a deployment where somebody is watching rather than surfacing on the
/// first invoice, in front of a customer.
async fn install_number_sequences(
    pool: &sqlx::PgPool,
    database: &str,
    app_id: &str,
) -> Result<(), DbError> {
    let series =
        phonix_config::numbering::series_for(app_id).map_err(|err| DbError::CorruptCatalogRow {
            slug: app_id.to_owned(),
            reason: format!("number series configuration is unusable: {err}"),
        })?;

    if series.is_empty() {
        return Ok(());
    }

    let created = crate::numbering::install_from_config(pool, app_id, &series).await?;

    tracing::info!(
        database,
        app = app_id,
        declared = series.len(),
        created,
        "number sequences installed"
    );
    Ok(())
}

/// `CREATE SCHEMA IF NOT EXISTS`, for an app about to be migrated.
///
/// Running this before core's own stream is what makes the `core` schema exist
/// in time for migration 0001 on a fresh database, so a database provisioned
/// today builds straight into `core` and 0014 finds nothing to relocate.
///
/// See the note in `create_database_if_absent` for why the identifier is
/// interpolated and why that is safe.
async fn create_schema_if_absent(pool: &sqlx::PgPool, schema: &str) -> Result<(), DbError> {
    assert_safe_identifier(schema)?;

    let sql = format!(r#"CREATE SCHEMA IF NOT EXISTS "{schema}""#);
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(pool)
        .await
        .map_err(DbError::Query)?;

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
