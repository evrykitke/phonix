//! Tenant identity: who a request belongs to.
//!
//! A [`TenantSlug`] is resolved from the request host, looked up in the shared
//! catalog, and mapped to a dedicated Postgres database. [`settings`] is what
//! that organization has decided for itself once it exists.

pub mod settings;
pub mod slug;
pub mod status;

pub use settings::WorkspaceSecuritySettings;
pub use slug::{InvalidTenantSlug, TenantSlug};
pub use status::{TenantId, TenantStatus, TenantSummary};
