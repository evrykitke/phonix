//! What a workspace can switch on.
//!
//! An app is a product: Books, and one day a CRM, a procurement module, an
//! inventory. A workspace subscribes to some of them and not to others, and the
//! ones it has not subscribed to should be invisible - not greyed out, not
//! answering with "forbidden", simply not part of that workspace's software.
//!
//! # Installing is not loading
//!
//! Nothing arrives at runtime. Apps are crates, compiled into one binary and
//! one wasm bundle, and every tenant database has every app's schema migrated
//! into it whether the workspace uses it or not. That is deliberate: a schema
//! that appears the moment somebody subscribes is a migration running under a
//! live request, and a bundle that fetches a route is a security surface.
//!
//! Installing an app **enables** it. What that means concretely is in
//! `phonix_services::workspace::apps`: the workspace's static roles gain the
//! permissions that hang beneath the app's root, and lose them again when it is
//! switched off. Everything downstream - the menu, the command palette, the
//! grids, and `Caller::require` in every service - already answers to
//! permissions, so enablement reaches all of them without a second mechanism.
//!
//! # Why the catalog lives here
//!
//! `phonix-core` compiles to wasm, and the browser is where the app launcher,
//! the store and the install dialog are drawn. The migration registry in
//! `phonix_db::tenancy::apps` cannot be: it holds `sqlx::Migrator`. So this is
//! the description and that is the plumbing, and a test over there asserts the
//! two name the same apps.
//!
//! # Adding an app
//!
//! One entry in [`CATALOG`], four message keys, an icon in
//! `phonix_web::apps::icon_of`, and the permissions it names declared in
//! [`crate::authorization::definitions`]. The tests here refuse everything
//! else: an unknown dependency, an undeclared permission, an id that is not a
//! legal Postgres schema name.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::authorization::names;

/// One app, as the store and the launcher describe it.
///
/// Every human-readable field is a message key rather than a sentence, for the
/// reason everything user-facing in this project is: the catalog is compiled
/// once and read in three languages.
///
/// Not serialisable, and it does not need to be: the browser has this table
/// compiled into the wasm bundle, so what crosses the wire is only what the
/// *workspace* has done about each app - see `AppState` in
/// `phonix_services::workspace::apps`. Sending the descriptions themselves
/// would be shipping a constant over the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppDescriptor {
    /// Stable, lowercase, and *identical to the Postgres schema it owns*.
    ///
    /// Never rename one. It is a primary key in every tenant database and the
    /// name of a schema holding their data.
    pub id: &'static str,

    /// Message key for the name on the tile, e.g. `app.books.name`.
    pub name: &'static str,

    /// Message key for the one line under it.
    pub summary: &'static str,

    /// Lucide icon name, kebab-cased - `file-text`, `boxes`.
    ///
    /// A string rather than the `Icon` enum because that enum is three hundred
    /// variants of SVG geometry living in `phonix-web`, which is above this
    /// crate. `phonix_web::apps::icon_of` resolves it, and a test there fails
    /// if an app names an icon that has not been through `tools/icons.txt`.
    pub icon: &'static str,

    /// The app's own version, not its schema version.
    ///
    /// Schema version is a migration count and means nothing to a reader;
    /// this is what a changelog entry is filed under and what "Books 0.2.0 -
    /// credit notes" is announcing. They move independently: a release can
    /// change a screen without touching a table.
    pub version: &'static str,

    /// The permission every one of this app's pages hangs beneath.
    ///
    /// Enabling the app grants this and its descendants to the static roles;
    /// disabling revokes them. That is why an app must own a whole subtree and
    /// not scatter permissions through somebody else's: revocation is a prefix
    /// match, and a shared parent would take a neighbour's pages down with it.
    pub permission: &'static str,

    /// App ids that have to be on for this one to be useful.
    ///
    /// Installing pulls them in; uninstalling one that something else depends
    /// on is refused. Not a foreign key anywhere - see
    /// `docs/adr/0001-core-boundary.md` for why an app may never hold one into
    /// another app's schema.
    pub requires: &'static [&'static str],

    /// Not offered, not removable, always on.
    ///
    /// `core` is the only one: it is the sign-in page, the user list and the
    /// audit trail, and a workspace that had switched it off would be a
    /// workspace nobody could sign in to.
    pub always_on: bool,
}

impl AppDescriptor {
    /// Whether `permission` belongs to this app.
    ///
    /// A prefix match on a *dotted boundary*, so `Pages.Sales` claims
    /// `Pages.Sales.Invoices.Post` and would not claim a hypothetical
    /// `Pages.SalesTax`.
    pub fn owns(&self, permission: &str) -> bool {
        permission == self.permission
            || permission
                .strip_prefix(self.permission)
                .is_some_and(|rest| rest.starts_with('.'))
    }

    /// Where the launcher sends somebody who picks this app.
    ///
    /// Derived from the permission rather than stored: the route and the
    /// permission are already the same fact said twice, and one of the two
    /// spellings would eventually be wrong.
    pub fn home(&self) -> String {
        let path = self
            .permission
            .strip_prefix("Pages.")
            .unwrap_or(self.permission);

        let mut href = String::with_capacity(path.len() + 1);
        for segment in path.split('.') {
            href.push('/');
            for (index, ch) in segment.char_indices() {
                if ch.is_ascii_uppercase() && index > 0 {
                    href.push('-');
                }
                href.push(ch.to_ascii_lowercase());
            }
        }
        href
    }
}

/// The app that owns everything the others are allowed to depend on.
pub const CORE: &str = "core";
/// Commercial master data: parties, taxes.
pub const MASTER: &str = "master";
/// Sales: what the workspace invoices.
pub const BOOKS: &str = "books";

/// Every app this build can offer.
///
/// Order is display order in the store, and core comes first for the same
/// reason it migrates first: everything else assumes it.
pub const CATALOG: &[AppDescriptor] = &[
    AppDescriptor {
        id: CORE,
        name: "app.core.name",
        summary: "app.core.summary",
        icon: "layout-dashboard",
        version: "1.0.0",
        permission: names::PAGES,
        requires: &[],
        always_on: true,
    },
    AppDescriptor {
        id: MASTER,
        name: "app.master.name",
        summary: "app.master.summary",
        icon: "boxes",
        version: "0.1.0",
        permission: names::MASTER,
        requires: &[],
        always_on: false,
    },
    AppDescriptor {
        id: BOOKS,
        name: "app.books.name",
        summary: "app.books.summary",
        icon: "file-text",
        version: "0.1.0",
        permission: names::SALES,
        // An invoice names a customer and a tax group, and both are master's.
        // Books without master would be a screen that can only ever say the
        // list is empty.
        requires: &[MASTER],
        always_on: false,
    },
];

/// Look one up by id.
pub fn find(id: &str) -> Option<&'static AppDescriptor> {
    CATALOG.iter().find(|app| app.id == id)
}

/// The apps a workspace chooses, in display order.
pub fn optional() -> impl Iterator<Item = &'static AppDescriptor> {
    CATALOG.iter().filter(|app| !app.always_on)
}

/// The apps that are on in a workspace holding `enabled`, plus the always-on
/// ones.
///
/// Takes the ids rather than a set type so that the caller can pass whatever
/// the database handed back.
pub fn enabled_in<'a>(
    enabled: &'a [String],
) -> impl Iterator<Item = &'static AppDescriptor> + use<'a> {
    CATALOG
        .iter()
        .filter(move |app| app.always_on || enabled.iter().any(|id| id == app.id))
}

/// Which app a permission belongs to, if any.
///
/// `None` for a permission no app claims, which is a bug the tests here catch
/// rather than something to handle: a permission outside every app's subtree
/// could never be granted or revoked by installing anything.
pub fn owner_of(permission: &str) -> Option<&'static AppDescriptor> {
    // Longest permission root first, so `Pages.Sales` wins over `Pages` for
    // `Pages.Sales.Invoices`. Core owns `Pages` itself and would otherwise
    // claim everything.
    CATALOG
        .iter()
        .filter(|app| app.owns(permission))
        .max_by_key(|app| app.permission.len())
}

/// What a workspace has done about one app.
///
/// The *state*, and only the state. The name, the summary, the icon and the
/// dependencies are [`CATALOG`], compiled into the browser bundle already, so
/// sending them would be sending a constant over the network; a store screen
/// joins this to the catalog by id.
///
/// Here rather than in `phonix-services` for the reason `PostOutcome` is in
/// `app-books`: the browser is one of the two ends of this wire, and it does
/// not have the services crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppState {
    pub app_id: String,
    pub enabled: bool,
    /// The app's version at the moment it was switched on, which is not
    /// necessarily the version running now. The difference between the two is
    /// what a "what's new" list is a list of.
    pub installed_version: Option<String>,
    pub enabled_on: Option<DateTime<Utc>>,
}

/// What an install did.
///
/// A list rather than a unit, because installing Books installs master data
/// too, and a dialog that said "Books is ready" without mentioning that would
/// be hiding a change to somebody's menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    /// The app that was asked for.
    pub app_id: String,
    /// Everything switched on by this call, dependencies first, in install
    /// order. Empty when all of it was already on.
    pub switched_on: Vec<String>,
}

/// What an uninstall answered.
///
/// A value rather than a `Result`, for the reason `PostOutcome` is one: two of
/// the three answers are things a screen renders beside the button, not faults.
/// And both of those name an *app*, which a `Message` could not carry - a name
/// here is itself a message key, and a key interpolated into a sentence renders
/// as `app.books.name`. The browser has the catalog; it builds the sentence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UninstallOutcome {
    /// It is off. Also the answer when it was already off - "make it off" is a
    /// statement about the end state.
    SwitchedOff,
    /// Core. Every workspace has it and no workspace can be without it.
    AlwaysOn,
    /// Something else that is switched on depends on it. Names which, because
    /// "no" without a reason is a dead end and this one has an obvious next
    /// step.
    NeededBy { app_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        for (index, app) in CATALOG.iter().enumerate() {
            assert!(
                !CATALOG[..index].iter().any(|other| other.id == app.id),
                "{} appears twice",
                app.id
            );
        }
    }

    #[test]
    fn an_id_is_a_legal_postgres_schema_name() {
        // It reaches DDL as a schema name in every tenant database. The runner
        // asserts the same thing at the moment it is used; this fails at build
        // time instead.
        for app in CATALOG {
            assert!(
                app.id.starts_with(|c: char| c.is_ascii_lowercase())
                    && app
                        .id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{} is not a bare lowercase identifier",
                app.id
            );
            assert!((2..=63).contains(&app.id.len()), "{} is too long", app.id);
        }
    }

    #[test]
    fn core_is_the_only_app_nobody_can_switch_off() {
        let always_on: Vec<&str> = CATALOG
            .iter()
            .filter(|app| app.always_on)
            .map(|app| app.id)
            .collect();

        assert_eq!(always_on, vec![CORE]);
    }

    #[test]
    fn every_dependency_is_an_app_in_this_catalog() {
        for app in CATALOG {
            for needed in app.requires {
                assert!(
                    find(needed).is_some(),
                    "{} requires '{needed}', which is not an app",
                    app.id
                );
                assert_ne!(*needed, app.id, "{} requires itself", app.id);
            }
        }
    }

    #[test]
    fn a_dependency_comes_earlier_in_the_catalog() {
        // Install order. Books before master would mean installing Books pulls
        // in something the store has not offered yet.
        for (index, app) in CATALOG.iter().enumerate() {
            for needed in app.requires {
                assert!(
                    CATALOG[..index].iter().any(|other| other.id == *needed),
                    "{} requires '{needed}', which comes after it",
                    app.id
                );
            }
        }
    }

    #[test]
    fn every_permission_root_is_one_this_build_defines() {
        for app in CATALOG {
            assert!(
                crate::authorization::is_defined(app.permission),
                "{} hangs off '{}', which is not in the permission tree",
                app.id,
                app.permission
            );
        }
    }

    #[test]
    fn no_two_apps_share_a_permission_subtree() {
        // Revocation is a prefix match. Two apps under one root would mean
        // switching either off took the other's pages with it.
        for app in optional() {
            for other in optional() {
                if app.id == other.id {
                    continue;
                }
                assert!(
                    !app.owns(other.permission),
                    "{} owns {}'s permission root",
                    app.id,
                    other.id
                );
            }
        }
    }

    #[test]
    fn every_permission_belongs_to_exactly_one_app() {
        // Nothing may sit outside the catalog: a permission no app owns could
        // never be granted by installing anything, so the page behind it would
        // be unreachable for ever with no error to explain it.
        for definition in crate::authorization::DEFINITIONS {
            assert!(
                owner_of(definition.name).is_some(),
                "'{}' belongs to no app",
                definition.name
            );
        }
    }

    #[test]
    fn the_deepest_root_claims_a_permission() {
        // Core owns `Pages`, so every permission matches it. The invoice
        // permissions have to come back as Books all the same.
        assert_eq!(owner_of(names::INVOICES_POST).map(|a| a.id), Some(BOOKS));
        assert_eq!(owner_of(names::PARTIES).map(|a| a.id), Some(MASTER));
        assert_eq!(owner_of(names::USERS).map(|a| a.id), Some(CORE));
    }

    #[test]
    fn an_app_owns_its_own_root_and_nothing_beside_it() {
        let books = find(BOOKS).expect("books is in the catalog");

        assert!(books.owns(names::SALES));
        assert!(books.owns(names::INVOICES_VOID));
        assert!(!books.owns(names::PARTIES));
        // The dotted boundary: a sibling whose name merely starts the same way.
        assert!(!books.owns("Pages.SalesTax"));
    }

    #[test]
    fn home_is_the_route_the_permission_names() {
        assert_eq!(find(BOOKS).expect("books").home(), "/sales");
        assert_eq!(find(MASTER).expect("master").home(), "/master");
    }

    #[test]
    fn a_workspace_always_has_core_whatever_it_has_enabled() {
        let nothing: Vec<String> = Vec::new();
        let ids: Vec<&str> = enabled_in(&nothing).map(|app| app.id).collect();
        assert_eq!(ids, vec![CORE]);

        let some = vec![BOOKS.to_owned()];
        let ids: Vec<&str> = enabled_in(&some).map(|app| app.id).collect();
        assert_eq!(ids, vec![CORE, BOOKS]);
    }

    #[test]
    fn a_version_is_three_numbers() {
        // It is read by people, in a changelog. "0.1" and "2026-08" both drift
        // into meaning different things in different apps.
        for app in CATALOG {
            let parts: Vec<&str> = app.version.split('.').collect();
            assert_eq!(parts.len(), 3, "{} has version '{}'", app.id, app.version);
            for part in parts {
                assert!(
                    !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()),
                    "{} has version '{}'",
                    app.id,
                    app.version
                );
            }
        }
    }
}
