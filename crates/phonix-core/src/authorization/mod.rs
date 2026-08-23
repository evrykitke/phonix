//! Authorization: permissions and roles, in the style of ABP / ASP.NET Zero.
//!
//! Two halves that must not be confused:
//!
//! * **Definitions** ([`definitions`]) - the permissions the software *has*. A
//!   dotted tree (`Pages.Administration.Users.Create`), declared in code,
//!   identical for every tenant, compiled into both the server and the browser
//!   bundle. Adding one is a code change, because the code is what enforces it.
//!
//! * **Grants** ([`PermissionSet`], [`role`]) - which of those a given role or
//!   user *holds*. Rows in a tenant's own database, editable at runtime,
//!   different per workspace.
//!
//! Kept separate from [`crate::identity`] because "who you are" and "what you
//! may do" are different questions. Conflating them is how `if user.role ==
//! Admin` ends up scattered through a codebase, at which point adding a role is
//! a code change in fifty places instead of a row in a table.
//!
//! # Resolving what a user holds
//!
//! ```text
//! union of the user's roles' grants
//!   .. plus  individual grants   (user_permissions where is_granted = true)
//!   .. minus individual denials  (user_permissions where is_granted = false)
//! ```
//!
//! An individual denial wins over any role grant, so one person can be excluded
//! from something their role allows without inventing a near-duplicate role.

pub mod definitions;
pub mod grants;
pub mod permission_set;
pub mod role;

pub use definitions::{
    DEFINITIONS, PermissionDefinition, ancestors, children, definition, is_defined,
    is_descendant_of, is_valid_name, names,
};
pub use grants::{GrantSource, PermissionOverrides, UserPermissionView};
pub use permission_set::PermissionSet;
pub use role::{
    MAX_ROLE_DESCRIPTION_LEN, MAX_ROLE_NAME_LEN, PermissionDenied, RoleDetail, RoleInput,
    RoleSummary, ValidRole, validate_role_name,
};

/// The names of the built-in roles: `roles::ADMIN`, `roles::USER`.
pub use role::names as roles;
