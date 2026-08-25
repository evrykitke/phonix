//! One party: who they are, where to bill them, who to write to.
//!
//! # Why this has tabs
//!
//! A party is three things edited by three different people at three different
//! moments. Accounts corrects a tax registration. Sales adds the contact who
//! signs the order. Dispatch adds a delivery address. Putting all of it on one
//! page would mean one save button for three decisions, and a stale draft
//! reverting somebody else's correction as a side effect.
//!
//! Each tab therefore has its own save, and the address and contact panels
//! write their own rows.
//!
//! # The address editor is one form, re-opened
//!
//! Rather than an editable form per card. A form per row is N drafts on screen,
//! N submit buttons, and N chances to save the wrong one; one form that opens
//! on whichever address was chosen is a single place for the draft to live.

use leptos::prelude::*;
use leptos_meta::Title;
use phonix_core::audit::kinds;
use phonix_core::permissions;
use phonix_master::address::{PartyAddress, PartyAddressInput};
use phonix_master::contact::{PartyContact, PartyContactInput};
use phonix_master::party::{Party, PartyInput};
use uuid::Uuid;

use crate::components::history::RecordHistory;
use crate::components::page::{Badge, GhostButton, Notice, PageHeader, Panel, Tone};
use crate::icons::Icon;
use crate::l;
use crate::server_fns::currency_fns::enabled_currencies;
use crate::server_fns::master_fns::{
    delete_party_address, delete_party_contact, list_tax_groups, party_detail,
};
use crate::ui::alert::{Alert, Alerts};
use crate::ui::form::config::parties::party_form;
use crate::ui::form::{EntityForm, FormHost};
use crate::ui::tabs::{Tab, TabbedPanel};

#[component]
pub fn party_page() -> impl IntoView {
    let params = leptos_router::hooks::use_params_map();
    let party_id = move || params.with(|params| params.get("id").unwrap_or_default());

    // Three resources rather than one call that fetches all of them: a resource
    // starts as soon as it is created, so these are already in flight together
    // and awaiting them in the same block is the join.
    let party = Resource::new(party_id, |raw| async move {
        match raw.parse::<Uuid>() {
            Ok(id) => party_detail(id).await,
            Err(_) => Err(ServerFnError::new("That is not a party id.")),
        }
    });
    let groups = Resource::new(|| (), |()| async move { list_tax_groups().await });
    let currencies = Resource::new(|| (), |()| async move { enabled_currencies().await });

    view! {
        <Title text=format!("{} | Phonix", l!("entity.party.singular")) />

        // Transition, not Suspense: navigating from one party to another
        // re-suspends, and a fallback here would blank a screen somebody is
        // looking at rather than replacing it when the next one arrives.
        <Transition fallback=|| {
            view! { <p class="text-sm text-content-subtle">{l!("common.loading")}</p> }
        }>
            {move || Suspend::new(async move {
                match party.await {
                    Ok(party) => {
                        // A failed picker is an empty picker, not a failed
                        // screen: neither field is required.
                        let groups = groups.await.unwrap_or_default();
                        let currencies = currencies.await.unwrap_or_default();

                        view! {
                            <PartyEditor
                                party=party
                                groups=groups
                                currencies=currencies
                            />
                        }
                            .into_any()
                    }
                    // The server's own words say more than a house phrase.
                    Err(err) => {
                        view! {
                            <>
                                <PageHeader
                                    title=l!("entity.party.singular")
                                    icon=Icon::Users
                                    back=("/master/parties", l!("parties.title"))
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
fn party_editor(
    party: Party,
    groups: Vec<phonix_tax::group::TaxGroup>,
    currencies: Vec<phonix_core::locale::Currency>,
) -> impl IntoView {
    let party_id = party.id;
    let title = party.name.clone();
    let code = party.code.clone();
    let is_active = party.is_active;
    let held: Vec<String> = party
        .roles
        .iter()
        .map(|role| role.as_str().to_owned())
        .collect();

    // Hoisted above the tab strip rather than created inside the tab that uses
    // them. A render closure runs again every time its tab comes back on
    // screen, so state declared inside one is state thrown away by looking at
    // another tab - which here would be an address somebody was halfway
    // through typing.
    let details = RwSignal::new(PartyInput::from_party(&party));
    let addresses = RwSignal::new(party.addresses.clone());
    let contacts = RwSignal::new(party.contacts.clone());

    // What every save on this screen runs. Re-read rather than the draft echoed
    // back: the database trims and normalises, and a screen showing the draft
    // would show a change that did not happen.
    let refresh = Callback::new(move |()| {
        leptos::task::spawn_local(async move {
            if let Ok(fresh) = party_detail(party_id).await {
                details.set(PartyInput::from_party(&fresh));
                addresses.set(fresh.addresses);
                contacts.set(fresh.contacts);
            }
        });
    });

    // A tab's render closure runs again every time that tab comes back on
    // screen, so the pickers it needs have to survive being read more than
    // once. `StoredValue` is what holds an owned value a `Fn` may re-read.
    let pickers = StoredValue::new((groups, currencies));

    let details_tab = Tab::new("details", "Details", move || {
        let host = FormHost {
            refresh: Some(refresh),
            close: None,
        };
        let (groups, currencies) = pickers.get_value();

        view! {
            <div class="max-w-3xl">
                <Panel>
                    <EntityForm
                        config=party_form(groups, currencies)
                        value=details.get_untracked()
                        host=host
                    />
                </Panel>
            </div>
        }
        .into_any()
    })
    .icon(Icon::SlidersHorizontal);

    let addresses_tab = Tab::new("addresses", "Addresses", move || {
        view! { <AddressPanel party_id=party_id addresses=addresses refresh=refresh /> }.into_any()
    })
    .icon(Icon::Building2);

    let contacts_tab = Tab::new("contacts", "Contacts", move || {
        view! { <ContactPanel party_id=party_id contacts=contacts refresh=refresh /> }.into_any()
    })
    .icon(Icon::User);

    // A tab rather than a fourth panel stacked below. The page is already three
    // panels tall; a history under them is a section nobody scrolls to.
    let history_tab = Tab::new("history", "History", move || {
        view! { <RecordHistory kind=kinds::PARTY id=Some(party_id.to_string()) /> }.into_any()
    })
    .icon(Icon::Clock)
    .require(permissions::AUDIT_LOGS);

    view! {
        <PageHeader
            title=title
            icon=Icon::Users
            back=("/master/parties", l!("parties.title"))
        >
            <div class="flex flex-wrap items-center gap-1.5">
                <Badge label=code />
                {held
                    .into_iter()
                    .map(|role| view! { <Badge label=role tone=Tone::Brand /> })
                    .collect_view()}
                {(!is_active).then(|| view! { <Badge label=l!("common.inactive") /> })}
            </div>
        </PageHeader>

        <TabbedPanel
            id="party"
            tabs=vec![details_tab, addresses_tab, contacts_tab, history_tab]
        />
    }
}

/// The addresses on a party, and one form that opens on whichever is chosen.
#[component]
fn address_panel(
    party_id: Uuid,
    addresses: RwSignal<Vec<PartyAddress>>,
    refresh: Callback<()>,
) -> impl IntoView {
    // `None` means the form is closed. `Some(draft)` is the one being edited,
    // which is either a blank for adding or a copy of a stored row.
    let editing: RwSignal<Option<PartyAddressInput>> = RwSignal::new(None);
    let alerts = Alerts::get();

    let remove = move |address_id: Uuid| {
        leptos::task::spawn_local(async move {
            match delete_party_address(party_id, address_id).await {
                Ok(()) => refresh.run(()),
                Err(err) => alerts.post(Alert::failure(err.to_string())),
            }
        });
    };

    view! {
        <div class="space-y-3">
            <Panel
                title=l!("parties.addresses")
                description=l!("parties.address.help")
            >
                <div class="space-y-2">
                    <Show
                        when=move || !addresses.get().is_empty()
                        fallback=move || {
                            view! {
                                <p class="text-sm text-content-subtle">
                                    {l!("parties.address.none")}
                                </p>
                            }
                        }
                    >
                        <ul class="space-y-2">
                            {move || {
                                addresses
                                    .get()
                                    .into_iter()
                                    .map(|address| {
                                        let id = address.id;
                                        let draft = PartyAddressInput::from_address(&address);
                                        let lines = address.address.lines();
                                        let purpose = crate::i18n::t(&address.purpose.label());
                                        let label = address.label.clone();
                                        let is_primary = address.is_primary;

                                        view! {
                                            <li class="flex items-start justify-between gap-3 rounded-card border border-edge p-3">
                                                <div class="min-w-0 space-y-1">
                                                    <div class="flex flex-wrap items-center gap-1.5">
                                                        <Badge label=purpose tone=Tone::Brand />
                                                        {is_primary
                                                            .then(|| {
                                                                view! {
                                                                    <Badge label=l!("field.primary") />
                                                                }
                                                            })}
                                                        {label
                                                            .map(|label| {
                                                                view! {
                                                                    <span class="text-xs text-content-muted">
                                                                        {label}
                                                                    </span>
                                                                }
                                                            })}
                                                    </div>
                                                    <div class="text-sm text-content">
                                                        {lines
                                                            .into_iter()
                                                            .map(|line| view! { <div>{line}</div> })
                                                            .collect_view()}
                                                    </div>
                                                </div>
                                                <div class="flex shrink-0 items-center gap-1.5">
                                                    <GhostButton
                                                        label=l!("common.edit")
                                                        icon=Icon::Pencil
                                                        on_click=Callback::new(move |()| {
                                                            editing.set(Some(draft.clone()));
                                                        })
                                                    />
                                                    <GhostButton
                                                        label=l!("common.remove")
                                                        icon=Icon::Trash2
                                                        on_click=Callback::new(move |()| remove(id))
                                                    />
                                                </div>
                                            </li>
                                        }
                                    })
                                    .collect_view()
                            }}
                        </ul>
                    </Show>

                    <GhostButton
                        label=l!("parties.address.new")
                        icon=Icon::Plus
                        on_click=Callback::new(move |()| {
                            editing.set(Some(PartyAddressInput::blank()));
                        })
                    />
                </div>
            </Panel>

            // The form is created fresh each time `editing` changes, which is
            // what makes it re-seed: `EntityForm` takes its opening value once,
            // so re-opening on a different address means a new form.
            {move || {
                editing
                    .get()
                    .map(|draft| {
                        let host = FormHost {
                            refresh: Some(refresh),
                            close: Some(Callback::new(move |()| editing.set(None))),
                        };

                        view! {
                            <div class="max-w-3xl">
                                <Panel>
                                    <EntityForm
                                        config=crate::ui::form::config::parties::party_address_form(
                                            party_id,
                                        )
                                        value=draft
                                        host=host
                                    />
                                </Panel>
                            </div>
                        }
                    })
            }}
        </div>
    }
}

/// The people at a party, and one form that opens on whichever is chosen.
#[component]
fn contact_panel(
    party_id: Uuid,
    contacts: RwSignal<Vec<PartyContact>>,
    refresh: Callback<()>,
) -> impl IntoView {
    let editing: RwSignal<Option<PartyContactInput>> = RwSignal::new(None);
    let alerts = Alerts::get();

    let remove = move |contact_id: Uuid| {
        leptos::task::spawn_local(async move {
            match delete_party_contact(party_id, contact_id).await {
                Ok(()) => refresh.run(()),
                Err(err) => alerts.post(Alert::failure(err.to_string())),
            }
        });
    };

    view! {
        <div class="space-y-3">
            <Panel title=l!("parties.contacts")>
                <div class="space-y-2">
                    <Show
                        when=move || !contacts.get().is_empty()
                        fallback=move || {
                            view! {
                                <p class="text-sm text-content-subtle">
                                    {l!("parties.contact.none")}
                                </p>
                            }
                        }
                    >
                        <ul class="space-y-2">
                            {move || {
                                contacts
                                    .get()
                                    .into_iter()
                                    .map(|contact| {
                                        let id = contact.id;
                                        let draft = PartyContactInput::from_contact(&contact);
                                        let name = contact.name.clone();
                                        let job_title = contact.job_title.clone();
                                        let email = contact.email.clone();
                                        let phone = contact.phone.clone();
                                        let is_primary = contact.is_primary;

                                        view! {
                                            <li class="flex items-start justify-between gap-3 rounded-card border border-edge p-3">
                                                <div class="min-w-0 space-y-0.5">
                                                    <div class="flex flex-wrap items-center gap-1.5">
                                                        <span class="font-medium text-content">
                                                            {name}
                                                        </span>
                                                        {is_primary
                                                            .then(|| {
                                                                view! {
                                                                    <Badge
                                                                        label=l!("field.primary")
                                                                        tone=Tone::Brand
                                                                    />
                                                                }
                                                            })}
                                                    </div>
                                                    {job_title
                                                        .map(|title| {
                                                            view! {
                                                                <div class="text-xs text-content-muted">
                                                                    {title}
                                                                </div>
                                                            }
                                                        })}
                                                    <div class="text-xs text-content-subtle">
                                                        {email} {phone.map(|p| format!(" · {p}"))}
                                                    </div>
                                                </div>
                                                <div class="flex shrink-0 items-center gap-1.5">
                                                    <GhostButton
                                                        label=l!("common.edit")
                                                        icon=Icon::Pencil
                                                        on_click=Callback::new(move |()| {
                                                            editing.set(Some(draft.clone()));
                                                        })
                                                    />
                                                    <GhostButton
                                                        label=l!("common.remove")
                                                        icon=Icon::Trash2
                                                        on_click=Callback::new(move |()| remove(id))
                                                    />
                                                </div>
                                            </li>
                                        }
                                    })
                                    .collect_view()
                            }}
                        </ul>
                    </Show>

                    <GhostButton
                        label=l!("parties.contact.new")
                        icon=Icon::Plus
                        on_click=Callback::new(move |()| {
                            editing.set(Some(PartyContactInput::blank()));
                        })
                    />
                </div>
            </Panel>

            {move || {
                editing
                    .get()
                    .map(|draft| {
                        let host = FormHost {
                            refresh: Some(refresh),
                            close: Some(Callback::new(move |()| editing.set(None))),
                        };

                        view! {
                            <div class="max-w-3xl">
                                <Panel>
                                    <EntityForm
                                        config=crate::ui::form::config::parties::party_contact_form(
                                            party_id,
                                        )
                                        value=draft
                                        host=host
                                    />
                                </Panel>
                            </div>
                        }
                    })
            }}
        </div>
    }
}
