//! The invoice list.

use leptos::prelude::*;
use leptos_meta::Title;

use crate::components::page::PageHeader;
use crate::icons::Icon;
use crate::l;
use crate::ui::table::DataGrid;
use crate::ui::table::config::invoices::invoices_grid;

#[component]
pub fn invoices_page() -> impl IntoView {
    view! {
        // "Phonix" is the product's name, not a word.
        <Title text=format!("{} | Phonix", l!("invoices.title")) />

        <PageHeader
            title=l!("invoices.title")
            subtitle=l!("invoices.subtitle")
            icon=Icon::FileText
        />

        <DataGrid config=invoices_grid() />
    }
}
