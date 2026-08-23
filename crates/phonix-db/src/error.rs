//! Database errors.
//!
//! These stay server-side. Anything crossing to the browser is first mapped to
//! [`phonix_core::Error`], which deliberately carries no connection details.

use phonix_core::Error as CoreError;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("could not connect to {target}: {source}")]
    Connect {
        target: String,
        #[source]
        source: sqlx::Error,
    },

    #[error("query failed: {0}")]
    Query(#[from] sqlx::Error),

    #[error("migration of {target} failed: {source}")]
    Migrate {
        target: String,
        #[source]
        source: sqlx::migrate::MigrateError,
    },

    #[error("tenant '{0}' is not registered in the catalog")]
    UnknownTenant(String),

    #[error("tenant '{slug}' is {status}, not active")]
    TenantInactive { slug: String, status: String },

    #[error("tenant '{0}' already exists")]
    TenantExists(String),

    #[error("catalog row for tenant '{slug}' is invalid: {reason}")]
    CorruptCatalogRow { slug: String, reason: String },

    // --- identity -------------------------------------------------------
    /// The unique index on `lower(email)` refused the insert.
    ///
    /// Separate from a bare `Query` because the caller shows it on a form: it
    /// is the one write failure that is the user's to fix.
    #[error("an account already uses '{0}'")]
    UserExists(String),

    /// A write that would have removed the workspace owner's own access.
    ///
    /// The owner is the account that can always reach the workspace. Suspending
    /// or deleting it, or stripping its Admin role, would leave a workspace
    /// nobody can administer, so those statements carry `AND NOT is_owner` and
    /// report this when they match nothing.
    #[error("the workspace owner cannot be suspended, deleted or demoted")]
    OwnerProtected,

    /// A stored row this build cannot interpret - an enum column holding a
    /// value no variant matches, usually a migration that did not run.
    ///
    /// Distinct from [`Self::Query`] because the query succeeded: the row is
    /// there and it is wrong, which is an operational problem rather than a
    /// transient one.
    #[error("unusable row: {0}")]
    CorruptRow(String),

    /// A write the CHECK constraints refused. The application layer validates
    /// first and reports per field; this is the backstop for anything that
    /// reached the database anyway.
    #[error("invalid workspace policy: {0}")]
    InvalidPolicy(String),

    // --- authorization --------------------------------------------------
    #[error("a role named '{0}' already exists")]
    RoleExists(String),

    /// Admin and User are defined by the application, not by the organization.
    #[error("the Admin and User roles cannot be renamed or deleted")]
    StaticRoleProtected,

    /// A static role is missing from a tenant database that should have been
    /// seeded with it - a broken migration rather than anything the caller did.
    #[error("the '{0}' role is missing from this workspace")]
    MissingStaticRole(String),

    /// A grant naming a permission this build does not define. Refused on the
    /// way in so `role_permissions` cannot accumulate names nothing checks.
    #[error("'{0}' is not a permission this build defines")]
    UnknownPermission(String),

    // --- outbox ---------------------------------------------------------
    /// An event payload that would not serialise.
    ///
    /// A programming error rather than a storage failure - but it happens
    /// inside a transaction that is about to commit real work, so it has to be
    /// an error the caller can return rather than a panic that poisons it.
    #[error("could not serialise an event payload: {0}")]
    Serialization(String),
}

impl From<DbError> for CoreError {
    /// Collapse database failures into the coarse, safe error the client sees.
    ///
    /// Everything that is not a tenant-routing problem becomes `Unavailable`
    /// with a fixed label, so a SQL string or host name can never reach a
    /// browser through this path.
    fn from(err: DbError) -> Self {
        match err {
            DbError::UnknownTenant(slug) => CoreError::UnknownTenant(slug),
            DbError::TenantInactive { slug, .. } => CoreError::TenantInactive(slug),
            DbError::TenantExists(slug) => {
                CoreError::Conflict(format!("tenant '{slug}' already exists"))
            }
            DbError::UserExists(email) => {
                CoreError::Conflict(format!("an account already uses '{email}'"))
            }
            DbError::RoleExists(name) => {
                CoreError::Conflict(format!("a role named '{name}' already exists"))
            }
            // Refusals the caller could have avoided, so they say what was
            // refused. None of them names a table, a column or a host.
            DbError::OwnerProtected | DbError::StaticRoleProtected => CoreError::Forbidden,
            DbError::UnknownPermission(name) => {
                CoreError::Validation(format!("unknown permission '{name}'"))
            }
            DbError::InvalidPolicy(detail) => CoreError::Validation(detail),
            other => {
                // The detail is logged here and dropped from the returned value.
                tracing::error!(error = %other, "database error");
                CoreError::Unavailable("database".to_owned())
            }
        }
    }
}
