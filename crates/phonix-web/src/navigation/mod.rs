//! The navigation tree: what the sidebar and the command palette both read.
//!
//! # One declaration, three consumers
//!
//! The menu is declared once, in [`tree`], and three separate things read it:
//! the sidebar draws it, the command palette searches it, and the breadcrumb
//! names the current place. Adding a screen is a node in that one list - not an
//! entry in a sidebar array, another in a search index, and a third in a
//! breadcrumb map that will drift apart within a month.
//!
//! # Permissions
//!
//! Every node may name a permission from [`phonix_core::authorization`]. A node
//! whose permission the viewer does not hold is not rendered - and neither is a
//! group left with nothing inside it, which is what stops an empty
//! "Administration" heading from advertising a section nobody can open.
//!
//! This is presentation, not enforcement. Hiding a link stops it being offered;
//! it does not stop it being typed. The server function behind the screen states
//! its own permission through `phonix_services::Caller::require`, and that is
//! the check that matters. The two read the same constants so they cannot
//! disagree about the name.
//!
//! # Active state
//!
//! Menus are trees, so "which item is selected" is really "which *path* through
//! the tree is selected". [`Trail`] answers that once per navigation: the
//! deepest node whose route matches the URL, plus every ancestor. The sidebar
//! highlights the last entry, marks the rest as ancestors, and expands the
//! groups among them - so opening a child route opens its parents for free, at
//! any depth, without a single screen knowing it has to.

pub mod tree;

use phonix_core::Message;
use phonix_core::authorization::is_defined;
use phonix_core::i18n::Catalog;
use phonix_core::identity::AuthUser;

use crate::icons::Icon;

pub use tree::MENU;

/// How a node's route is compared with the current URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Match {
    /// Active when the URL is this route or anything beneath it.
    ///
    /// The default, because a list screen normally stays selected while you are
    /// looking at one of its rows.
    #[default]
    Prefix,
    /// Active only on this exact route.
    ///
    /// For routes that are a prefix of everything - `/` above all - where
    /// `Prefix` would leave them permanently selected.
    Exact,
}

/// One entry in the menu.
///
/// `const`-constructible on purpose: the whole tree is a `static`, so it costs
/// nothing at runtime and a malformed menu is a compile error rather than a
/// blank sidebar.
#[derive(Debug, Clone, Copy)]
pub struct NavNode {
    /// Stable identifier, unique across the whole tree.
    ///
    /// Separate from the route because expansion state is keyed by it and
    /// groups have no route at all.
    pub key: &'static str,

    /// The catalog key its words live under - `nav.users`, not `"Users"`.
    ///
    /// A key rather than a word because this tree is a `static`: it is built
    /// once, before any request exists, and cannot know what language the
    /// person reading it wants. See [`NavNode::label`].
    pub label_key: &'static str,

    pub icon: Option<Icon>,

    /// Where it goes. `None` makes it a pure group - a heading that opens and
    /// closes but is not itself a destination.
    pub href: Option<&'static str>,

    /// The permission required to see it, from `phonix_core::authorization::names`.
    ///
    /// `None` means everyone signed in sees it.
    pub permission: Option<&'static str>,

    pub children: &'static [NavNode],

    /// Extra words the command palette should match.
    ///
    /// For the gap between what a thing is called and what people call it:
    /// "Users" should be found by typing "people", "invite", or "staff".
    pub keywords: &'static [&'static str],

    pub match_mode: Match,

    /// Hide from the sidebar but keep in the palette and the breadcrumb.
    ///
    /// For destinations that are real places with real routes but do not earn a
    /// permanent row - a profile page, a settings sub-tab.
    pub hidden: bool,
}

impl NavNode {
    /// A leaf: label, icon, route.
    pub const fn leaf(
        key: &'static str,
        label_key: &'static str,
        icon: Icon,
        href: &'static str,
    ) -> Self {
        Self {
            key,
            label_key,
            icon: Some(icon),
            href: Some(href),
            permission: None,
            children: &[],
            keywords: &[],
            match_mode: Match::Prefix,
            hidden: false,
        }
    }

    /// A group: a heading with children and no route of its own.
    pub const fn group(
        key: &'static str,
        label_key: &'static str,
        icon: Icon,
        children: &'static [NavNode],
    ) -> Self {
        Self {
            key,
            label_key,
            icon: Some(icon),
            href: None,
            permission: None,
            children,
            keywords: &[],
            match_mode: Match::Prefix,
            hidden: false,
        }
    }

    pub const fn require(mut self, permission: &'static str) -> Self {
        self.permission = Some(permission);
        self
    }

    pub const fn keywords(mut self, keywords: &'static [&'static str]) -> Self {
        self.keywords = keywords;
        self
    }

    pub const fn exact(mut self) -> Self {
        self.match_mode = Match::Exact;
        self
    }

    pub const fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    pub const fn with_children(mut self, children: &'static [NavNode]) -> Self {
        self.children = children;
        self
    }

    /// What this node is called, unresolved.
    ///
    /// Deliberately not built with `msg!`: the key is a field read off a
    /// `static` rather than a literal at this call site, so the compile-time
    /// check would have nothing to look at. The test
    /// `every_node_names_itself_with_a_key_that_exists` does that job instead.
    pub fn label(&self) -> Message {
        Message::new(self.label_key)
    }

    /// Whether this node has children worth opening.
    pub const fn is_group(&self) -> bool {
        !self.children.is_empty()
    }

    /// Whether `path` selects this node itself.
    ///
    /// Prefix matches stop at a segment boundary, so `/users` does not stay lit
    /// while you are looking at `/users-archive`.
    pub fn matches(&self, path: &str) -> bool {
        let Some(href) = self.href else {
            return false;
        };

        let path = strip_trailing_slash(path);
        let href = strip_trailing_slash(href);

        match self.match_mode {
            Match::Exact => path == href,
            Match::Prefix => {
                // The byte after the prefix has to be a separator, or `/users`
                // would claim `/users-archive`. Asked for rather than indexed:
                // the `starts_with` above already implies it is there, and a
                // bounds check that can only ever succeed still reads better
                // than one the compiler would panic on.
                path == href
                    || (path.starts_with(href) && path.as_bytes().get(href.len()) == Some(&b'/'))
            }
        }
    }

    /// Whether the viewer may see this node.
    ///
    /// A group survives only if something inside it did: a heading over an empty
    /// list tells the viewer a section exists that they cannot open, which is
    /// both untidy and a small disclosure.
    pub fn visible_to(&self, user: Option<&AuthUser>) -> bool {
        let permitted = match self.permission {
            None => true,
            Some(permission) => user.is_some_and(|user| user.can(permission)),
        };

        if !permitted {
            return false;
        }

        if self.href.is_some() || self.children.is_empty() {
            return true;
        }

        self.children.iter().any(|child| child.visible_to(user))
    }

    /// The children the viewer may see.
    pub fn visible_children<'a>(
        &'a self,
        user: Option<&'a AuthUser>,
    ) -> impl Iterator<Item = &'a NavNode> + 'a {
        self.children
            .iter()
            .filter(move |child| child.visible_to(user))
    }

    /// Every node in this subtree, parents before children.
    pub fn descendants(&'static self) -> Vec<&'static NavNode> {
        let mut out = vec![self];
        for child in self.children {
            out.extend(child.descendants());
        }
        out
    }
}

/// The chain of nodes from a root down to the one the URL selects.
///
/// Empty when the URL matches nothing - a route with no menu entry, which is
/// perfectly normal for a modal or a detail page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trail {
    keys: Vec<&'static str>,
}

impl Trail {
    /// Resolve the trail for a URL path.
    ///
    /// The *deepest* match wins, not the first. With `/admin` and
    /// `/admin/users` both in the tree, visiting `/admin/users` has to select
    /// the child; a first-match walk would stop at the parent and light the
    /// wrong row.
    pub fn resolve(roots: &'static [NavNode], path: &str) -> Self {
        let mut best: Vec<&'static str> = Vec::new();
        let mut best_depth = 0usize;

        fn walk(
            nodes: &'static [NavNode],
            path: &str,
            stack: &mut Vec<&'static str>,
            best: &mut Vec<&'static str>,
            best_depth: &mut usize,
        ) {
            for node in nodes {
                stack.push(node.key);

                if node.matches(path) {
                    // Ties go to the longer route: `/admin/users/roles` beats
                    // `/admin/users` on the same URL. Depth alone is not enough,
                    // because a shallow node can carry a long route.
                    let depth = node.href.map(str::len).unwrap_or(0);
                    if depth >= *best_depth {
                        *best_depth = depth;
                        *best = stack.clone();
                    }
                }

                walk(node.children, path, stack, best, best_depth);
                stack.pop();
            }
        }

        walk(roots, path, &mut Vec::new(), &mut best, &mut best_depth);

        Self { keys: best }
    }

    /// Whether this node is the one the URL selects.
    pub fn is_current(&self, key: &str) -> bool {
        self.keys.last().is_some_and(|last| *last == key)
    }

    /// Whether this node is on the way to the selected one.
    ///
    /// True for the selected node too - "on the trail" includes its end. The
    /// sidebar uses this to decide what to expand, and a selected group has to
    /// be open or its own selected child would be hidden.
    pub fn contains(&self, key: &str) -> bool {
        self.keys.contains(&key)
    }

    /// The selected node's key, if the URL matched anything.
    pub fn current(&self) -> Option<&'static str> {
        self.keys.last().copied()
    }

    /// Root first, selected last. Drives the breadcrumb.
    pub fn keys(&self) -> &[&'static str] {
        &self.keys
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// Look a node up by key anywhere in the tree.
pub fn find(roots: &'static [NavNode], key: &str) -> Option<&'static NavNode> {
    for node in roots {
        if node.key == key {
            return Some(node);
        }
        if let Some(found) = find(node.children, key) {
            return Some(found);
        }
    }
    None
}

/// Every routable node the viewer may reach, flattened.
///
/// What the command palette searches: groups are dropped because opening one is
/// not somewhere to go, and hidden nodes are kept because being absent from the
/// sidebar is not a reason to be unfindable.
pub fn reachable(
    roots: &'static [NavNode],
    user: Option<&AuthUser>,
    catalog: &Catalog,
) -> Vec<Destination> {
    fn walk(
        nodes: &'static [NavNode],
        user: Option<&AuthUser>,
        catalog: &Catalog,
        ancestors: &mut Vec<String>,
        out: &mut Vec<Destination>,
    ) {
        for node in nodes {
            if !node.visible_to(user) {
                // Pruned with its whole subtree: a child cannot be reachable
                // through a section its holder may not enter.
                continue;
            }

            if let Some(href) = node.href {
                out.push(Destination {
                    key: node.key,
                    label: catalog.render(&node.label()),
                    icon: node.icon,
                    href,
                    keywords: node.keywords,
                    section: ancestors.first().cloned(),
                    breadcrumb: ancestors.clone(),
                });
            }

            ancestors.push(catalog.render(&node.label()));
            walk(node.children, user, catalog, ancestors, out);
            ancestors.pop();
        }
    }

    let mut out = Vec::new();
    walk(roots, user, catalog, &mut Vec::new(), &mut out);
    out
}

/// A place the command palette can send someone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub key: &'static str,
    /// Already resolved, because a destination is built for one reader in one
    /// language and the palette both shows this string and searches it.
    pub label: String,
    pub icon: Option<Icon>,
    pub href: &'static str,
    /// Extra search terms, in the language the menu was written in.
    ///
    /// Not translated, and matched *as well as* the label rather than instead
    /// of it. Somebody typing English at a French workspace - a developer, an
    /// administrator working from a support ticket - still finds the screen,
    /// and nobody typing French is any worse off for their being there.
    pub keywords: &'static [&'static str],
    /// The top-level section this sits under, for grouping results.
    pub section: Option<String>,
    /// Ancestor labels, outermost first.
    pub breadcrumb: Vec<String>,
}

impl Destination {
    /// Score this destination against a lowercase query.
    ///
    /// Higher is better; `None` means no match. The ordering is deliberate: a
    /// label that starts with what you typed should beat one that merely
    /// contains it, and both should beat a keyword hit - otherwise typing "us"
    /// puts "Users" below something that happens to list "users" as a synonym.
    pub fn score(&self, query: &str) -> Option<i32> {
        if query.is_empty() {
            return Some(0);
        }

        let label = self.label.to_lowercase();

        if label == query {
            return Some(1000);
        }
        if label.starts_with(query) {
            return Some(800 - label.len() as i32);
        }
        if label.contains(query) {
            return Some(600 - label.len() as i32);
        }

        if self
            .keywords
            .iter()
            .any(|keyword| keyword.to_lowercase().starts_with(query))
        {
            return Some(400);
        }
        if self
            .keywords
            .iter()
            .any(|keyword| keyword.to_lowercase().contains(query))
        {
            return Some(300);
        }

        // Last resort: the path through the tree. Lets "admin users" find the
        // right row even though no single field contains both words.
        let trail = self.breadcrumb.join(" ").to_lowercase();
        if trail.contains(query) {
            return Some(100);
        }

        None
    }
}

/// Search the menu, best match first.
pub fn search(
    roots: &'static [NavNode],
    user: Option<&AuthUser>,
    catalog: &Catalog,
    query: &str,
) -> Vec<Destination> {
    let query = query.trim().to_lowercase();

    let mut scored: Vec<(i32, Destination)> = reachable(roots, user, catalog)
        .into_iter()
        .filter_map(|destination| destination.score(&query).map(|score| (score, destination)))
        .collect();

    // Ties broken by label so the list does not reshuffle between renders.
    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.label.cmp(&right.label))
    });

    scored
        .into_iter()
        .map(|(_, destination)| destination)
        .collect()
}

/// `"/admin/"` and `"/admin"` are the same place; `"/"` stays `"/"`.
fn strip_trailing_slash(path: &str) -> &str {
    match path.strip_suffix('/') {
        Some("") | None => path,
        Some(trimmed) => trimmed,
    }
}

/// Every permission named by the menu, for the test below and for tooling.
pub fn declared_permissions(roots: &'static [NavNode]) -> Vec<&'static str> {
    fn walk(nodes: &'static [NavNode], out: &mut Vec<&'static str>) {
        for node in nodes {
            if let Some(permission) = node.permission {
                out.push(permission);
            }
            walk(node.children, out);
        }
    }

    let mut out = Vec::new();
    walk(roots, &mut out);
    out
}

/// Whether every permission the menu names is one this build defines.
///
/// A typo here fails *open* in the worst way: `user.can("Pages.Admin.Uzers")`
/// is false for everyone, so the item silently vanishes for the entire
/// workspace and nothing reports why.
pub fn permissions_are_defined(roots: &'static [NavNode]) -> Result<(), &'static str> {
    declared_permissions(roots)
        .into_iter()
        .find(|permission| !is_defined(permission))
        .map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use phonix_core::authorization::{PermissionSet, names};
    use phonix_core::i18n::Language;
    use phonix_core::identity::{UserId, UserStatus};

    use super::*;

    /// The words these tests read against.
    ///
    /// The fixtures below name themselves `"Home"`, `"Shipping"` and so on
    /// rather than `nav.home` - keys the catalog has never heard of. That is
    /// deliberate and it is not a shortcut: an unknown key renders as itself,
    /// so a fixture reads as the word it stands for while still going down the
    /// same code path as a real one.
    fn words() -> Catalog {
        Catalog::builtin(Language::ENGLISH)
    }

    // A tree deeper than the real menu, so the trail and expansion logic are
    // tested at the depth the sidebar has to survive rather than the depth it
    // happens to be today.
    static DEEP: &[NavNode] = &[
        NavNode::leaf("home", "Home", Icon::LayoutDashboard, "/").exact(),
        NavNode::group(
            "ops",
            "Operations",
            Icon::Boxes,
            &[
                NavNode::group(
                    "inv",
                    "Inventory",
                    Icon::Package,
                    &[
                        NavNode::leaf("items", "Items", Icon::Package, "/ops/inventory/items"),
                        NavNode::group(
                            "req",
                            "Requisitions",
                            Icon::ClipboardList,
                            &[NavNode::leaf(
                                "req-new",
                                "New requisition",
                                Icon::Plus,
                                "/ops/inventory/requisitions/new",
                            )],
                        ),
                    ],
                ),
                NavNode::leaf("ship", "Shipping", Icon::Truck, "/ops/shipping")
                    .require(names::SETTINGS),
            ],
        ),
    ];

    /// Somebody who may see everything, for the tests that are about the shape
    /// of the menu rather than about who can see it.
    ///
    /// Carries the same `cfg` as its only caller: the route-registration test
    /// needs the server's router, so under a wasm test build this would be dead
    /// code and a warning.
    #[cfg(feature = "ssr")]
    fn administrator() -> AuthUser {
        AuthUser {
            permissions: PermissionSet::all(),
            ..user(&[])
        }
    }

    fn user(permissions: &[&str]) -> AuthUser {
        AuthUser {
            id: UserId::nil(),
            email: "ada@example.com".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            display_name: "Ada Lovelace".into(),
            roles: vec!["User".into()],
            permissions: permissions.iter().copied().collect::<PermissionSet>(),
            is_owner: false,
            status: UserStatus::Active,
            mfa_enabled: false,
            mfa_satisfied: true,
            email_verified: true,
        }
    }

    #[test]
    fn the_real_menu_names_only_permissions_this_build_defines() {
        assert_eq!(permissions_are_defined(MENU), Ok(()));
    }

    #[test]
    fn the_real_menu_has_unique_keys() {
        // Expansion state and the palette's selection are both keyed by this, so
        // a duplicate would open two groups at once.
        let mut seen = BTreeSet::new();
        for node in MENU.iter().flat_map(NavNode::descendants) {
            assert!(seen.insert(node.key), "duplicate nav key {:?}", node.key);
        }
    }

    #[test]
    fn the_real_menu_routes_are_absolute() {
        for node in MENU.iter().flat_map(NavNode::descendants) {
            if let Some(href) = node.href {
                assert!(href.starts_with('/'), "{} has a relative route", node.key);
            }
        }
    }

    /// Every path the real router will register, as the server asks for it.
    ///
    /// This is the same call `phonix-server` makes at startup, so what it
    /// returns is what Axum will actually route - not what `app.rs` appears to
    /// say. The difference is not academic: a `<Routes>` placed somewhere the
    /// walk cannot reach returns an *empty* list here while every screen still
    /// renders correctly in a browser, under a 404.
    #[cfg(feature = "ssr")]
    fn registered_paths() -> BTreeSet<String> {
        leptos_axum::generate_route_list(crate::app::App)
            .into_iter()
            .map(|listing| listing.path().to_owned())
            .collect()
    }

    #[test]
    #[cfg(feature = "ssr")]
    fn every_menu_route_is_registered_in_the_router() {
        // The failure this catches is a menu entry that renders a 404: the node
        // is added here, the `<Route>` is forgotten, and nothing complains
        // until somebody clicks it.
        let registered = registered_paths();

        for destination in reachable(MENU, Some(&administrator()), &words()) {
            assert!(
                registered.contains(destination.href),
                "{} points at {}, which the router does not register. Registered: {registered:?}",
                destination.key,
                destination.href,
            );
        }
    }

    #[test]
    #[cfg(feature = "ssr")]
    fn the_router_registers_the_screens_that_are_not_on_the_menu() {
        // Three paths nothing in `MENU` links to, and each is load-bearing:
        // sign-in and sign-up are how a session starts, and the challenge is
        // where `LoginResult::next_path` sends a half-authenticated one. If
        // that last route disappears, enabling a second factor becomes a
        // lockout - which is exactly the kind of thing nobody clicks through
        // by hand before a release.
        let registered = registered_paths();

        for path in ["/", "/signup", "/auth/challenge", "/account"] {
            assert!(
                registered.contains(path),
                "{path} is not registered. Registered: {registered:?}",
            );
        }
    }

    #[test]
    fn a_group_opens_every_ancestor_of_the_selected_route() {
        // The behaviour the sidebar depends on: navigate four levels down and
        // every node above it must report itself on the trail.
        let trail = Trail::resolve(DEEP, "/ops/inventory/requisitions/new");

        assert_eq!(trail.keys(), &["ops", "inv", "req", "req-new"]);
        assert!(trail.is_current("req-new"));
        for ancestor in ["ops", "inv", "req"] {
            assert!(trail.contains(ancestor), "{ancestor} should expand");
            assert!(
                !trail.is_current(ancestor),
                "{ancestor} is not the current page"
            );
        }
    }

    #[test]
    fn the_deepest_route_wins_not_the_first() {
        // `/ops/inventory/items` is beneath `Inventory`, which has no route, and
        // beside `Requisitions`, which does. A first-match walk would stop early.
        let trail = Trail::resolve(DEEP, "/ops/inventory/items");
        assert_eq!(trail.current(), Some("items"));
    }

    #[test]
    fn a_route_off_the_menu_selects_nothing() {
        let trail = Trail::resolve(DEEP, "/somewhere/else");
        assert!(trail.is_empty());
        assert_eq!(trail.current(), None);
    }

    #[test]
    fn prefix_matching_stops_at_a_segment_boundary() {
        let shipping = find(DEEP, "ship").unwrap();

        assert!(shipping.matches("/ops/shipping"));
        assert!(shipping.matches("/ops/shipping/42"));
        // The case that makes naive `starts_with` wrong.
        assert!(!shipping.matches("/ops/shipping-labels"));
    }

    #[test]
    fn an_exact_route_does_not_swallow_the_whole_app() {
        let home = find(DEEP, "home").unwrap();

        assert!(home.matches("/"));
        assert!(!home.matches("/ops/shipping"));

        // And a trailing slash is the same place, not a different one.
        assert!(
            find(DEEP, "items")
                .unwrap()
                .matches("/ops/inventory/items/")
        );
    }

    #[test]
    fn a_node_is_hidden_from_whoever_lacks_its_permission() {
        let shipping = find(DEEP, "ship").unwrap();

        assert!(!shipping.visible_to(None));
        assert!(!shipping.visible_to(Some(&user(&[names::DASHBOARD]))));
        assert!(shipping.visible_to(Some(&user(&[names::SETTINGS]))));
    }

    #[test]
    fn a_group_disappears_once_everything_in_it_has() {
        static GATED: &[NavNode] = &[NavNode::group(
            "admin",
            "Administration",
            Icon::Shield,
            &[
                NavNode::leaf("roles", "Roles", Icon::ShieldCheck, "/admin/roles")
                    .require(names::ROLES),
            ],
        )];

        // The point: no empty "Administration" heading advertising a section
        // the viewer cannot open.
        assert!(!GATED[0].visible_to(Some(&user(&[names::DASHBOARD]))));
        assert!(GATED[0].visible_to(Some(&user(&[names::ROLES]))));
    }

    #[test]
    fn a_half_authenticated_session_sees_nothing_gated() {
        // `AuthUser::can` is false for every permission until MFA is satisfied,
        // so the menu empties itself out with no extra check here.
        let mut pending = user(&[names::SETTINGS]);
        pending.mfa_enabled = true;
        pending.mfa_satisfied = false;

        assert!(!find(DEEP, "ship").unwrap().visible_to(Some(&pending)));
    }

    #[test]
    fn the_palette_only_lists_places_you_may_go() {
        let permitted = user(&[names::SETTINGS]);
        let keys: Vec<&str> = reachable(DEEP, Some(&permitted), &words())
            .iter()
            .map(|destination| destination.key)
            .collect();

        assert!(keys.contains(&"ship"));
        // Groups are not destinations.
        assert!(!keys.contains(&"ops"));
        assert!(!keys.contains(&"inv"));

        let plain = user(&[names::DASHBOARD]);
        let keys: Vec<&str> = reachable(DEEP, Some(&plain), &words())
            .iter()
            .map(|destination| destination.key)
            .collect();
        assert!(!keys.contains(&"ship"));
    }

    #[test]
    fn search_puts_the_obvious_answer_first() {
        let permitted = user(&[names::SETTINGS]);

        let results = search(DEEP, Some(&permitted), &words(), "ship");
        assert_eq!(results.first().map(|first| first.key), Some("ship"));

        // An empty query lists everything, which is what the palette shows
        // before you type.
        assert_eq!(
            search(DEEP, Some(&permitted), &words(), "").len(),
            reachable(DEEP, Some(&permitted), &words()).len()
        );

        assert!(search(DEEP, Some(&permitted), &words(), "zzzz").is_empty());
    }

    #[test]
    fn search_finds_things_by_what_people_call_them() {
        static TAGGED: &[NavNode] = &[NavNode::leaf("users", "Users", Icon::Users, "/admin/users")
            .keywords(&["people", "staff", "invite"])];

        assert_eq!(
            search(TAGGED, None, &words(), "invite")
                .first()
                .map(|f| f.key),
            Some("users")
        );
        // And the label still outranks a synonym.
        assert!(
            search(TAGGED, None, &words(), "users")
                .first()
                .unwrap()
                .score("users")
                > search(TAGGED, None, &words(), "staff")
                    .first()
                    .unwrap()
                    .score("staff")
        );
    }

    /// Every node names itself with a key the catalog actually has.
    ///
    /// `msg!` cannot check these: the key is a field on a `static`, so there is
    /// no literal at a call site for the macro to see. Without this test a
    /// typo would ship as a sidebar row reading `nav.setings`.
    #[test]
    fn every_node_names_itself_with_a_key_that_exists() {
        fn walk(nodes: &'static [NavNode]) {
            for node in nodes {
                assert!(
                    phonix_core::i18n::catalog::builtin_contains(node.label_key),
                    "the menu node {} is labelled {}, which is not a key in \
                     crates/phonix-core/i18n/en.json",
                    node.key,
                    node.label_key,
                );
                walk(node.children);
            }
        }

        walk(MENU);
    }

    #[test]
    fn a_bad_permission_name_is_caught_rather_than_silently_hiding_a_menu() {
        static TYPO: &[NavNode] =
            &[NavNode::leaf("x", "X", Icon::Users, "/x").require("Pages.Administration.Uzers")];

        assert_eq!(
            permissions_are_defined(TYPO),
            Err("Pages.Administration.Uzers")
        );
    }
}
