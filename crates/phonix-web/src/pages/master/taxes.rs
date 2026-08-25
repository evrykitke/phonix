//! The tax screen: what is charged, and what applies together.
//!
//! Two grids under two tabs rather than two screens in the menu. A tax and a
//! group are two halves of one answer - a tax with no group cannot reach a
//! document, and a group is nothing but an ordered list of taxes - so somebody
//! setting a workspace up moves between them constantly, and two menu entries
//! would be two places to be.

use leptos::prelude::*;
use leptos_meta::Title;
use phonix_tax::code::TaxCodeInput;

use crate::components::page::{PageHeader, Panel};
use crate::icons::Icon;
use crate::l;
use crate::ui::form::EntityForm;
use crate::ui::form::config::taxes::tax_code_form;
use crate::ui::table::DataGrid;
use crate::ui::table::config::taxes::{tax_groups_grid, taxes_grid};
use crate::ui::tabs::{Tab, TabbedPanel};

#[component]
pub fn taxes_page() -> impl IntoView {
    // Built outside the tab closures. A render closure runs again every time
    // its tab comes back on screen, and a grid rebuilt on every visit is a grid
    // that loses its sort, its filter and its scroll position.
    let taxes_tab = Tab::new("taxes", "Taxes", || {
        view! { <DataGrid config=taxes_grid() /> }.into_any()
    })
    .icon(Icon::Receipt);

    let groups_tab = Tab::new("groups", "Groups", || {
        view! {
            <div class="space-y-3">
                <p class="text-sm text-content-muted">{l!("tax_groups.subtitle")}</p>
                <DataGrid config=tax_groups_grid() />
            </div>
        }
        .into_any()
    })
    .icon(Icon::ListTree);

    view! {
        // "Phonix" is the product's name, not a word.
        <Title text=format!("{} | Phonix", l!("taxes.title")) />

        <PageHeader
            title=l!("taxes.title")
            subtitle=l!("taxes.subtitle")
            icon=Icon::Receipt
        />

        <TabbedPanel id="taxes" tabs=vec![taxes_tab, groups_tab] />
    }
}

/// Defining a tax.
///
/// A page rather than a dialog, for the reason the role screen is: what happens
/// next is a second step - giving it a rate, without which it charges nothing
/// and refuses every document - and a dialog that closes onto a list has
/// nowhere to send somebody.
#[component]
pub fn tax_new_page() -> impl IntoView {
    view! {
        <Title text=format!("{} | Phonix", l!("taxes.new")) />

        <PageHeader
            title=l!("taxes.new")
            subtitle=l!("taxes.new.subtitle")
            icon=Icon::Receipt
            back=("/master/taxes", l!("taxes.title"))
        />

        <div class="max-w-3xl">
            <Panel>
                <EntityForm config=tax_code_form() value=TaxCodeInput::blank() />
            </Panel>
        </div>
    }
}
