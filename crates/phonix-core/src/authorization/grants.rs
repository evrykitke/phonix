//! What one account holds, and where each piece of it came from.
//!
//! The permission editor cannot show a plain list of checkboxes, because two
//! accounts with the same ticks can have arrived there by different routes and
//! unticking a box means different things in each:
//!
//! ```text
//! role grants it, nothing overrides   ..  untick => record a denial
//! granted to this account alone       ..  untick => remove the grant
//! role grants it, account denied      ..  tick   => remove the denial
//! nothing grants it                   ..  tick   => record a grant
//! ```
//!
//! [`GrantSource`] is that distinction, and [`UserPermissionView::overrides_for`]
//! is the whole of the save path: hand it the roles and the ticked set, and it
//! produces the two override sets to store. It lives here rather than in the
//! service layer so it is testable without a database and so the screen can
//! show the consequence of a click before the click is made.
//!
//! # Denials beat grants
//!
//! That precedence is decided in [`crate::authorization`] and enforced in the
//! query that resolves a user. Nothing here re-implements it; this module only
//! decides which rows to write so that the query gives the intended answer.

use serde::{Deserialize, Serialize};

use crate::identity::UserId;

use super::permission_set::PermissionSet;

/// Why an account holds a permission - or why it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantSource {
    /// Nothing grants it.
    NotGranted,
    /// One of the account's roles grants it, and nothing takes it away.
    Role,
    /// Granted to this account individually, on top of its roles.
    Individual,
    /// A role grants it, but this account is individually denied.
    ///
    /// The state that makes the editor worth building: without it the only way
    /// to exclude one person from one thing is a near-duplicate role.
    Denied,
}

impl GrantSource {
    /// Whether the account may actually do it.
    pub const fn is_granted(self) -> bool {
        matches!(self, Self::Role | Self::Individual)
    }

    /// The word next to the checkbox. `None` for the ordinary cases, which
    /// need no annotation.
    pub const fn label(self) -> Option<&'static str> {
        match self {
            Self::NotGranted => None,
            Self::Role => Some("from role"),
            Self::Individual => Some("granted directly"),
            Self::Denied => Some("denied for this user"),
        }
    }
}

/// The two override sets stored against one account.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionOverrides {
    /// Individual additions, on top of whatever the roles give.
    pub granted: PermissionSet,
    /// Individual denials, which beat any role grant.
    pub denied: PermissionSet,
}

impl PermissionOverrides {
    pub fn is_empty(&self) -> bool {
        self.granted.is_empty() && self.denied.is_empty()
    }
}

/// One account's permissions, as the editor needs to render them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPermissionView {
    pub user_id: UserId,
    pub display_name: String,
    pub email: String,
    /// Created the workspace. Its permissions are not editable - see
    /// [`Self::is_editable`].
    pub is_owner: bool,
    pub roles: Vec<String>,
    /// The union of what this account's roles grant.
    pub from_roles: PermissionSet,
    pub overrides: PermissionOverrides,
}

impl UserPermissionView {
    /// What the account may actually do: roles, plus grants, minus denials.
    ///
    /// The same arithmetic the resolving query performs, restated here so the
    /// editor can show the outcome without a round trip.
    pub fn effective(&self) -> PermissionSet {
        let mut effective = self.from_roles.clone();
        effective.extend_from(&self.overrides.granted);

        for denied in self.overrides.denied.iter() {
            // Exact removal, not `revoke`: a denial names one permission, and
            // cascading it to descendants here would deny things the stored
            // rows do not.
            effective.remove_exact(denied);
        }

        effective
    }

    /// Where one permission comes from.
    pub fn source(&self, name: &str) -> GrantSource {
        if self.overrides.denied.is_granted(name) {
            return GrantSource::Denied;
        }
        if self.overrides.granted.is_granted(name) {
            return GrantSource::Individual;
        }
        if self.from_roles.is_granted(name) {
            return GrantSource::Role;
        }
        GrantSource::NotGranted
    }

    /// Whether this account's permissions may be changed at all.
    ///
    /// The owner's may not. A workspace whose owner has been denied
    /// `Administration` is a workspace nobody can administer, and the
    /// individual-denial mechanism is precisely powerful enough to do that by
    /// accident.
    pub const fn is_editable(&self) -> bool {
        !self.is_owner
    }

    /// The overrides that would make `desired` the effective set.
    ///
    /// The save path in one function. `desired` is what the editor has ticked;
    /// it is normalised first, so a client that ticked a child without its
    /// parent - or named a permission this build has since dropped - cannot
    /// store a set the resolver would read differently than the screen showed.
    pub fn overrides_for(
        from_roles: &PermissionSet,
        desired: &PermissionSet,
    ) -> PermissionOverrides {
        let desired = normalise(desired);

        let mut granted = PermissionSet::new();
        for name in desired.iter() {
            if !from_roles.is_granted(name) {
                granted.insert_exact(name);
            }
        }

        let mut denied = PermissionSet::new();
        for name in from_roles.iter() {
            if !desired.is_granted(name) {
                denied.insert_exact(name);
            }
        }

        PermissionOverrides { granted, denied }
    }
}

/// Close a submitted selection under the tree's own rules.
///
/// Ticking a child implies its ancestors - that is [`PermissionSet::grant`]'s
/// job - and a name this build does not define is dropped rather than stored.
pub fn normalise(desired: &PermissionSet) -> PermissionSet {
    let mut out = PermissionSet::new();
    for name in desired.iter() {
        if super::definitions::is_defined(name) {
            out.grant(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::names;
    use super::*;

    fn set(names: &[&str]) -> PermissionSet {
        names.iter().copied().collect()
    }

    fn view(from_roles: &[&str], granted: &[&str], denied: &[&str]) -> UserPermissionView {
        UserPermissionView {
            user_id: UserId::nil(),
            display_name: "Ada Lovelace".into(),
            email: "ada@example.com".into(),
            is_owner: false,
            roles: vec!["User".into()],
            from_roles: set(from_roles),
            overrides: PermissionOverrides {
                granted: set(granted),
                denied: set(denied),
            },
        }
    }

    #[test]
    fn a_denial_beats_the_role_that_grants_it() {
        let view = view(&[names::PAGES, names::DASHBOARD], &[], &[names::DASHBOARD]);

        assert_eq!(view.source(names::DASHBOARD), GrantSource::Denied);
        assert!(!view.effective().is_granted(names::DASHBOARD));
        // The rest of what the role gives is untouched.
        assert!(view.effective().is_granted(names::PAGES));
    }

    #[test]
    fn each_permission_reports_where_it_came_from() {
        let view = view(
            &[names::PAGES, names::DASHBOARD],
            &[names::AUDIT_LOGS],
            &[names::DASHBOARD],
        );

        assert_eq!(view.source(names::PAGES), GrantSource::Role);
        assert_eq!(view.source(names::AUDIT_LOGS), GrantSource::Individual);
        assert_eq!(view.source(names::DASHBOARD), GrantSource::Denied);
        assert_eq!(view.source(names::USERS), GrantSource::NotGranted);

        assert!(GrantSource::Role.is_granted());
        assert!(GrantSource::Individual.is_granted());
        assert!(!GrantSource::Denied.is_granted());
        assert!(!GrantSource::NotGranted.is_granted());
    }

    #[test]
    fn ticking_something_a_role_already_gives_stores_nothing() {
        // The case that would otherwise fill `user_permissions` with rows that
        // change nothing, and then keep granting after the role stopped.
        let roles = set(&[names::PAGES, names::DASHBOARD]);
        let overrides = UserPermissionView::overrides_for(&roles, &roles);

        assert!(overrides.is_empty(), "{overrides:?}");
    }

    #[test]
    fn unticking_a_role_grant_records_a_denial() {
        let roles = set(&[names::PAGES, names::DASHBOARD]);
        let desired = set(&[names::PAGES]);

        let overrides = UserPermissionView::overrides_for(&roles, &desired);

        assert!(overrides.granted.is_empty());
        assert!(overrides.denied.is_granted(names::DASHBOARD));
        assert!(!overrides.denied.is_granted(names::PAGES));
    }

    #[test]
    fn ticking_something_no_role_gives_records_a_grant_with_its_ancestors() {
        // `Users.Create` on its own is unusable: the account could create a
        // user but not open the page. The ancestors come along.
        let roles = PermissionSet::new();
        let desired = set(&[names::USERS_CREATE]);

        let overrides = UserPermissionView::overrides_for(&roles, &desired);

        for expected in [
            names::PAGES,
            names::ADMINISTRATION,
            names::USERS,
            names::USERS_CREATE,
        ] {
            assert!(overrides.granted.is_granted(expected), "{expected}");
        }
        assert!(overrides.denied.is_empty());
    }

    #[test]
    fn a_permission_this_build_does_not_define_is_dropped_not_stored() {
        let roles = PermissionSet::new();
        let desired = set(&["Pages.Inventory.Requisitions.Approve"]);

        let overrides = UserPermissionView::overrides_for(&roles, &desired);

        assert!(overrides.is_empty(), "{overrides:?}");
    }

    #[test]
    fn saving_then_reloading_gives_back_the_same_screen() {
        // The round trip that matters: whatever the editor showed must survive
        // being stored and read again, or an administrator's second visit
        // silently disagrees with their first.
        let roles = set(&[names::PAGES, names::DASHBOARD, names::ADMINISTRATION]);
        let desired = set(&[names::PAGES, names::ADMINISTRATION, names::AUDIT_LOGS]);

        let overrides = UserPermissionView::overrides_for(&roles, &desired);
        let reloaded = UserPermissionView {
            from_roles: roles,
            overrides,
            ..view(&[], &[], &[])
        };

        assert_eq!(reloaded.effective(), normalise(&desired));
    }

    #[test]
    fn the_owner_is_not_editable() {
        let owner = UserPermissionView {
            is_owner: true,
            ..view(&[names::PAGES], &[], &[])
        };
        assert!(!owner.is_editable());
        assert!(view(&[names::PAGES], &[], &[]).is_editable());
    }
}
