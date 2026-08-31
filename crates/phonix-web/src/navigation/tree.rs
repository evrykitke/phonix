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
    // Sales before master data: raising an invoice is the daily work, and
    // keeping the customer list tidy is what somebody does on the way to it.
    NavNode::group(
        "sales",
        "nav.sales",
        Icon::ShoppingCart,
        &[
            // First in each app's group: the group heading opens and closes,
            // it does not navigate, so without this an app's own front page
            // is reachable from the launcher and from nowhere in the menu.
            NavNode::leaf("sales-overview", "nav.overview", Icon::LayoutGrid, "/sales")
                .require(names::SALES)
                .keywords(&["books", "home", "start"]),
            NavNode::leaf(
                "invoices",
                "nav.invoices",
                Icon::FileText,
                "/sales/invoices",
            )
            .require(names::INVOICES)
            .keywords(&["bill", "billing", "receivable", "sales", "customer"]),
        ],
    )
    .require(names::SALES),
    NavNode::group(
        "master",
        "nav.master",
        Icon::Boxes,
        &[
            NavNode::leaf(
                "master-overview",
                "nav.overview",
                Icon::LayoutGrid,
                "/master",
            )
            .require(names::MASTER)
            .keywords(&["home", "start"]),
            NavNode::leaf("parties", "nav.parties", Icon::Users, "/master/parties")
                .require(names::PARTIES)
                .keywords(&["customers", "suppliers", "clients", "vendors", "contacts"]),
            NavNode::leaf("taxes", "nav.taxes", Icon::Receipt, "/master/taxes")
                .require(names::TAXES)
                .keywords(&["vat", "gst", "sales tax", "rates", "groups"]),
        ],
    )
    .require(names::MASTER),
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
            NavNode::leaf("apps", "nav.apps", Icon::Blocks, "/admin/apps")
                .require(names::APPS)
                .keywords(&["install", "modules", "subscription", "books", "store"]),
            NavNode::leaf(
                "api-keys",
                "nav.api_keys",
                Icon::KeySquare,
                "/admin/api-keys",
            )
            .require(names::API_KEYS)
            .keywords(&["api", "integration", "token", "credentials", "developer"]),
            NavNode::leaf(
                "audit-logs",
                "nav.audit_logs",
                Icon::ScrollText,
                "/admin/audit-logs",
            )
            .require(names::AUDIT_LOGS)
            .keywords(&["history", "activity", "trail", "who did what"]),
            // Last, and gated on a permission nobody is given by default: it
            // is a developer reference that happens to live in the shipped
            // binary, not something a workspace bought.
            NavNode::leaf("ui-library", "nav.ui_library", Icon::Palette, "/admin/ui")
                .require(names::UI_LIBRARY)
                .keywords(&["components", "kit", "design", "showcase", "widgets"]),
        ],
    )
    .require(names::ADMINISTRATION),
];
