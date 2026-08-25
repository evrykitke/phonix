//! The tenant registry, stored in the shared catalog database.

use chrono::{DateTime, Utc};
use phonix_core::{TenantId, TenantSlug, TenantStatus, TenantSummary};
use sqlx::{FromRow, PgPool, Row};

use crate::error::DbError;

/// How a tenant came into existence. Stored for support and for metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantOrigin {
    /// The public onboarding wizard.
    Signup,
    /// Created by an operator or a CLI.
    Admin,
    /// Conjured by an unrecognised Host header. Development only.
    AutoProvision,
    /// A seeding script or a test fixture.
    Seed,
}

impl TenantOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Signup => "signup",
            Self::Admin => "admin",
            Self::AutoProvision => "auto_provision",
            Self::Seed => "seed",
        }
    }
}

/// Everything needed to register a tenant, before its database exists.
#[derive(Debug, Clone)]
pub struct NewTenant<'a> {
    pub slug: &'a TenantSlug,
    pub display_name: &'a str,
    pub database_name: &'a str,
    pub origin: TenantOrigin,
    /// The address of whoever created it. Registry-level support information,
    /// not an account - the account lives in the tenant's own database.
    pub owner_email: Option<&'a str>,
}

/// One row of `catalog.tenants`.
#[derive(Debug, Clone)]
pub struct TenantRecord {
    pub id: TenantId,
    pub slug: TenantSlug,
    pub display_name: String,
    pub database_name: String,
    pub status: TenantStatus,
    pub schema_version: Option<String>,
    pub owner_email: Option<String>,
    pub onboarded_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl TenantRecord {
    pub fn summary(&self) -> TenantSummary {
        TenantSummary {
            id: self.id,
            slug: self.slug.clone(),
            display_name: self.display_name.clone(),
            status: self.status,
        }
    }
}

// `status` and `slug` are TEXT in Postgres but validated types in Rust, so the
// conversion happens here rather than by deriving FromRow on the raw columns.
impl<'r> FromRow<'r, sqlx::postgres::PgRow> for TenantRecord {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let raw_slug: String = row.try_get("slug")?;
        let raw_status: String = row.try_get("status")?;

        let slug = TenantSlug::parse(&raw_slug).map_err(|err| sqlx::Error::ColumnDecode {
            index: "slug".to_owned(),
            source: Box::new(err),
        })?;

        let status = TenantStatus::parse(&raw_status).ok_or_else(|| sqlx::Error::ColumnDecode {
            index: "status".to_owned(),
            source: format!("unrecognised tenant status '{raw_status}'").into(),
        })?;

        Ok(Self {
            id: row.try_get("id")?,
            slug,
            display_name: row.try_get("display_name")?,
            database_name: row.try_get("database_name")?,
            status,
            schema_version: row.try_get("schema_version")?,
            owner_email: row.try_get("owner_email")?,
            onboarded_at: row.try_get("onboarded_at")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

// sqlx 0.9 only accepts `&'static str` as SQL unless the string is explicitly
// asserted safe, so these are written out as literals rather than assembled
// from a shared column constant at runtime.
const SELECT_BY_SLUG: &str = "SELECT id, slug, display_name, database_name, status, \
     schema_version, owner_email, onboarded_at, created_at FROM tenants WHERE slug = $1";

const SELECT_ALL: &str = "SELECT id, slug, display_name, database_name, status, \
     schema_version, owner_email, onboarded_at, created_at FROM tenants ORDER BY slug";

const INSERT_TENANT: &str = "INSERT INTO tenants \
     (slug, display_name, database_name, status, created_via, owner_email) \
     VALUES ($1, $2, $3, 'provisioning', $4, $5) \
     RETURNING id, slug, display_name, database_name, status, schema_version, \
     owner_email, onboarded_at, created_at";

/// Case-insensitive, because addresses are stored lowercased but a caller may
/// not have normalised theirs.
const SELECT_BY_OWNER_EMAIL: &str = "SELECT id, slug, display_name, database_name, status, \
     schema_version, owner_email, onboarded_at, created_at FROM tenants \
     WHERE lower(owner_email) = lower($1) ORDER BY created_at";

/// Read access to the tenant registry.
#[derive(Clone)]
pub struct Catalog {
    pool: PgPool,
}

impl Catalog {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Apply the catalog migrations.
    pub async fn migrate(&self) -> Result<(), DbError> {
        crate::CATALOG_MIGRATIONS
            .run(&self.pool)
            .await
            .map_err(|source| DbError::Migrate {
                target: "catalog".to_owned(),
                source,
            })?;
        tracing::info!("catalog migrations applied");
        Ok(())
    }

    /// Look up a tenant by slug. `Ok(None)` means "no such tenant", which is a
    /// routine outcome for a junk Host header, not an error.
    pub async fn find_by_slug(&self, slug: &TenantSlug) -> Result<Option<TenantRecord>, DbError> {
        sqlx::query_as::<_, TenantRecord>(SELECT_BY_SLUG)
            .bind(slug.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(DbError::Query)
    }

    /// Look up a tenant that is allowed to serve traffic.
    pub async fn find_active(&self, slug: &TenantSlug) -> Result<TenantRecord, DbError> {
        let record = self
            .find_by_slug(slug)
            .await?
            .ok_or_else(|| DbError::UnknownTenant(slug.to_string()))?;

        if !record.status.serves_traffic() {
            return Err(DbError::TenantInactive {
                slug: slug.to_string(),
                status: format!("{:?}", record.status).to_lowercase(),
            });
        }

        Ok(record)
    }

    pub async fn list(&self) -> Result<Vec<TenantRecord>, DbError> {
        sqlx::query_as::<_, TenantRecord>(SELECT_ALL)
            .fetch_all(&self.pool)
            .await
            .map_err(DbError::Query)
    }

    /// Insert a tenant in `provisioning` state.
    ///
    /// The row is created before the database exists so that two concurrent
    /// requests for a new tenant cannot both start provisioning: the unique
    /// index on `slug` makes the second one fail here.
    pub async fn insert(&self, new: NewTenant<'_>) -> Result<TenantRecord, DbError> {
        sqlx::query_as::<_, TenantRecord>(INSERT_TENANT)
            .bind(new.slug.as_str())
            .bind(new.display_name)
            .bind(new.database_name)
            .bind(new.origin.as_str())
            .bind(new.owner_email)
            .fetch_one(&self.pool)
            .await
            .map_err(|err| match &err {
                // 23505 = unique_violation
                sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
                    DbError::TenantExists(new.slug.to_string())
                }
                _ => DbError::Query(err),
            })
    }

    /// Whether a slug is free.
    ///
    /// Answers the availability check on the signup form. Deliberately reports
    /// only "taken", never *which* status a taken slug is in - that would let
    /// an anonymous caller enumerate suspended and archived workspaces.
    pub async fn slug_is_available(&self, slug: &TenantSlug) -> Result<bool, DbError> {
        let taken: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM tenants WHERE slug = $1")
            .bind(slug.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(DbError::Query)?;

        Ok(taken.is_none())
    }

    /// Every workspace registered to an address. Backs a future "find my
    /// workspace" email.
    pub async fn find_by_owner_email(&self, email: &str) -> Result<Vec<TenantRecord>, DbError> {
        sqlx::query_as::<_, TenantRecord>(SELECT_BY_OWNER_EMAIL)
            .bind(email)
            .fetch_all(&self.pool)
            .await
            .map_err(DbError::Query)
    }

    /// Stamp the moment self-service onboarding finished.
    pub async fn mark_onboarded(&self, slug: &TenantSlug) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE tenants
                SET onboarded_at = now(), updated_at = now()
              WHERE slug = $1 AND onboarded_at IS NULL",
        )
        .bind(slug.as_str())
        .execute(&self.pool)
        .await
        .map_err(DbError::Query)?;
        Ok(())
    }

    /// Mark a tenant active and record the schema version it was migrated to.
    pub async fn mark_active(
        &self,
        slug: &TenantSlug,
        schema_version: &str,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE tenants
                SET status = 'active', schema_version = $2, migrated_at = now(),
                    updated_at = now()
              WHERE slug = $1",
        )
        .bind(slug.as_str())
        .bind(schema_version)
        .execute(&self.pool)
        .await
        .map_err(DbError::Query)?;

        Ok(())
    }

    pub async fn set_status(&self, slug: &TenantSlug, status: TenantStatus) -> Result<(), DbError> {
        sqlx::query("UPDATE tenants SET status = $2, updated_at = now() WHERE slug = $1")
            .bind(slug.as_str())
            .bind(status.as_str())
            .execute(&self.pool)
            .await
            .map_err(DbError::Query)?;

        Ok(())
    }
}
