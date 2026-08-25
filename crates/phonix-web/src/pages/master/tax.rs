//! One tax: what it is, and what it has been charged at.
//!
//! # The rate history is the point of this screen
//!
//! A tax code is settled once and edited rarely. Its rates are a running
//! record: 17.5% until April 2026, 20% since. That record is what makes a
//! reprinted invoice show the rate of its own date, so it is drawn as a list of
//! windows rather than as a single "rate" box - a box would invite somebody to
//! overwrite history rather than to add to it.
//!
//! # Windows abut; they do not overlap
//!
//! Postgres enforces that with an exclusion constraint, and this screen does
//! not check first: a check-then-insert is a race, and the race is two
//! administrators filing the same rate change on the same afternoon. The
//! refusal arrives on the `From` field, which is the one to change.

use leptos::prelude::*;
use leptos_meta::Title;
use phonix_core::audit::kinds;
use phonix_core::permissions;
use phonix_tax::code::TaxCodeInput;
use phonix_tax::rate::{TaxRateInput, TaxRateRow};
use uuid::Uuid;

use crate::components::history::RecordHistory;
use crate::components::page::{Badge, GhostButton, Notice, PageHeader, Panel, Tone};
use crate::icons::Icon;
use crate::l;
use crate::server_fns::master_fns::{delete_tax_rate, tax_code_edit, tax_rates};
use crate::ui::alert::{Alert, Alerts};
use crate::ui::form::config::taxes::{tax_code_form, tax_rate_form};
use crate::ui::form::{EntityForm, FormHost};
use crate::ui::tabs::{Tab, TabbedPanel};

#[component]
pub fn tax_page() -> impl IntoView {
    let params = leptos_router::hooks::use_params_map();
    let tax_id = move || params.with(|params| params.get("id").unwrap_or_default());

    let code = Resource::new(tax_id, |raw| async move {
        match raw.parse::<Uuid>() {
            Ok(id) => tax_code_edit(id).await,
            Err(_) => Err(ServerFnError::new("That is not a tax id.")),
        }
    });

    view! {
        <Title text=format!("{} | Phonix", l!("entity.tax_code.singular")) />

        // Transition, not Suspense: navigating from one tax to another
        // re-suspends, and a fallback here would blank a screen somebody is
        // looking at rather than replacing it when the next one arrives.
        <Transition fallback=|| {
            view! { <p class="text-sm text-content-subtle">{l!("common.loading")}</p> }
        }>
            {move || Suspend::new(async move {
                match code.await {
                    Ok(draft) => {
                        view! { <TaxEditor draft=draft /> }.into_any()
                    }
                    Err(err) => {
                        view! {
                            <>
                                <PageHeader
                                    title=l!("entity.tax_code.singular")
                                    icon=Icon::Receipt
                                    back=("/master/taxes", l!("taxes.title"))
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
fn tax_editor(draft: TaxCodeInput) -> impl IntoView {
    // A tax opened for editing always has an id: `tax_code_edit` reads a stored
    // row. `Uuid::nil` is unreachable and is not worth an error path on a
    // screen that is already showing the record.
    let tax_id = draft.id.unwrap_or_else(Uuid::nil);
    let title = draft.name.clone();
    let code = draft.code.clone();
    let is_active = draft.is_active;
    let is_compound = draft.is_compound;

    // Hoisted above the tab strip: a render closure runs again every time its
    // tab comes back on screen, so state declared inside one is state thrown
    // away by looking at the other tab.
    let details = RwSignal::new(draft);
    let rates = Resource::new(move || tax_id, |id| async move { tax_rates(id).await });

    let refresh = Callback::new(move |()| {
        rates.refetch();
        leptos::task::spawn_local(async move {
            if let Ok(fresh) = tax_code_edit(tax_id).await {
                details.set(fresh);
            }
        });
    });

    let details_tab = Tab::new("details", "Details", move || {
        let host = FormHost {
            refresh: Some(refresh),
            close: None,
        };

        view! {
            <div class="max-w-3xl">
                <Panel>
                    <EntityForm
                        config=tax_code_form()
                        value=details.get_untracked()
                        host=host
                    />
                </Panel>
            </div>
        }
        .into_any()
    })
    .icon(Icon::SlidersHorizontal);

    let rates_tab = Tab::new("rates", "Rates", move || {
        view! { <RatePanel tax_id=tax_id rates=rates refresh=refresh /> }.into_any()
    })
    .icon(Icon::ChartColumn);

    let history_tab = Tab::new("history", "History", move || {
        view! { <RecordHistory kind=kinds::TAX_CODE id=Some(tax_id.to_string()) /> }.into_any()
    })
    .icon(Icon::Clock)
    .require(permissions::AUDIT_LOGS);

    view! {
        <PageHeader title=title icon=Icon::Receipt back=("/master/taxes", l!("taxes.title"))>
            <div class="flex flex-wrap items-center gap-1.5">
                <Badge label=code />
                {is_compound
                    .then(|| view! { <Badge label=l!("taxes.compound") tone=Tone::Warning /> })}
                {(!is_active).then(|| view! { <Badge label=l!("common.inactive") /> })}
            </div>
        </PageHeader>

        <TabbedPanel id="tax" tabs=vec![details_tab, rates_tab, history_tab] />
    }
}

/// The windows this tax has been charged over, newest first.
#[component]
fn rate_panel(
    tax_id: Uuid,
    rates: Resource<Result<Vec<TaxRateRow>, ServerFnError>>,
    refresh: Callback<()>,
) -> impl IntoView {
    // `None` means the form is closed. `Some((rate_id, draft))` is the window
    // being edited - `None` for the id when it is a new one.
    let editing: RwSignal<Option<(Option<Uuid>, TaxRateInput)>> = RwSignal::new(None);
    let alerts = Alerts::get();

    let remove = move |rate_id: Uuid| {
        leptos::task::spawn_local(async move {
            match delete_tax_rate(tax_id, rate_id).await {
                Ok(()) => refresh.run(()),
                Err(err) => alerts.post(Alert::failure(err.to_string())),
            }
        });
    };

    view! {
        <div class="space-y-3">
            <Panel title=l!("taxes.rates") description=l!("taxes.rates.help")>
                <Transition fallback=|| {
                    view! { <p class="text-sm text-content-subtle">{l!("common.loading")}</p> }
                }>
                    {move || Suspend::new(async move {
                        let windows = rates.await.unwrap_or_default();

                        view! {
                            <div class="space-y-2">
                                {if windows.is_empty() {
                                    view! {
                                        <p class="text-sm text-warning">
                                            {l!("taxes.rate.none")}
                                        </p>
                                    }
                                        .into_any()
                                } else {
                                    view! {
                                        <ul class="space-y-2">
                                            {windows
                                                .into_iter()
                                                .map(|window| {
                                                    let id = window.id;
                                                    let draft = TaxRateInput {
                                                        percent: window.period.rate.to_percent_string()
                                                            .trim_end_matches('%')
                                                            .to_owned(),
                                                        valid_from: window.period.valid_from,
                                                        valid_to: window.period.valid_to,
                                                    };
                                                    let percent = window.period.rate.to_percent_string();
                                                    let from = window.period.valid_from.to_string();
                                                    let until = window
                                                        .period
                                                        .valid_to
                                                        .map_or_else(
                                                            || l!("taxes.rate.open_ended"),
                                                            |to| to.to_string(),
                                                        );

                                                    view! {
                                                        <li class="flex items-center justify-between gap-3 rounded-card border border-edge p-3">
                                                            <div class="min-w-0">
                                                                <span class="font-medium tabular-nums text-content">
                                                                    {percent}
                                                                </span>
                                                                <div class="text-xs text-content-subtle">
                                                                    {from} " → " {until}
                                                                </div>
                                                            </div>
                                                            <div class="flex shrink-0 items-center gap-1.5">
                                                                <GhostButton
                                                                    label=l!("common.edit")
                                                                    icon=Icon::Pencil
                                                                    on_click=Callback::new(move |()| {
                                                                        editing.set(Some((Some(id), draft.clone())));
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
                                                .collect_view()}
                                        </ul>
                                    }
                                        .into_any()
                                }}

                                <GhostButton
                                    label=l!("taxes.rate.new")
                                    icon=Icon::Plus
                                    on_click=Callback::new(move |()| {
                                        editing
                                            .set(
                                                Some((
                                                    None,
                                                    TaxRateInput {
                                                        percent: String::new(),
                                                        // Today, because a rate
                                                        // change is nearly always
                                                        // filed on or near the day
                                                        // it takes effect.
                                                        valid_from: chrono::Local::now()
                                                            .date_naive(),
                                                        valid_to: None,
                                                    },
                                                )),
                                            );
                                    })
                                />
                            </div>
                        }
                    })}
                </Transition>
            </Panel>

            // Created fresh each time `editing` changes, which is what makes it
            // re-seed: `EntityForm` takes its opening value once.
            {move || {
                editing
                    .get()
                    .map(|(rate_id, draft)| {
                        let host = FormHost {
                            refresh: Some(refresh),
                            close: Some(Callback::new(move |()| editing.set(None))),
                        };

                        view! {
                            <div class="max-w-3xl">
                                <Panel>
                                    <EntityForm
                                        config=tax_rate_form(tax_id, rate_id)
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
