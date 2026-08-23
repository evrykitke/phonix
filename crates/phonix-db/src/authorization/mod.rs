//! Authorization storage: roles, role grants and per-user overrides.
//!
//! The permission *definitions* live in `phonix_core::authorization` and are
//! compiled in. This module only stores and resolves who holds what.

pub mod permission;
pub mod role;

pub use permission::{
    UserOverrides, clear_all_overrides, clear_override, is_granted, overrides_for_user,
    resolve_for_user, set_override,
};
pub use role::{RoleRecord, sync_static_roles};
