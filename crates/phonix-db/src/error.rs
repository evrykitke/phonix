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

    /// The workspace has no current licence.
    ///
    /// Deliberately not [`Self::TenantInactive`], even though both answer 403.
    /// A suspension is somebody's decision with their name against it; a lapse
    /// is a date passing. Collapsing the two would mean a customer whose trial
    /// ran out and a customer we stopped read the same sentence, and would
    /// start the wrong conversation. See ADR 0005 section 7.
    #[error("tenant '{slug}' is not licensed: {reason}")]
    TenantUnlicensed {
        slug: String,
        /// One word for the log line and the pill: `expired`, `revoked`,
        /// `unlicensed`, `not yet started`.
        standing: String,
        /// The sentence the request is refused with.
        reason: String,
    },

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

    // --- numbering ------------------------------------------------------
    /// A document asked for a number from a sequence that is missing or
    /// switched off.
    ///
    /// A refusal rather than a number invented on the spot. A document that
    /// numbers itself outside the sequence is exactly the gap the sequence
    /// exists to prevent, and one that cannot be explained afterwards.
    #[error("no active number sequence for {app_id}.{doc_type} (scope '{scope_key}')")]
    UnusableSequence {
        app_id: String,
        doc_type: String,
        scope_key: String,
    },

    // --- master ---------------------------------------------------------
    /// Two rates for one tax code would have been live at the same time.
    ///
    /// Raised by the exclusion constraint in `master.tax_rates`, mapped here
    /// rather than checked for first: a check-then-insert is a race, and the
    /// race is two administrators filing the same rate change on the same
    /// afternoon. An expected path through a form, so it is a named variant a
    /// screen can render rather than a Postgres string.
    #[error("a rate for that tax already covers part of that period")]
    TaxRateOverlap,

    /// A name a workspace's own code or key already uses.
    ///
    /// The unique indexes on `lower(code)` refused the write. Separate from a
    /// bare `Query` for the reason [`Self::UserExists`] is: it is the one write
    /// failure that is the person's to fix, on the field they typed it in.
    #[error("a {entity} with the code '{code}' already exists")]
    CodeExists { entity: &'static str, code: String },

    // --- books ----------------------------------------------------------
    /// A write that would have changed a document which is no longer a draft.
    ///
    /// The statement carries `WHERE status = 'draft'` and reports this when it
    /// matches nothing, so the rule is true of the database rather than only of
    /// the service above it. An invoice that can be edited after it has been
    /// sent is not evidence of anything.
    #[error("a posted invoice cannot be edited")]
    InvoiceNotEditable,

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
            // Both are 403 to the browser. The difference between them is for
            // the log, the audit trail and the sentence nginx's upstream puts
            // on the page - not for the status code.
            DbError::TenantUnlicensed { slug, .. } => CoreError::TenantInactive(slug),
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
            // A refusal the caller could have avoided, and one worth naming:
            // "that invoice has been posted" is an answer, "forbidden" is not.
            DbError::InvoiceNotEditable => {
                CoreError::Conflict("a posted invoice cannot be edited".to_owned())
            }
            DbError::TaxRateOverlap => CoreError::Conflict(
                "a rate for that tax already covers part of that period".to_owned(),
            ),
            DbError::CodeExists { entity, code } => {
                CoreError::Conflict(format!("a {entity} with the code '{code}' already exists"))
            }
            other => {
                // The detail is logged here and dropped from the returned value.
                tracing::error!(error = %other, "database error");
                CoreError::Unavailable("database".to_owned())
            }
        }
    }
}
