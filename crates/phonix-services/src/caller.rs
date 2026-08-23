//! Who is asking, and whether they may.
//!
//! Every use case that changes something takes a [`Caller`] and states its
//! permission on the first line:
//!
//! ```ignore
//! pub async fn create_requisition(pool: &PgPool, caller: &Caller, ..) -> ServiceResult<..> {
//!     caller.require(permissions::INVENTORY_REQUISITION_CREATE)?;
//!     ..
//! }
//! ```
//!
//! # Why here and not in the router
//!
//! A route guard protects a URL. A use case is reachable from a server
//! function, another use case, a background job and a future API - and every
//! one of those is a separate place to forget. Checking where the work happens
//! means the check cannot be routed around, and it puts the required permission
//! next to the code that needs it rather than in a table of paths somewhere
//! else.
//!
//! The UI still hides what a user cannot do. That is a courtesy, not a control:
//! `AuthUser::can` decides what to render, this decides what may happen.
//!
//! # Half-authenticated callers hold nothing
//!
//! A session that has not cleared its second factor, or belongs to a suspended
//! account, fails every check here - because [`phonix_core::AuthUser::can`]
//! returns false until `is_fully_authenticated`. That is what makes
//! [`crate::identity::authentication`]'s "yes, but" outcomes safe: the session
//! exists so the challenge screen has something to attach to, and it can reach
//! nothing else.

use phonix_core::identity::{AuthUser, UserId};
use phonix_core::{PermissionDenied, PermissionSet};

use crate::error::{ServiceError, ServiceResult};

/// The authenticated party behind a use case.
#[derive(Debug, Clone)]
pub enum Caller {
    /// A signed-in person.
    User(Box<AuthUser>),
    /// The application itself: onboarding before an owner exists, a scheduled
    /// sweep, a migration.
    ///
    /// Passes every check, which is exactly why it is a named variant rather
    /// than an `Option<AuthUser>` that is `None`. `Caller::system("nightly
    /// session purge")` is visible in review; a missing check is not.
    System { reason: &'static str },
}

impl Caller {
    pub fn user(auth_user: AuthUser) -> Self {
        Self::User(Box::new(auth_user))
    }

    /// An internal caller. The reason is recorded in the audit trail.
    pub fn system(reason: &'static str) -> Self {
        Self::System { reason }
    }

    /// The signed-in user, if this is one.
    pub fn auth_user(&self) -> Option<&AuthUser> {
        match self {
            Self::User(user) => Some(user),
            Self::System { .. } => None,
        }
    }

    /// The acting user's id, for `granted_by`, `updated_by` and audit rows.
    pub fn user_id(&self) -> Option<UserId> {
        self.auth_user().map(|user| user.id)
    }

    /// Whether this caller holds a permission. Does not fail; see
    /// [`Self::require`] for the gate.
    pub fn can(&self, permission: &str) -> bool {
        match self {
            Self::User(user) => user.can(permission),
            Self::System { .. } => true,
        }
    }

    /// Refuse unless the caller holds `permission`.
    ///
    /// The error names what was required, so the log says which permission is
    /// missing rather than "forbidden". The *browser* is told only "forbidden";
    /// see `ServiceError`'s conversion.
    pub fn require(&self, permission: &str) -> ServiceResult<()> {
        debug_assert!(
            phonix_core::authorization::is_defined(permission),
            "'{permission}' is not in the compiled permission tree; a typo here \
             would refuse everybody or, worse, be spelled the same way in both \
             the check and the grant and let everybody through"
        );

        if self.can(permission) {
            Ok(())
        } else {
            Err(ServiceError::Forbidden(PermissionDenied::new(permission)))
        }
    }

    /// Refuse unless the caller holds every one of `permissions`.
    ///
    /// For an operation that genuinely does two things - creating a user *and*
    /// assigning their roles, say - so that holding half of it is not enough.
    pub fn require_all(&self, permissions: &[&str]) -> ServiceResult<()> {
        for permission in permissions {
            self.require(permission)?;
        }
        Ok(())
    }

    /// Refuse unless the caller holds at least one of `permissions`.
    pub fn require_any(&self, permissions: &[&str]) -> ServiceResult<()> {
        if permissions.iter().any(|permission| self.can(permission)) {
            return Ok(());
        }

        Err(ServiceError::Forbidden(PermissionDenied::new(
            permissions.first().copied().unwrap_or("unknown"),
        )))
    }

    /// Allow when the caller is acting on their own account, or holds
    /// `permission` for acting on someone else's.
    ///
    /// The shape of nearly every "user" operation: change *your* password
    /// freely, change *someone else's* only with `Users.Edit`. Without this the
    /// two would be separate use cases doing the same thing.
    pub fn require_self_or(&self, subject: UserId, permission: &str) -> ServiceResult<()> {
        if self.user_id() == Some(subject) {
            // Still refuse a half-authenticated session: it has proven a
            // password and nothing more.
            return match self {
                Self::User(user) if !user.is_fully_authenticated() => {
                    Err(ServiceError::Forbidden(PermissionDenied::new(permission)))
                }
                _ => Ok(()),
            };
        }

        self.require(permission)
    }

    /// Everything this caller may do, for building a UI.
    pub fn permissions(&self) -> PermissionSet {
        match self {
            Self::User(user) if user.is_fully_authenticated() => user.permissions.clone(),
            // A half-authenticated user gets an empty set rather than their
            // real one, so a view built from this cannot render a menu the
            // server would refuse.
            Self::User(_) => PermissionSet::new(),
            Self::System { .. } => PermissionSet::all(),
        }
    }

    /// Whether this is the workspace owner.
    ///
    /// Not a permission: ownership is about the one account that must always be
    /// able to administer the workspace, which is enforced in SQL rather than
    /// granted.
    pub fn is_owner(&self) -> bool {
        self.auth_user().is_some_and(|user| user.is_owner)
    }
}

/// Require a signed-in person, not the system.
///
/// For use cases that must attribute the change to somebody - anything writing
/// `updated_by` or an audit row that says "who".
pub fn acting_user(caller: &Caller) -> ServiceResult<UserId> {
    caller.user_id().ok_or(ServiceError::Unauthenticated)
}

#[cfg(test)]
mod tests {
    use phonix_core::identity::UserStatus;
    use phonix_core::permissions as names;

    use super::*;

    fn user_with(permissions: &[&str], mfa_satisfied: bool) -> AuthUser {
        let mut set = PermissionSet::new();
        for permission in permissions {
            set.grant(permission);
        }

        AuthUser {
            id: uuid::Uuid::nil(),
            email: "ada@example.com".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            display_name: "Ada Lovelace".into(),
            roles: vec!["User".into()],
            permissions: set,
            is_owner: false,
            status: UserStatus::Active,
            mfa_enabled: true,
            mfa_satisfied,
            email_verified: true,
        }
    }

    #[test]
    fn a_held_permission_passes_and_a_missing_one_does_not() {
        let caller = Caller::user(user_with(&[names::USERS, names::USERS_CREATE], true));

        assert!(caller.require(names::USERS_CREATE).is_ok());
        assert!(caller.require(names::USERS_DELETE).is_err());
    }

    #[test]
    fn the_refusal_names_what_was_required() {
        let caller = Caller::user(user_with(&[], true));

        match caller.require(names::USERS_DELETE) {
            Err(ServiceError::Forbidden(denied)) => {
                assert_eq!(denied.required, names::USERS_DELETE);
            }
            other => panic!("expected a refusal naming the permission, got {other:?}"),
        }
    }

    #[test]
    fn a_half_authenticated_caller_holds_nothing() {
        // Password proven, second factor outstanding. This is the state the
        // challenge screen runs in, and it must not be able to do anything
        // else - including things the user really is permitted.
        let caller = Caller::user(user_with(&[names::USERS, names::USERS_CREATE], false));

        assert!(caller.require(names::USERS_CREATE).is_err());
        assert!(caller.permissions().is_empty());
    }

    #[test]
    fn a_half_authenticated_caller_cannot_even_act_on_itself() {
        let user = user_with(&[], false);
        let own_id = user.id;
        let caller = Caller::user(user);

        // Otherwise "change your own password" would be reachable from a
        // session that has proven a password and nothing more - which is
        // exactly the session an attacker holding a stolen password has.
        assert!(caller.require_self_or(own_id, names::USERS_EDIT).is_err());
    }

    #[test]
    fn acting_on_your_own_account_needs_no_permission() {
        let user = user_with(&[], true);
        let own_id = user.id;
        let someone_else = uuid::Uuid::from_u128(9);
        let caller = Caller::user(user);

        assert!(caller.require_self_or(own_id, names::USERS_EDIT).is_ok());
        assert!(
            caller
                .require_self_or(someone_else, names::USERS_EDIT)
                .is_err()
        );
    }

    #[test]
    fn require_all_needs_all_of_them() {
        let caller = Caller::user(user_with(&[names::USERS, names::USERS_CREATE], true));

        assert!(
            caller
                .require_all(&[names::USERS, names::USERS_CREATE])
                .is_ok()
        );
        assert!(
            caller
                .require_all(&[names::USERS_CREATE, names::USERS_DELETE])
                .is_err()
        );
    }

    #[test]
    fn require_any_needs_one_of_them() {
        let caller = Caller::user(user_with(&[names::AUDIT_LOGS], true));

        assert!(
            caller
                .require_any(&[names::USERS_CREATE, names::AUDIT_LOGS])
                .is_ok()
        );
        assert!(
            caller
                .require_any(&[names::USERS_CREATE, names::USERS_DELETE])
                .is_err()
        );
    }

    #[test]
    fn the_system_caller_passes_and_says_why_it_exists() {
        let caller = Caller::system("workspace onboarding");

        assert!(caller.require(names::SETTINGS).is_ok());
        // It is nobody, so anything attributing a change to a person must
        // refuse it rather than write a null and move on.
        assert!(caller.user_id().is_none());
        assert!(acting_user(&caller).is_err());
    }

    #[test]
    fn a_suspended_account_holds_nothing_even_with_grants() {
        let mut user = user_with(&[names::USERS_CREATE], true);
        user.status = UserStatus::Suspended;
        let caller = Caller::user(user);

        assert!(caller.require(names::USERS_CREATE).is_err());
    }
}
