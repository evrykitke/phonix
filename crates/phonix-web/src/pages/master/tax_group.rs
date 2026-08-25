//! A tax group: which taxes apply together, and in what order.
//!
//! One screen for both creating and editing, because there is nothing to a
//! group beyond the form - no rate history, no children. The list it was opened
//! from is on the taxes screen's second tab.

use leptos::prelude::*;
use leptos_meta::Title;
use phonix_core::audit::kinds;
use phonix_core::permissions;
use phonix_tax::group::TaxGroupInput;
use uuid::Uuid;

use crate::components::history::RecordHistory;
use crate::components::page::{Badge, Notice, PageHeader, Panel, Tone};
use crate::icons::Icon;
use crate::l;
use crate::server_fns::master_fns::{list_tax_codes, tax_group_edit};
use crate::ui::form::EntityForm;
use crate::ui::form::config::taxes::tax_group_form;
use crate::ui::tabs::{Tab, TabbedPanel};

/// Defining a group.
#[component]
pub fn tax_group_new_page() -> impl IntoView {
    let codes = Resource::new(|| (), |()| async move { list_tax_codes().await });

    view! {
        // "Phonix" is the product's name, not a word.
        <Title text=format!("{} | Phonix", l!("tax_groups.new")) />

        <PageHeader
            title=l!("tax_groups.new")
            subtitle=l!("tax_groups.new.subtitle")
            icon=Icon::ListTree
            back=("/master/taxes?tab=groups", l!("tax_groups.title"))
        />

        <div class="max-w-3xl">
            <Panel>
                <Transition fallback=|| {
                    view! { <p class="text-sm text-content-subtle">{l!("common.loading")}</p> }
                }>
                    {move || Suspend::new(async move {
                        // An empty picker rather than a failed screen. A
                        // workspace with no taxes yet gets a form that cannot
                        // be submitted, which is the honest state of affairs -
                        // a group needs at least one tax in it.
                        let codes = codes.await.unwrap_or_default();

                        view! {
                            <EntityForm
                                config=tax_group_form(codes)
                                value=TaxGroupInput::blank()
                            />
                        }
                    })}
                </Transition>
            </Panel>
        </div>
    }
}

/// One group.
#[component]
pub fn tax_group_page() -> impl IntoView {
    let params = leptos_router::hooks::use_params_map();
    let group_id = move || params.with(|params| params.get("id").unwrap_or_default());

    // Two resources rather than one call that fetches both: a resource starts
    // as soon as it is created, so these are already in flight together and
    // awaiting them in the same block is the join.
    let group = Resource::new(group_id, |raw| async move {
        match raw.parse::<Uuid>() {
            Ok(id) => tax_group_edit(id).await,
            Err(_) => Err(ServerFnError::new("That is not a tax group id.")),
        }
    });
    let codes = Resource::new(|| (), |()| async move { list_tax_codes().await });

    view! {
        <Title text=format!("{} | Phonix", l!("entity.tax_group.singular")) />

        <Transition fallback=|| {
            view! { <p class="text-sm text-content-subtle">{l!("common.loading")}</p> }
        }>
            {move || Suspend::new(async move {
                match group.await {
                    Ok(draft) => {
                        let codes = codes.await.unwrap_or_default();
                        view! { <GroupEditor draft=draft codes=codes /> }.into_any()
                    }
                    Err(err) => {
                        view! {
                            <>
                                <PageHeader
                                    title=l!("entity.tax_group.singular")
                                    icon=Icon::ListTree
                                    back=("/master/taxes?tab=groups", l!("tax_groups.title"))
                                />
                                <Notice
                                    message=Signal::derive(move || Some(err.to_string()))
                                    tone=Tone::Danger
                                />
                            </>
                        }
                            .into_any()
                    }
                }
            })}
        </Transition>
    }
}

#[component]
fn group_editor(draft: TaxGroupInput, codes: Vec<phonix_tax::code::TaxCode>) -> impl IntoView {
    // A group opened for editing always has an id: `tax_group_edit` reads a
    // stored row.
    let group_id = draft.id.unwrap_or_else(Uuid::nil);
    let title = draft.name.clone();
    let code = draft.code.clone();
    let is_active = draft.is_active;
    // `i64`, because that is what a plural message counts in.
    let member_count = draft.members.len() as i64;

    // A tab's render closure runs again on every visit, so what it needs has to
    // survive being read more than once.
    let opening = StoredValue::new((draft, codes));

    let details_tab = Tab::new("details", "Details", move || {
        let (draft, codes) = opening.get_value();

        view! {
            <div class="max-w-3xl">
                <Panel>
                    <EntityForm config=tax_group_form(codes) value=draft />
                </Panel>
            </div>
        }
        .into_any()
    })
    .icon(Icon::SlidersHorizontal);

    // Its own tab rather than a panel below, for the reason every other
    // history on this application is: a section stacked under a form is one
    // nobody scrolls to.
    let history_tab = Tab::new("history", "History", move || {
        view! { <RecordHistory kind=kinds::TAX_GROUP id=Some(group_id.to_string()) /> }.into_any()
    })
    .icon(Icon::Clock)
    .require(permissions::AUDIT_LOGS);

    view! {
        <PageHeader
            title=title
            icon=Icon::ListTree
            back=("/master/taxes?tab=groups", l!("tax_groups.title"))
        >
            <div class="flex flex-wrap items-center gap-1.5">
                <Badge label=code />
                <Badge label=crate::lp!("tax_groups.member_count", member_count) />
                {(!is_active).then(|| view! { <Badge label=l!("common.inactive") /> })}
            </div>
        </PageHeader>

        <TabbedPanel id="tax-group" tabs=vec![details_tab, history_tab] />
    }
}
