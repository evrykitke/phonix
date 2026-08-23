//! The audit screen: two trails, one page.
//!
//! Read-only, and deliberately so: an audit log an administrator can edit is
//! not one. The only action on a row goes to the entry itself.
//!
//! # Why two tabs and not one table
//!
//! The two trails answer different questions and a reader is asking one of them
//! at a time:
//!
//! * **Security** - who signed in, who was locked out, who spent a recovery
//!   code. Keyed by an account, and every row has an outcome.
//! * **Changes** - who created, edited or deleted a record. Keyed by a record,
//!   and no row has an outcome because a change that failed was never written.
//!
//! Interleaving them would produce a list where half the columns are empty on
//! any given row, and where "show me the failures" and "show me the deletions"
//! are the same control meaning two things. They are also stored separately -
//! `identity_events` and `entity_events` - so one grid would be one query that
//! could not be sorted or paged. See `phonix_core::audit` for where the line is
//! drawn.
//!
//! Both grids are configurations rather than markup: everything that would have
//! been here is in [`crate::ui::table::config::audit`] and
//! [`crate::ui::table::config::changes`], because none of it is about *this
//! page*.
//!
//! Security stays the tab this screen opens on. It is the older trail and the
//! one an incident sends somebody to, and the `?tab=` value keeps a bookmark
//! pointed wherever it was pointed.

use leptos::prelude::*;
use leptos_meta::Title;

use crate::components::page::PageHeader;
use crate::icons::Icon;
use crate::l;
use crate::ui::table::DataGrid;
use crate::ui::table::config::audit::audit_grid;
use crate::ui::table::config::changes::changes_grid;
use crate::ui::tabs::{Tab, TabbedPanel};

#[component]
pub fn audit_logs_page() -> impl IntoView {
    let tabs = vec![
        Tab::new("security", "Security", || {
            view! { <DataGrid config=audit_grid() /> }.into_any()
        })
        .icon(Icon::Shield),
        Tab::new("changes", "Changes", || {
            view! { <DataGrid config=changes_grid() /> }.into_any()
        })
        .icon(Icon::ClipboardList),
    ];

    view! {
        <Title text=format!("{} | Phonix", l!("audit.title")) />

        <PageHeader
            title=l!("audit.title")
            subtitle=l!("audit.subtitle")
            icon=Icon::ScrollText
        />

        <TabbedPanel id="audit" tabs=tabs />
    }
}
