//! The party list, and the screen that adds one.

use leptos::prelude::*;
use leptos_meta::Title;
use phonix_master::party::PartyInput;

use crate::components::page::{PageHeader, Panel};
use crate::icons::Icon;
use crate::l;
use crate::server_fns::currency_fns::enabled_currencies;
use crate::server_fns::master_fns::list_tax_groups;
use crate::ui::form::EntityForm;
use crate::ui::form::config::parties::party_form;
use crate::ui::table::DataGrid;
use crate::ui::table::config::parties::parties_grid;

#[component]
pub fn parties_page() -> impl IntoView {
    view! {
        // "Phonix" is the product's name, not a word.
        <Title text=format!("{} | Phonix", l!("parties.title")) />

        <PageHeader
            title=l!("parties.title")
            subtitle=l!("parties.subtitle")
            icon=Icon::Users
        />

        <DataGrid config=parties_grid() />
    }
}

/// Adding a party.
///
/// A page rather than a dialog over the list, for the reason the invite screen
/// is: what happens next is a second step - an address, a contact - and a
/// dialog that closes onto a list has nowhere to send somebody.
#[component]
pub fn party_new_page() -> impl IntoView {
    // Two resources rather than one call that fetches both: a resource starts
    // as soon as it is created, so these are already in flight together and
    // awaiting them in the same block is the join.
    //
    // A form is a poor place to discover that a workspace has no tax groups
    // yet: the picker is simply empty, which is the correct answer.
    let groups = Resource::new(|| (), |()| async move { list_tax_groups().await });
    let currencies = Resource::new(|| (), |()| async move { enabled_currencies().await });

    view! {
        <Title text=format!("{} | Phonix", l!("parties.new.title")) />

        <PageHeader
            title=l!("parties.new.title")
            subtitle=l!("parties.new.subtitle")
            icon=Icon::Users
            back=("/master/parties", l!("parties.title"))
        />

        // A single-column form, so the card ends where it ends rather than
        // stretching a name and a website across a wide monitor.
        <div class="max-w-3xl">
            <Panel>
                // Transition rather than Suspense: the fallback replaces
                // nothing here, but navigating away and back must not blank a
                // form somebody is filling in.
                <Transition fallback=|| {
                    view! { <p class="text-sm text-content-subtle">{l!("common.loading")}</p> }
                }>
                    {move || Suspend::new(async move {
                        // A failed fetch is an empty picker rather than a
                        // failed screen: both of these are optional fields, and
                        // refusing to let anybody add a customer because the
                        // tax table would not load is the wrong trade.
                        let groups = groups.await.unwrap_or_default();
                        let currencies = currencies.await.unwrap_or_default();

                        view! {
                            <EntityForm
                                config=party_form(groups, currencies)
                                value=PartyInput::blank()
                            />
                        }
                    })}
                </Transition>
            </Panel>
        </div>
    }
}
