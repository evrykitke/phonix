//! Tenancy: the catalog, the pool registry, and provisioning.
//!
//! The routing layer of a database-per-tenant deployment.
//!
//! * [`apps`]     - the apps compiled in, and the schema each one owns.
//! * [`catalog`] - the shared registry, one row per tenant.
//! * [`registry`] - slug to live `PgPool`, with idle eviction.
//! * [`provision`] - creating and migrating a tenant's own database.

pub mod apps;
pub mod catalog;
pub mod provision;
pub mod registry;

pub use apps::{APPS, AppMigrations, CORE_APP_ID, schema_fingerprint};
pub use catalog::{Catalog, TenantRecord};
pub use provision::{
    MigrationSweep, drop_tenant_database, migrate_outdated_tenants, migrate_tenant,
    provision_tenant,
};
pub use registry::{TenantHandle, TenantRegistry};
