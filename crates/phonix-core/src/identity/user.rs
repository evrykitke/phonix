//! The user account, as the rest of the application sees it.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::authorization::PermissionSet;
use crate::{Message, msg};

/// Stable primary key of a user inside one tenant's database.
///
/// Only unique *within* a tenant: two tenants may hold the same id without
/// conflict, because they are different databases.
pub type UserId = Uuid;

/// Lifecycle state of a user account, mirrored from `users.status`.
///
/// Separate from "is this session valid": a suspended user's existing sessions
/// are revoked, but the row and its history stay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserStatus {
    /// Invited but has not set a password or verified their email yet.
    Pending,
    /// Normal.
    Active,
    /// Blocked by an administrator. Reversible.
    Suspended,
    /// Left the organization. Kept for referential integrity and audit trails.
    Deactivated,
}

impl UserStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Deactivated => "deactivated",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "pending" => Some(Self::Pending),
            "active" => Some(Self::Active),
            "suspended" => Some(Self::Suspended),
            "deactivated" => Some(Self::Deactivated),
            _ => None,
        }
    }

    /// Whether this account may hold a session.
    pub fn can_sign_in(self) -> bool {
        matches!(self, Self::Active)
    }

    pub fn label(self) -> Message {
        match self {
            Self::Pending => msg!("user.status.pending"),
            Self::Active => msg!("user.status.active"),
            Self::Suspended => msg!("user.status.suspended"),
            Self::Deactivated => msg!("user.status.deactivated"),
        }
    }
}

impl std::fmt::Display for UserStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The signed-in user, as the UI sees them.
///
/// A deliberately narrow projection of the `users` row: no password hash, no
/// MFA secret, no lockout counters. This type is serialised to the browser on
/// every page, so anything added here becomes visible to that user.
///
/// `permissions` is the *resolved* set - their roles' grants plus their
/// individual overrides, already flattened. It is here so the UI can hide what
/// the user cannot do; it is never the enforcement point. Every server function
/// re-resolves permissions from the database before acting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthUser {
    pub id: UserId,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub display_name: String,

    /// Role names held by this user, e.g. `["Admin"]`. For display; authority
    /// comes from `permissions`.
    pub roles: Vec<String>,
    pub permissions: PermissionSet,

    /// Created the workspace. Cannot be deleted, suspended, or stripped of the
    /// `Admin` role by anyone - including themselves - because that is how a
    /// workspace ends up with nobody able to administer it.
    pub is_owner: bool,

    pub status: UserStatus,
    /// Whether the account has at least one confirmed second factor.
    pub mfa_enabled: bool,
    /// Whether this *session* has satisfied MFA. False while a login is
    /// half-finished: authenticated by password, awaiting the second factor.
    pub mfa_satisfied: bool,
    pub email_verified: bool,
}

impl AuthUser {
    /// Initials for an avatar placeholder, e.g. "AL" for Ada Lovelace.
    pub fn initials(&self) -> String {
        let first = self.first_name.chars().next();
        let last = self.last_name.chars().next();

        match (first, last) {
            (Some(f), Some(l)) => format!("{f}{l}").to_uppercase(),
            (Some(f), None) => f.to_uppercase().to_string(),
            _ => self
                .email
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "?".to_owned()),
        }
    }

    /// Whether the user has completed every step required to use the app.
    ///
    /// A session that has not satisfied MFA may reach the challenge page and
    /// nothing else.
    pub fn is_fully_authenticated(&self) -> bool {
        self.status.can_sign_in() && (!self.mfa_enabled || self.mfa_satisfied)
    }

    /// Convenience wrapper so views can write `user.can(names::USERS_CREATE)`.
    ///
    /// A half-authenticated session holds nothing: until the second factor is
    /// satisfied the account is not really signed in, and a UI that renders as
    /// though it were is one missing server-side check away from a real bypass.
    pub fn can(&self, permission: &str) -> bool {
        self.is_fully_authenticated() && self.permissions.is_granted(permission)
    }

    pub fn can_any(&self, permissions: &[&str]) -> bool {
        self.is_fully_authenticated() && self.permissions.is_granted_any(permissions)
    }

    pub fn has_role(&self, role: &str) -> bool {
        self.roles
            .iter()
            .any(|held| held.eq_ignore_ascii_case(role))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authorization::names;

    fn user() -> AuthUser {
        AuthUser {
            id: Uuid::nil(),
            email: "ada@example.com".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            display_name: "Ada Lovelace".into(),
            roles: vec!["Admin".into()],
            permissions: PermissionSet::all(),
            is_owner: true,
            status: UserStatus::Active,
            mfa_enabled: false,
            mfa_satisfied: false,
            email_verified: true,
        }
    }

    #[test]
    fn statuses_round_trip_through_their_stored_form() {
        for status in [
            UserStatus::Pending,
            UserStatus::Active,
            UserStatus::Suspended,
            UserStatus::Deactivated,
        ] {
            assert_eq!(UserStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(UserStatus::parse("banned"), None);
        assert!(UserStatus::Active.can_sign_in());
        assert!(!UserStatus::Suspended.can_sign_in());
    }

    #[test]
    fn initials_fall_back_to_the_email() {
        let mut u = user();
        assert_eq!(u.initials(), "AL");

        u.first_name = String::new();
        u.last_name = String::new();
        assert_eq!(u.initials(), "A");
    }

    #[test]
    fn mfa_gates_full_authentication() {
        let mut u = user();
        assert!(u.is_fully_authenticated());

        // Once a second factor exists, a password-only session is not enough.
        u.mfa_enabled = true;
        assert!(!u.is_fully_authenticated());
        u.mfa_satisfied = true;
        assert!(u.is_fully_authenticated());

        u.status = UserStatus::Suspended;
        assert!(!u.is_fully_authenticated());
    }

    #[test]
    fn a_half_authenticated_session_holds_no_permissions() {
        let mut u = user();
        assert!(u.can(names::USERS_DELETE));

        // Password accepted, second factor outstanding: the UI must render as
        // though this user can do nothing.
        u.mfa_enabled = true;
        u.mfa_satisfied = false;
        assert!(!u.can(names::USERS_DELETE));
        assert!(!u.can_any(&[names::DASHBOARD, names::USERS]));
        // The underlying set is untouched - only the gate closed.
        assert!(u.permissions.is_granted(names::USERS_DELETE));
    }

    #[test]
    fn roles_are_matched_case_insensitively() {
        let u = user();
        assert!(u.has_role("Admin"));
        assert!(u.has_role("admin"));
        assert!(!u.has_role("User"));
    }
}
