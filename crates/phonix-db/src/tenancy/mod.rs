//! Tenancy: the catalog, the pool registry, and provisioning.
//!
//! The routing layer of a database-per-tenant deployment.
//!
//! * [`apps`]      - the apps compiled in, and the schema each one owns.
//! * [`installs`]  - which of them this workspace has switched on.
//! * [`catalog`]   - the shared registry, one row per tenant.
//! * [`licence`]   - whether a tenant is authorized to be here, and until when.
//! * [`registry`]  - slug to live `PgPool`, with idle eviction.
//! * [`provision`] - creating and migrating a tenant's own database.

pub mod apps;
pub mod catalog;
pub mod installs;
pub mod licence;
pub mod provision;
pub mod registry;

pub use apps::{APPS, AppMigrations, CORE_APP_ID, schema_fingerprint};
pub use catalog::{Catalog, TenantRecord};
pub use installs::AppInstall;
pub use licence::LicenceInput;
pub use provision::{
    MigrationSweep, drop_tenant_database, migrate_outdated_tenants, migrate_tenant,
    provision_tenant,
};
pub use registry::{TenantHandle, TenantRegistry};
