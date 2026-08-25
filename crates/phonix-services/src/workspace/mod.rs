//! Workspace use cases: creating one, and changing what it requires.
//!
//! [`onboarding`] is the only place in the application that spans the catalog
//! and a tenant database. [`settings`] is how an organization tightens its own
//! password and MFA policy afterwards, and [`profile`] is who it says it is -
//! the legal entity, its address, and the currency and time zone it works in.

pub mod apps;
pub mod onboarding;
pub mod profile;
pub mod settings;

pub use onboarding::{OnboardedWorkspace, onboard_workspace};
