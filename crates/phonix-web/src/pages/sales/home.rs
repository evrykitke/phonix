//! Books' front page: how the invoices stand, and the way in to them.
//!
//! # The counts come from the list endpoint
//!
//! Not from a purpose-built `invoice_counts` server function. The invoices grid
//! already loads every row into the browser - see
//! [`crate::ui::table::config::invoices`] - so a workspace at a scale where
//! that is fine is one where this is fine too, and a second endpoint would be a
//! second thing to keep in step with the first.
//!
//! When the grid needs a server-side source, so will this, and they should get
//! one together.

use leptos::prelude::*;
use phonix_core::apps;
use phonix_core::i18n::Message;
use phonix_core::permissions;

use crate::components::app_home::{AppHome, Shortcut, Stat};
use crate::i18n::t;
use crate::icons::Icon;
use crate::server_fns::books_fns::{InvoiceQuery, list_invoices};

#[component]
pub fn sales_home_page() -> impl IntoView {
    let invoices = Resource::new(
        || (),
        |()| async move { list_invoices(InvoiceQuery::default()).await.ok() },
    );

    // Counts of *states*, never of periods. A figure that reads the clock can
    // differ between the server's render and the browser's, and near midnight
    // that is a hydration mismatch - see the component's module docs.
    let stats = Signal::derive(move || {
        let Some(Some(rows)) = invoices.get() else {
            return Vec::new();
        };

        let count = |wanted: &str| {
            rows.iter()
                .filter(|row| row.status.as_str() == wanted)
                .count()
        };

        vec![
            Stat::new(t(&Message::new("books.home.total")), rows.len()),
            Stat::new(t(&Message::new("books.status.draft")), count("draft")),
            Stat::new(t(&Message::new("books.status.posted")), count("posted")),
            Stat::new(t(&Message::new("books.status.voided")), count("voided")),
        ]
    });

    #[allow(
        clippy::expect_used,
        reason = "the catalog is a compiled constant and a test asserts Books is in it"
    )]
    let app = apps::find(apps::BOOKS).expect("books is in the catalog");

    let shortcuts = vec![
        Shortcut::new(
            t(&Message::new("invoices.new")),
            t(&Message::new("books.home.new_detail")),
            "/sales/invoices/new",
            Icon::Plus,
        )
        .require(permissions::INVOICES_CREATE)
        .primary(),
        Shortcut::new(
            t(&Message::new("nav.invoices")),
            t(&Message::new("books.home.list_detail")),
            "/sales/invoices",
            Icon::FileText,
        )
        .require(permissions::INVOICES),
        // Books' own screens are only half of what somebody here needs: an
        // invoice cannot be raised without a customer, and the customer lives
        // in master data. Linking across the boundary is a *link*, which is
        // what the boundary permits - Books still holds no code of master's.
        Shortcut::new(
            t(&Message::new("nav.parties")),
            t(&Message::new("books.home.customers_detail")),
            "/master/parties",
            Icon::Users,
        )
        .require(permissions::PARTIES),
    ];

    view! {
        <Suspense fallback=|| view! { <div class="h-48" /> }>
            <AppHome app=app stats=stats shortcuts=shortcuts.clone() />
        </Suspense>
    }
}
