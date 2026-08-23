//! The people in this workspace.
//!
//! The screen is a heading and a grid. Everything that used to be here - the
//! filter box, the table, the badges, the empty state - is now a configuration
//! in [`crate::ui::table::config::users`], because none of it was about *this
//! page* and all of it will be wanted again by the next list.

use leptos::prelude::*;
use leptos_meta::Title;

use crate::components::page::PageHeader;
use crate::icons::Icon;
use crate::l;
use crate::ui::table::DataGrid;
use crate::ui::table::config::users::users_grid;

#[component]
pub fn users_page() -> impl IntoView {
    view! {
        <Title text=format!("{} | Phonix", l!("users.title")) />

        <PageHeader
            title=l!("users.title")
            subtitle=l!("users.subtitle")
            icon=Icon::Users
        />

        <DataGrid config=users_grid() />
    }
}
