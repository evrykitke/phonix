//! PostgreSQL access for a database-per-tenant deployment.
//!
//! Two kinds of database:
//!
//! * **catalog** - one shared database (`phonix_catalog`) holding the tenant
//!   registry. One long-lived pool, created at startup.
//! * **tenant**  - one database per tenant (`phonix_tenant_<slug>`). Pools are
//!   created lazily on first request and evicted when the tenant goes idle, so
//!   a thousand registered tenants do not mean a thousand open pools.
//!
//! Queries use the runtime `sqlx::query*` functions rather than the compile-time
//! `query!` macros. The macros would require a reachable database (and one
//! chosen tenant's schema) at build time, which is the wrong trade for a
//! multi-database application and for CI.
//!
//! # Layout
//!
//! | Module            | Scope                                                |
//! | ----------------- | ---------------------------------------------------- |
//! | [`connect`]       | Pools and connections, catalog and tenant            |
//! | [`tenancy`]       | The catalog, the pool registry, provisioning         |
//! | [`identity`]      | `users`, `sessions`, `user_tokens`, MFA factors, audit |
//! | [`audit`]         | `entity_events` - one row per change to one record   |
//! | [`authorization`] | `roles`, `role_permissions`, `user_permissions`      |
//! | [`settings`]      | `workspace_settings`                                 |
//! | [`files`]         | `file_uploads` - and the queue the upload jobs run on |
//! | [`outbox`]        | `outbox_events` - events written with the change they describe |
//!
//! `tenancy` works against the catalog; everything else works against exactly
//! one tenant database and carries no tenant column - isolation is the database
//! boundary, so a query that reaches the wrong workspace is a routing bug rather
//! than a missing `WHERE` clause.
//!
//! # What belongs here, and what does not
//!
//! This is the **data access layer**. A function here reads or writes rows and
//! nothing else. In particular it holds no use cases: "sign in" is not a query,
//! it is a sequence of them with decisions in between, and it lives in
//! `phonix-services` along with the hashing, the TOTP arithmetic and the
//! cipher.
//!
//! One rule follows from that and is worth stating plainly: **no repository in
//! this crate ever receives a credential in a form it could use.** Passwords
//! arrive as PHC strings, session and one-time tokens as digests, TOTP secrets
//! as sealed bytes. A dump of what this layer can see is a dump of things that
//! cannot be presented anywhere.
//!
//! ```text
//!   phonix-services   use cases: sign_in, onboard_workspace, enrol_totp
//!         |           hashes, seals, digests, decides
//!         v
//!   phonix-db         repositories: rows in, rows out          <- you are here
//!         |
//!         v
//!   PostgreSQL        one catalog database, one database per tenant
//! ```

pub mod audit;
pub mod authorization;
pub mod connect;
pub mod error;
pub mod files;
pub mod identity;
pub mod mail;
pub mod organization;
pub mod outbox;
pub mod settings;
pub mod tenancy;

pub use connect::{catalog_pool, maintenance_connection, tenant_pool};
pub use error::DbError;
pub use tenancy::{Catalog, TenantHandle, TenantRecord, TenantRegistry};

pub use sqlx::PgPool;
pub use sqlx::postgres::PgPoolOptions;

// Re-exported so dependent crates can write queries without taking their own
// direct sqlx dependency, which would risk a version mismatch.
pub use sqlx;

/// Migrations for the shared catalog database.
pub static CATALOG_MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/catalog");

/// Migrations applied to every tenant database.
pub static TENANT_MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/tenant");
