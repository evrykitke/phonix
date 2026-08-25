//! Master data's front page: who this workspace trades with, and what it
//! charges them.
//!
//! # Roles, not separate lists
//!
//! One organization is often both a customer and a supplier, and counting them
//! twice is right - the two numbers answer two different questions and are not
//! meant to add up to the total. That is the same reason the parties screen has
//! one grid with a role filter rather than a customers page and a suppliers
//! page; see `phonix_master::party`.

use leptos::prelude::*;
use phonix_core::apps;
use phonix_core::i18n::Message;
use phonix_core::permissions;
use phonix_master::party::roles;

use crate::components::app_home::{AppHome, Shortcut, Stat};
use crate::i18n::t;
use crate::icons::Icon;
use crate::server_fns::master_fns::{list_parties, list_tax_codes_today};

#[component]
pub fn master_home_page() -> impl IntoView {
    // Both unfiltered, and counted in the browser - the same bargain the grids
    // on these screens already make. See the sales home for when that stops
    // being the right one.
    let parties = Resource::new(|| (), |()| async move { list_parties(None).await.ok() });
    let taxes = Resource::new(|| (), |()| async move { list_tax_codes_today().await.ok() });

    let stats = Signal::derive(move || {
        let (Some(Some(parties)), Some(Some(taxes))) = (parties.get(), taxes.get()) else {
            return Vec::new();
        };

        // `PartySummary` carries the roles rather than answering about them -
        // it is a row, not the record - so the question is asked here.
        let with_role = |role: &str| {
            parties
                .iter()
                .filter(|party| party.roles.iter().any(|held| held.as_str() == role))
                .count()
        };

        vec![
            Stat::new(t(&Message::new("master.home.parties")), parties.len()),
            Stat::new(
                t(&Message::new("master.home.customers")),
                with_role(roles::CUSTOMER),
            ),
            Stat::new(
                t(&Message::new("master.home.suppliers")),
                with_role(roles::SUPPLIER),
            ),
            Stat::new(t(&Message::new("master.home.taxes")), taxes.len()),
        ]
    });

    #[allow(
        clippy::expect_used,
        reason = "the catalog is a compiled constant and a test asserts master is in it"
    )]
    let app = apps::find(apps::MASTER).expect("master is in the catalog");

    let shortcuts = vec![
        Shortcut::new(
            t(&Message::new("nav.parties")),
            t(&Message::new("master.home.parties_detail")),
            "/master/parties",
            Icon::Users,
        )
        .require(permissions::PARTIES)
        .primary(),
        Shortcut::new(
            t(&Message::new("nav.taxes")),
            t(&Message::new("master.home.taxes_detail")),
            "/master/taxes",
            Icon::Receipt,
        )
        .require(permissions::TAXES),
        Shortcut::new(
            t(&Message::new("master.home.add_party")),
            t(&Message::new("master.home.add_party_detail")),
            "/master/parties/new",
            Icon::Plus,
        )
        .require(permissions::PARTIES_CREATE),
    ];

    view! {
        <Suspense fallback=|| view! { <div class="h-48" /> }>
            <AppHome app=app stats=stats shortcuts=shortcuts.clone() />
        </Suspense>
    }
}
