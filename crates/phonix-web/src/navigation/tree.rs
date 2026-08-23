//! The menu itself.
//!
//! # Adding a screen
//!
//! One node, in one place:
//!
//! ```ignore
//! NavNode::leaf("users", "nav.users", Icon::Users, "/admin/users")
//!     .require(names::USERS)
//!     .keywords(&["people", "accounts", "staff"])
//! ```
//!
//! The sidebar, the command palette and the breadcrumb all pick it up from
//! there. Three rules the tests enforce, so they are worth knowing before the
//! test tells you:
//!
//! * `key` is unique across the whole tree - expansion state is keyed by it.
//! * `href` is absolute, because the sidebar renders the same link from every
//!   route.
//! * `require` names a constant from [`phonix_core::authorization::names`], not
//!   a literal. A permission this build does not define fails the test rather
//!   than quietly hiding a menu nobody can explain the absence of.
//!
//! # Adding a module
//!
//! An inventory module is a group under a new top-level node, and the
//! permissions it names have to exist in `phonix-core` first - the definition
//! tree there is the source of truth, this is a view of it. Declare
//! `Pages.Inventory`, `Pages.Inventory.Items`, `Pages.Inventory.Requisitions`
//! and so on in `authorization::definitions`, then:
//!
//! ```ignore
//! NavNode::group("inventory", "nav.inventory", Icon::Boxes, &[
//!     NavNode::leaf("items", "nav.items", Icon::Package, "/inventory/items")
//!         .require(names::INVENTORY_ITEMS),
//! ])
//! .require(names::INVENTORY)
//! ```
//!
//! Depth is free: a group inside a group inside a group highlights and expands
//! the same way, because [`Trail`](super::Trail) walks the tree rather than
//! knowing how tall it is.

use phonix_core::authorization::names;

use super::NavNode;
use crate::icons::Icon;

/// Where a finished sign-in lands, and the first entry in the menu.
pub const DASHBOARD: &str = "/dashboard";

/// The workspace menu, top to bottom.
///
/// Order here is order on screen. Nothing sorts it: the sequence is a design
/// decision - what people reach for most, first - and an alphabetical sidebar
/// would bury the dashboard under "Audit logs".
pub static MENU: &[NavNode] = &[
    NavNode::leaf(
        "dashboard",
        "nav.dashboard",
        Icon::LayoutDashboard,
        DASHBOARD,
    )
    .require(names::DASHBOARD)
    .keywords(&["home", "overview", "start"]),
    NavNode::group(
        "administration",
        "nav.administration",
        Icon::SlidersHorizontal,
        &[
            NavNode::leaf("users", "nav.users", Icon::Users, "/admin/users")
                .require(names::USERS)
                .keywords(&["people", "accounts", "staff", "members", "invite"]),
            NavNode::leaf("roles", "nav.roles", Icon::ShieldCheck, "/admin/roles")
                .require(names::ROLES)
                .keywords(&["permissions", "access", "groups"]),
            NavNode::leaf(
                "settings",
                "nav.settings",
                Icon::Settings,
                "/admin/settings",
            )
            .require(names::SETTINGS)
            .keywords(&["workspace", "configuration", "preferences", "tenant"]),
            NavNode::leaf(
                "audit-logs",
                "nav.audit_logs",
                Icon::ScrollText,
                "/admin/audit-logs",
            )
            .require(names::AUDIT_LOGS)
            .keywords(&["history", "activity", "trail", "who did what"]),
        ],
    )
    .require(names::ADMINISTRATION),
];
