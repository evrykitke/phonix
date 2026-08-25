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

    /// The app's own front page, or `None` for one that *is* the shell.
    ///
    /// Declared rather than derived from [`Self::permission`]. It was derived
    /// once, which produced `/pages` for core - an address that has never
    /// existed - because core's permission root is the tree's root. A field
    /// that is sometimes wrong is worse than a field somebody has to fill in,
    /// and `every_home_sits_under_its_permission` keeps the two in step.
    ///
    /// `None` is what the launcher skips. Core has nowhere to send anybody:
    /// it is the window every other app is drawn in.
    pub home: Option<&'static str>,

    /// Not offered in the store, not removable, always on.
    ///
    /// Two apps are, for different reasons. `core` is the sign-in page, the
    /// user list and the audit trail - a workspace that had switched it off
    /// would be one nobody could sign in to.
    ///
    /// `master` is the harder call and the more interesting one. It looked
    /// optional: a clinical build would not want a customer list. But that is
    /// an argument about which crates get *compiled in*, and this field is
    /// about what one workspace of a running deployment subscribes to - a
    /// different axis, and treating them as one is the mistake. Master data is
    /// what the other apps *reference*: an invoice names a party and a tax
    /// group, a purchase order would name a supplier, a CRM would name both.
    /// Make it switchable and every app that reads it has to answer "what if
    /// the thing I point at is off", which in practice means every app
    /// declaring `requires: ["master"]` - always-on again, with a dependency
    /// graph to maintain on top.
    ///
    /// It stays an app in every other sense: its own schema, its own migration
    /// stream, no foreign key reaching out of it. That is the boundary in
    /// `docs/adr/0001-core-boundary.md`, and it is worth keeping whether or
    /// not anybody can switch the thing off.
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

    /// Whether the launcher has anywhere to send somebody who picks this.
    pub const fn is_a_place(&self) -> bool {
        self.home.is_some()
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
        // The shell itself. There is nowhere to go: you are already there.
        home: None,
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
        home: Some("/master"),
        requires: &[],
        // Part of every workspace - see the field. It keeps its own schema and
        // its own migration stream all the same.
        always_on: true,
    },
    AppDescriptor {
        id: BOOKS,
        name: "app.books.name",
        summary: "app.books.summary",
        icon: "file-text",
        version: "0.1.0",
        permission: names::SALES,
        home: Some("/sales"),
        // An invoice names a customer and a tax group, and both are master's.
        // Always satisfied now that master is part of every workspace, and
        // stated anyway: it is true, and it is what an installer would need if
        // that ever stopped being so.
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

/// Whether a workspace holding `enabled` can hold this permission at all.
///
/// The question a permission editor has to ask before it draws a checkbox: a
/// tick beside an app nobody has subscribed to is a control that is ignored
/// when it is saved, which is worse than a control that is absent.
///
/// Always-on apps pass whatever `enabled` says, and a permission no app owns
/// passes too - the tests here make the second case impossible, and refusing
/// it at a call site would hide a new permission rather than a bug.
pub fn covers(enabled: &[String], permission: &str) -> bool {
    owner_of(permission).is_none_or(|app| app.always_on || enabled.iter().any(|id| id == app.id))
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
    fn the_store_offers_only_what_a_workspace_can_do_without() {
        // Core is the shell. Master data is what the other apps *reference* -
        // see the field's documentation for why that makes it a workspace
        // artifact rather than a subscription.
        let always_on: Vec<&str> = CATALOG
            .iter()
            .filter(|app| app.always_on)
            .map(|app| app.id)
            .collect();

        assert_eq!(always_on, vec![CORE, MASTER]);
        assert_eq!(
            optional().map(|app| app.id).collect::<Vec<_>>(),
            vec![BOOKS]
        );
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
    fn a_workspace_can_only_hold_the_permissions_of_apps_it_has() {
        let books_only = vec![BOOKS.to_owned()];

        // Its own, and the always-on apps, which every workspace has.
        assert!(covers(&books_only, names::INVOICES_POST));
        assert!(covers(&books_only, names::USERS));
        assert!(covers(&books_only, names::PAGES));
        assert!(covers(&books_only, names::PARTIES));

        // A workspace with nothing optional still has core and master.
        let nothing: Vec<String> = Vec::new();
        assert!(covers(&nothing, names::AUDIT_LOGS));
        assert!(covers(&nothing, names::TAXES_EDIT));
        assert!(!covers(&nothing, names::INVOICES));
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
    fn every_home_sits_under_its_permission() {
        // The route and the permission root are the same fact said twice, and
        // deriving one from the other put core at `/pages`. So they are stated
        // separately and checked against each other: `Pages.Sales` implies
        // something beginning `/sales`, and a screen that answered on a
        // different path would be one the permission does not guard.
        for app in CATALOG {
            let Some(home) = app.home else {
                assert!(app.always_on, "{} has no home and is not the shell", app.id);
                continue;
            };

            assert!(home.starts_with('/'), "{} has a relative home", app.id);

            let implied = app
                .permission
                .strip_prefix("Pages.")
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();

            assert_eq!(
                home,
                format!("/{implied}"),
                "{}'s home and its permission root disagree",
                app.id,
            );
        }
    }

    #[test]
    fn only_the_shell_has_nowhere_to_go() {
        let homeless: Vec<&str> = CATALOG
            .iter()
            .filter(|app| !app.is_a_place())
            .map(|app| app.id)
            .collect();

        assert_eq!(homeless, vec![CORE]);
    }

    #[test]
    fn a_workspace_always_has_the_always_on_apps_whatever_it_enabled() {
        let nothing: Vec<String> = Vec::new();
        let ids: Vec<&str> = enabled_in(&nothing).map(|app| app.id).collect();
        assert_eq!(ids, vec![CORE, MASTER]);

        let some = vec![BOOKS.to_owned()];
        let ids: Vec<&str> = enabled_in(&some).map(|app| app.id).collect();
        assert_eq!(ids, vec![CORE, MASTER, BOOKS]);
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
