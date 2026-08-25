//! The permissions one user effectively holds.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::definitions::{DEFINITIONS, ancestors, is_defined, is_descendant_of};

/// A flattened set of granted permission names.
///
/// Resolved server-side from a user's roles and their individual overrides,
/// then serialised to the browser so the UI can hide what the user cannot do.
/// That copy is a *convenience*, never the enforcement point: every server
/// function re-resolves from the database before acting. Hiding a button is
/// courtesy; refusing the call is security.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PermissionSet(BTreeSet<String>);

impl PermissionSet {
    pub fn new() -> Self {
        Self(BTreeSet::new())
    }

    /// Every permission this build defines. What the `Admin` role gets.
    pub fn all() -> Self {
        Self(DEFINITIONS.iter().map(|def| def.name.to_owned()).collect())
    }

    /// The permissions the static `User` role starts with.
    pub fn defaults_for_user() -> Self {
        Self(
            DEFINITIONS
                .iter()
                .filter(|def| def.default_for_user)
                .map(|def| def.name.to_owned())
                .collect(),
        )
    }

    /// Drop everything belonging to an app this workspace has not switched on.
    ///
    /// This is what "installing an app" comes down to. Enablement is stored in
    /// one column of one table, and it reaches the menu, the command palette,
    /// every grid and every `Caller::require` by way of this method - because
    /// all of them already answer to permissions, and a second parallel
    /// mechanism would be a second place for the two to disagree.
    ///
    /// `enabled` is app ids, as `core.installed_apps` hands them back. The
    /// always-on apps are kept whether they appear in it or not: a workspace
    /// that had switched `core` off would be one nobody could sign in to.
    #[must_use]
    pub fn for_enabled_apps(mut self, enabled: &[String]) -> Self {
        self.0.retain(|name| {
            crate::apps::owner_of(name)
                .is_some_and(|app| app.always_on || enabled.iter().any(|id| id == app.id))
        });
        self
    }

    /// Exact-match check. This is the one every guard should call.
    pub fn is_granted(&self, name: &str) -> bool {
        self.0.contains(name)
    }

    /// True when *all* of `names` are held. For an action needing several.
    pub fn is_granted_all(&self, names: &[&str]) -> bool {
        names.iter().all(|name| self.is_granted(name))
    }

    /// True when *any* of `names` is held. For deciding whether a menu with
    /// several entries should appear at all.
    pub fn is_granted_any(&self, names: &[&str]) -> bool {
        names.iter().any(|name| self.is_granted(name))
    }

    /// Whether anything at or beneath `prefix` is granted, for showing a
    /// section header whose individual items are checked separately.
    pub fn has_any_under(&self, prefix: &str) -> bool {
        self.0
            .iter()
            .any(|held| held == prefix || is_descendant_of(held, prefix))
    }

    /// Grant `name` and every ancestor it needs to be reachable.
    ///
    /// Granting `Users.Create` without `Administration` would produce a user
    /// who may create accounts but cannot open the page that does it. The
    /// ancestors come along so that state is not representable.
    pub fn grant(&mut self, name: &str) {
        for ancestor in ancestors(name) {
            self.0.insert(ancestor.to_owned());
        }
        self.0.insert(name.to_owned());
    }

    /// Revoke `name` and everything beneath it.
    ///
    /// The mirror of [`PermissionSet::grant`]: dropping `Users` while leaving
    /// `Users.Delete` behind would leave an orphaned grant that some later
    /// check might honour.
    pub fn revoke(&mut self, name: &str) {
        self.0
            .retain(|held| held != name && !is_descendant_of(held, name));
    }

    /// Add exactly this name, pulling in nothing.
    ///
    /// For override bookkeeping only - see [`crate::authorization::grants`].
    /// A user's individual grants are stored *alongside* what their roles give,
    /// so cascading ancestors in here would write rows duplicating role grants,
    /// and those rows would keep granting after the role stopped. Screens and
    /// role editors want [`Self::grant`].
    pub fn insert_exact(&mut self, name: &str) {
        self.0.insert(name.to_owned());
    }

    /// Remove exactly this name, leaving anything beneath it.
    ///
    /// The counterpart of [`Self::insert_exact`], and the reason a denial does
    /// not silently take out a subtree the stored rows never named.
    pub fn remove_exact(&mut self, name: &str) -> bool {
        self.0.remove(name)
    }

    /// Merge another set in. Used to union a user's several roles.
    pub fn extend_from(&mut self, other: &Self) {
        self.0.extend(other.0.iter().cloned());
    }

    /// Drop names this build does not define, returning what was dropped.
    ///
    /// Run after loading from the database. A permission removed from the code
    /// leaves its grants behind, and carrying them forward means a later
    /// version that reuses the name silently inherits them.
    pub fn prune_unknown(&mut self) -> Vec<String> {
        let unknown: Vec<String> = self
            .0
            .iter()
            .filter(|name| !is_defined(name))
            .cloned()
            .collect();
        for name in &unknown {
            self.0.remove(name);
        }
        unknown
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether this set covers the whole tree - i.e. is an unrestricted admin.
    pub fn contains_all_defined(&self) -> bool {
        DEFINITIONS.iter().all(|def| self.is_granted(def.name))
    }
}

impl FromIterator<String> for PermissionSet {
    fn from_iter<I: IntoIterator<Item = String>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<'a> FromIterator<&'a str> for PermissionSet {
    fn from_iter<I: IntoIterator<Item = &'a str>>(iter: I) -> Self {
        Self(iter.into_iter().map(str::to_owned).collect())
    }
}

impl fmt::Display for PermissionSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}]",
            self.0.iter().cloned().collect::<Vec<_>>().join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::definitions::names;
    use super::*;

    #[test]
    fn granting_a_leaf_pulls_in_its_ancestors() {
        let mut set = PermissionSet::new();
        set.grant(names::USERS_CREATE);

        assert!(set.is_granted(names::USERS_CREATE));
        assert!(set.is_granted(names::USERS));
        assert!(set.is_granted(names::ADMINISTRATION));
        assert!(set.is_granted(names::PAGES));
        // ...and nothing else.
        assert!(!set.is_granted(names::USERS_DELETE));
        assert_eq!(set.len(), 4);
    }

    #[test]
    fn revoking_a_branch_takes_its_children() {
        let mut set = PermissionSet::all();
        set.revoke(names::USERS);

        assert!(!set.is_granted(names::USERS));
        assert!(!set.is_granted(names::USERS_CREATE));
        assert!(!set.is_granted(names::USERS_DELETE));
        // A sibling branch is untouched.
        assert!(set.is_granted(names::ROLES_CREATE));
        assert!(set.is_granted(names::ADMINISTRATION));
    }

    #[test]
    fn admin_holds_everything_and_user_holds_the_basics() {
        let admin = PermissionSet::all();
        assert!(admin.contains_all_defined());
        assert!(admin.is_granted(names::USERS_DELETE));

        let user = PermissionSet::defaults_for_user();
        assert!(user.is_granted(names::PAGES));
        assert!(user.is_granted(names::DASHBOARD));
        // The default role must not reach administration.
        assert!(!user.is_granted(names::ADMINISTRATION));
        assert!(!user.is_granted(names::USERS));
        assert!(!user.has_any_under(names::ADMINISTRATION));
        assert!(!user.contains_all_defined());
    }

    #[test]
    fn any_and_all_behave_as_named() {
        let mut set = PermissionSet::new();
        set.grant(names::USERS_CREATE);

        assert!(set.is_granted_any(&[names::USERS_CREATE, names::USERS_DELETE]));
        assert!(!set.is_granted_all(&[names::USERS_CREATE, names::USERS_DELETE]));
        assert!(set.is_granted_all(&[names::USERS_CREATE, names::USERS]));
        assert!(!set.is_granted_any(&[names::SETTINGS, names::AUDIT_LOGS]));
    }

    #[test]
    fn several_roles_union_into_one_set() {
        let mut editor = PermissionSet::new();
        editor.grant(names::USERS_EDIT);

        let mut auditor = PermissionSet::new();
        auditor.grant(names::AUDIT_LOGS);

        editor.extend_from(&auditor);
        assert!(editor.is_granted(names::USERS_EDIT));
        assert!(editor.is_granted(names::AUDIT_LOGS));
    }

    #[test]
    fn unknown_grants_are_pruned_on_load() {
        let mut set: PermissionSet = ["Pages", "Pages.Removed.Feature", names::DASHBOARD]
            .into_iter()
            .collect();

        let dropped = set.prune_unknown();

        assert_eq!(dropped, vec!["Pages.Removed.Feature".to_owned()]);
        assert!(set.is_granted(names::DASHBOARD));
        assert!(!set.is_granted("Pages.Removed.Feature"));
    }

    #[test]
    fn a_set_survives_the_trip_to_the_browser() {
        let set = PermissionSet::defaults_for_user();
        let json = serde_json::to_string(&set).unwrap();
        // Serialised transparently, as a plain array of strings.
        assert!(json.starts_with('['), "got {json}");
        assert_eq!(serde_json::from_str::<PermissionSet>(&json).unwrap(), set);
    }
}
