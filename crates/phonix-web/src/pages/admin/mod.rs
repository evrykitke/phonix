//! Administration screens.
//!
//! Each one is reachable from `navigation::tree`, gated there on the same
//! permission the server function behind it states. The menu entry is the
//! courtesy; the check in `phonix_services` is the control.

pub mod apps;
pub mod audit_event;
pub mod audit_logs;
pub mod currencies;
pub mod entity_change;
pub mod mail_settings;
pub mod numbering;
pub mod organization;
pub mod roles;
pub mod settings;
pub mod user_edit;
pub mod user_invite;
pub mod user_permissions;
pub mod users;
