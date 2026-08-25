//! One invoice: the editor while it is a draft, the document once it is not.
//!
//! # The totals are worked out here, in the browser
//!
//! Not on the server. `app_books::pricing` compiles to wasm, so as somebody
//! types a quantity the whole document is re-priced locally - compound tax,
//! inclusive pricing, document-level rounding and all - with no round trip.
//! When they save, the server runs the *same function* over the same
//! treatments, so the figures on screen and the figures stored cannot disagree.
//!
//! The one thing the browser cannot know is which taxes apply on the document's
//! date, because that needs a rate table. `tax_treatments` hands those over
//! once, and re-fetches only when the date changes.
//!
//! # Posting is a decision, so it looks like one
//!
//! It is not a row action in the grid and it is not beside Save. It takes a
//! number nobody can hand back and freezes the document, so it lives at the end
//! of the screen where the whole invoice is in front of the reader, and it asks
//! first.

use app_books::invoice::{InvoiceInput, InvoiceLineInput, InvoiceStatus, PostOutcome};
use app_books::pricing::{PricedInvoice, PricedLine};
use app_books::quantity::Quantity;
use leptos::prelude::*;
use leptos_meta::Title;
use phonix_core::audit::kinds;
use phonix_core::form::Submission;
use phonix_core::locale::Currency;
use phonix_core::money::Money;
use phonix_core::permissions;
use phonix_master::party::PartySummary;
use phonix_tax::compute::{DocumentTax, Pricing, RoundingLevel};
use phonix_tax::group::TaxTreatment;
use uuid::Uuid;

use crate::components::history::RecordHistory;
use crate::components::page::{Badge, GhostButton, Notice, PageHeader, Panel, PrimaryButton, Tone};
use crate::icons::Icon;
use crate::l;
use crate::server_fns::books_fns::{
    delete_invoice, invoice_detail, post_invoice, save_invoice, tax_treatments, void_invoice,
};
use crate::server_fns::master_fns::list_parties;
use crate::ui::alert::{Alert, Alerts, Confirm};

/// Raising one.
#[component]
pub fn invoice_new_page() -> impl IntoView {
    // Today, from the browser's own clock, captured once. An invoice is dated
    // where the person raising it is, and `issued_on` is what decides which tax
    // rate applies - so it has to be a value somebody can see and change rather
    // than one the server substitutes.
    let today = chrono::Local::now().date_naive();

    view! {
        // "Phonix" is the product's name, not a word.
        <Title text=format!("{} | Phonix", l!("invoices.new")) />

        <PageHeader
            title=l!("invoices.new")
            subtitle=l!("invoices.new.subtitle")
            icon=Icon::FileText
            back=("/sales/invoices", l!("invoices.title"))
        />

        <InvoiceEditor draft=InvoiceInput::blank(today, Currency::default()) />
    }
}

/// One invoice: editable while it is a draft, read-only afterwards.
#[component]
pub fn invoice_page() -> impl IntoView {
    let params = leptos_router::hooks::use_params_map();
    let invoice_id = move || params.with(|params| params.get("id").unwrap_or_default());

    let invoice = Resource::new(invoice_id, |raw| async move {
        match raw.parse::<Uuid>() {
            Ok(id) => invoice_detail(id).await,
            Err(_) => Err(ServerFnError::new("That is not an invoice id.")),
        }
    });

    view! {
        <Title text=format!("{} | Phonix", l!("entity.sales_invoice.singular")) />

        // Transition, not Suspense: navigating from one invoice to another
        // re-suspends, and a fallback here would blank a screen somebody is
        // looking at rather than replacing it when the next one arrives.
        <Transition fallback=|| {
            view! { <p class="text-sm text-content-subtle">{l!("common.loading")}</p> }
        }>
            {move || Suspend::new(async move {
                match invoice.await {
                    Ok(stored) => {
                        let heading = stored
                            .number
                            .clone()
                            .unwrap_or_else(|| l!("books.status.draft"));
                        let status = stored.status;
                        let party = stored.party.name.clone();
                        let editable = stored.is_editable();
                        let opened_on = InvoiceInput::from_invoice(&stored);

                        view! {
                            <>
                                <PageHeader
                                    title=heading
                                    subtitle=party
                                    icon=Icon::FileText
                                    back=("/sales/invoices", l!("invoices.title"))
                                >
                                    <StatusBadge status=status />
                                </PageHeader>

                                {if editable {
                                    view! { <InvoiceEditor draft=opened_on /> }.into_any()
                                } else {
                                    view! {
                                        <InvoiceDocument
                                            invoice=stored
                                            reload=Callback::new(move |()| invoice.refetch())
                                        />
                                    }
                                        .into_any()
                                }}
                            </>
                        }
                            .into_any()
                    }
                    // The server's own words say more than a house phrase.
                    Err(err) => {
                        view! {
                            <>
                                <PageHeader
                                    title=l!("entity.sales_invoice.singular")
                                    icon=Icon::FileText
                                    back=("/sales/invoices", l!("invoices.title"))
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
fn status_badge(status: InvoiceStatus) -> impl IntoView {
    let label = crate::i18n::t(&status.label());
    let tone = match status {
        InvoiceStatus::Draft => Tone::Neutral,
        InvoiceStatus::Posted => Tone::Success,
        // Not danger: withdrawing a document is an ordinary correction, and
        // painting it red every time somebody opens it is noise.
        InvoiceStatus::Voided => Tone::Warning,
    };

    view! { <Badge label=label tone=tone /> }
}

// --- the editor ---------------------------------------------------------

/// Price a draft with the treatments in force on its date.
///
/// The whole preview, and the same call the server makes. Lines that are still
/// blank are skipped rather than refused: somebody halfway through typing a row
/// should still see the total of the rows above it.
fn price(draft: &InvoiceInput, treatments: &[TaxTreatment]) -> Option<DocumentTax> {
    let mut lines = Vec::with_capacity(draft.lines.len());

    for line in &draft.lines {
        let quantity = Quantity::parse(&line.quantity).ok()?;
        let unit_price = Money::parse(draft.currency, line.unit_price.trim()).ok();

        // Not yet a line. Skipped so the totals keep moving while somebody
        // types, rather than blanking on every keystroke.
        let Some(unit_price) = unit_price else {
            continue;
        };

        let treatment = match line.tax_group_id {
            None => TaxTreatment::none(),
            Some(group_id) => treatments
                .iter()
                .find(|treatment| treatment.tax_group_id == group_id)
                .cloned()
                // A group with no treatment on this date cannot be priced here.
                // The server says why on save; the preview simply stops.
                .unwrap_or_else(TaxTreatment::none),
        };

        lines.push(PricedLine {
            quantity,
            unit_price,
            treatment,
        });
    }

    PricedInvoice {
        currency: draft.currency,
        pricing: draft.pricing,
        rounding_level: draft.rounding_level,
        rounding: draft.rounding,
        lines,
    }
    .compute()
    .ok()
}

#[component]
fn invoice_editor(draft: InvoiceInput) -> impl IntoView {
    let draft = RwSignal::new(draft);
    let saving = RwSignal::new(false);
    let rejected = RwSignal::new(None::<String>);

    // Everyone this workspace could invoice. Fetched once.
    let parties = Resource::new(|| (), |()| async move { list_parties(None).await });

    // The taxes in force on the document's date. Re-fetched when the date
    // changes, because a backdated invoice is charged at the rate that was in
    // force when it was issued - the whole reason rates are effective-dated.
    let treatments = Resource::new(
        move || draft.with(|d| d.issued_on),
        |on| async move { tax_treatments(on).await },
    );

    view! {
        <div class="space-y-3">
            <Notice message=Signal::derive(move || rejected.get()) tone=Tone::Danger />

            <Transition fallback=|| {
                view! { <p class="text-sm text-content-subtle">{l!("common.loading")}</p> }
            }>
                {move || Suspend::new(async move {
                    // An empty picker rather than a failed screen: a workspace
                    // with no parties yet gets a form it cannot submit, which
                    // is the honest state of affairs.
                    let parties = parties.await.unwrap_or_default();
                    let treatments = treatments.await.unwrap_or_default();

                    view! {
                        <EditorBody
                            draft=draft
                            parties=parties
                            treatments=treatments
                            saving=saving
                            rejected=rejected
                        />
                    }
                })}
            </Transition>
        </div>
    }
}

#[component]
fn editor_body(
    draft: RwSignal<InvoiceInput>,
    parties: Vec<PartySummary>,
    treatments: Vec<TaxTreatment>,
    saving: RwSignal<bool>,
    rejected: RwSignal<Option<String>>,
) -> impl IntoView {
    let alerts = Alerts::get();
    let navigate = leptos_router::hooks::use_navigate();

    // Held so the closures below can read them repeatedly without cloning a
    // vector on every keystroke.
    let parties = StoredValue::new(parties);
    let treatments = StoredValue::new(treatments);

    // The live preview.
    //
    // A memo, so it recomputes when the draft changes and not otherwise, and it
    // runs `app_books::pricing` - the same function the server posts with.
    let totals = Memo::new(move |_| {
        let current = draft.get();
        price(&current, &treatments.get_value())
    });

    let save = {
        let navigate = navigate.clone();
        move || {
            saving.set(true);
            rejected.set(None);
            let submission = draft.get_untracked();
            let navigate = navigate.clone();

            leptos::task::spawn_local(async move {
                let result = save_invoice(submission).await;
                saving.set(false);

                match result {
                    Ok(Submission::Saved(stored)) => {
                        let id = stored.id;
                        draft.set(stored);
                        alerts.post(Alert::success(l!("books.saved")));

                        // A new draft has an id now, so the address it lives at
                        // has changed. Replacing rather than pushing: going
                        // back should reach the list, not a form that no longer
                        // exists.
                        if let Some(id) = id {
                            navigate(
                                &format!("/sales/invoices/{id}"),
                                leptos_router::NavigateOptions {
                                    replace: true,
                                    ..Default::default()
                                },
                            );
                        }
                    }
                    // A rejected field arrives here rather than as an error, so
                    // the sentence lands beside the document rather than at the
                    // top of the page as a stack of colons.
                    Ok(Submission::Rejected(errors)) => {
                        rejected.set(errors.first().map(|error| crate::i18n::t(&error.message)));
                    }
                    Err(err) => rejected.set(Some(err.to_string())),
                }
            });
        }
    };

    let post = {
        let navigate = navigate.clone();
        move || {
            let Some(id) = draft.with_untracked(|d| d.id) else {
                return;
            };
            let navigate = navigate.clone();

            alerts.ask(
                Confirm::new(l!("invoices.post.confirm"), move || {
                    let navigate = navigate.clone();
                    saving.set(true);

                    leptos::task::spawn_local(async move {
                        let result = post_invoice(id).await;
                        saving.set(false);

                        match result {
                            Ok(PostOutcome::Posted { number }) => {
                                alerts.post(
                                    Alert::success(l!("invoices.posted", number = number))
                                        .titled(l!("books.posted")),
                                );
                                // Reload the route: the invoice is a document
                                // now, and this screen renders a different
                                // thing for one.
                                navigate(
                                    &format!("/sales/invoices/{id}"),
                                    leptos_router::NavigateOptions {
                                        replace: true,
                                        ..Default::default()
                                    },
                                );
                            }
                            // Outcomes rather than errors: both are things to
                            // read and act on, not failures.
                            Ok(outcome) => {
                                alerts.post(Alert::warning(crate::i18n::t(&outcome.message())));
                            }
                            Err(err) => alerts.post(Alert::failure(err.to_string())),
                        }
                    });
                })
                .titled(l!("invoices.post"))
                .confirm_label(l!("invoices.post")),
            );
        }
    };

    let delete = {
        let navigate = navigate.clone();
        move || {
            let Some(id) = draft.with_untracked(|d| d.id) else {
                return;
            };
            let navigate = navigate.clone();

            alerts.ask(
                Confirm::new(l!("invoices.delete.confirm"), move || {
                    let navigate = navigate.clone();

                    leptos::task::spawn_local(async move {
                        match delete_invoice(id).await {
                            Ok(()) => {
                                alerts.post(Alert::success(l!("invoices.deleted")));
                                navigate(
                                    "/sales/invoices",
                                    leptos_router::NavigateOptions::default(),
                                );
                            }
                            Err(err) => alerts.post(Alert::failure(err.to_string())),
                        }
                    });
                })
                .titled(l!("common.delete"))
                .confirm_label(l!("common.delete")),
            );
        }
    };

    let saved = move || draft.with(|d| d.id.is_some());

    view! {
        <div class="space-y-3">
            <Panel title=l!("invoices.header")>
                <HeaderFields draft=draft parties=parties />
            </Panel>

            <Panel title=l!("invoices.lines") description=l!("invoices.lines.help")>
                <LineTable draft=draft treatments=treatments />
            </Panel>

            <div class="grid gap-3 lg:grid-cols-[1fr_22rem] lg:items-start">
                <Panel title=l!("invoices.notes")>
                    <textarea
                        class="w-full"
                        rows="3"
                        prop:value=move || draft.with(|d| d.notes.clone().unwrap_or_default())
                        on:input=move |ev| {
                            let text = event_target_value(&ev);
                            draft
                                .update(|d| {
                                    d.notes = (!text.trim().is_empty()).then_some(text);
                                });
                        }
                    />
                </Panel>

                <Totals totals=totals />
            </div>

            <div class="flex flex-wrap items-center justify-end gap-2">
                <Show when=saved fallback=|| ()>
                    <GhostButton
                        label=l!("common.delete")
                        icon=Icon::Trash2
                        on_click=Callback::new({
                            let delete = delete.clone();
                            move |()| delete()
                        })
                    />
                </Show>

                <GhostButton
                    label=l!("common.save")
                    icon=Icon::Save
                    disabled=Signal::derive(move || saving.get())
                    on_click=Callback::new({
                        let save = save.clone();
                        move |()| save()
                    })
                />

                // Posting is the one irreversible act here, so it is its own
                // button at the end and it asks first. Offered only once the
                // draft has been saved: there is nothing to number otherwise.
                <Show when=saved fallback=|| ()>
                    <PrimaryButton
                        label=l!("invoices.post")
                        icon=Icon::Check
                        pending=Signal::derive(move || saving.get())
                        disabled=Signal::derive(move || totals.with(Option::is_none))
                        on_click=Callback::new({
                            let post = post.clone();
                            move |()| post()
                        })
                    />
                </Show>
            </div>
        </div>
    }
}

/// Who it is for, when it is dated, and how it is priced.
#[component]
fn header_fields(
    draft: RwSignal<InvoiceInput>,
    parties: StoredValue<Vec<PartySummary>>,
) -> impl IntoView {
    view! {
        <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            <label class="block space-y-1">
                <span class="text-xs font-medium text-content-muted">
                    {l!("invoices.customer")}
                </span>
                <select
                    class="w-full"
                    prop:value=move || {
                        draft.with(|d| d.party_id.map(|id| id.to_string()).unwrap_or_default())
                    }
                    on:change=move |ev| {
                        let chosen = event_target_value(&ev).parse::<Uuid>().ok();
                        draft.update(|d| d.party_id = chosen);
                    }
                >
                    <option value="">{l!("common.not_set")}</option>
                    {move || {
                        parties
                            .get_value()
                            .into_iter()
                            // A deactivated party is not somebody to invoice.
                            .filter(|party| party.is_active)
                            .map(|party| {
                                let value = party.id.to_string();
                                let label = format!("{} ({})", party.name, party.code);
                                view! { <option value=value>{label}</option> }
                            })
                            .collect_view()
                    }}
                </select>
            </label>

            <label class="block space-y-1">
                <span class="text-xs font-medium text-content-muted">
                    {l!("invoices.issued")}
                </span>
                <input
                    type="date"
                    class="w-full"
                    prop:value=move || draft.with(|d| d.issued_on.to_string())
                    on:change=move |ev| {
                        if let Ok(date) = event_target_value(&ev).parse() {
                            draft.update(|d| d.issued_on = date);
                        }
                    }
                />
                <span class="block text-2xs text-content-subtle">
                    {l!("invoices.issued.help")}
                </span>
            </label>

            <label class="block space-y-1">
                <span class="text-xs font-medium text-content-muted">{l!("invoices.due")}</span>
                <input
                    type="date"
                    class="w-full"
                    prop:value=move || {
                        draft.with(|d| d.due_on.map(|due| due.to_string()).unwrap_or_default())
                    }
                    on:change=move |ev| {
                        let value = event_target_value(&ev);
                        draft.update(|d| d.due_on = value.parse().ok());
                    }
                />
            </label>

            <label class="block space-y-1">
                <span class="text-xs font-medium text-content-muted">
                    {l!("field.currency")}
                </span>
                <select
                    class="w-full"
                    prop:value=move || draft.with(|d| d.currency.code().to_owned())
                    on:change=move |ev| {
                        // Unrecognised keeps what was there: a currency
                        // silently becoming dollars changes what every amount
                        // on the document means.
                        if let Ok(currency) = Currency::parse(event_target_value(&ev)) {
                            draft.update(|d| d.currency = currency);
                        }
                    }
                >
                    {Currency::all()
                        .iter()
                        .map(|currency| {
                            let value = currency.code().to_owned();
                            let label = currency.label();
                            view! { <option value=value>{label}</option> }
                        })
                        .collect_view()}
                </select>
            </label>

            <label class="block space-y-1">
                <span class="text-xs font-medium text-content-muted">
                    {l!("invoices.pricing")}
                </span>
                <select
                    class="w-full"
                    prop:value=move || draft.with(|d| d.pricing.as_str().to_owned())
                    on:change=move |ev| {
                        if let Some(pricing) = Pricing::parse(&event_target_value(&ev)) {
                            draft.update(|d| d.pricing = pricing);
                        }
                    }
                >
                    {[Pricing::Exclusive, Pricing::Inclusive]
                        .into_iter()
                        .map(|pricing| {
                            let value = pricing.as_str();
                            let label = crate::i18n::t(&pricing.label());
                            view! { <option value=value>{label}</option> }
                        })
                        .collect_view()}
                </select>
            </label>

            <label class="block space-y-1">
                <span class="text-xs font-medium text-content-muted">
                    {l!("invoices.rounding")}
                </span>
                <select
                    class="w-full"
                    prop:value=move || draft.with(|d| d.rounding_level.as_str().to_owned())
                    on:change=move |ev| {
                        if let Some(level) = RoundingLevel::parse(&event_target_value(&ev)) {
                            draft.update(|d| d.rounding_level = level);
                        }
                    }
                >
                    {[RoundingLevel::Line, RoundingLevel::Document]
                        .into_iter()
                        .map(|level| {
                            let value = level.as_str();
                            let label = crate::i18n::t(&level.label());
                            view! { <option value=value>{label}</option> }
                        })
                        .collect_view()}
                </select>
            </label>
        </div>
    }
}

/// The lines, and what each comes to.
#[component]
fn line_table(
    draft: RwSignal<InvoiceInput>,
    treatments: StoredValue<Vec<TaxTreatment>>,
) -> impl IntoView {
    view! {
        <div class="space-y-2">
            // `overflow-x-auto` on the table's own container, never on the
            // page: anything wider than the phone inflates the viewport and
            // throws every fixed overlay off screen.
            <div class="overflow-x-auto">
                <table class="w-full min-w-[46rem] text-sm">
                    <thead>
                        <tr class="border-b border-edge text-left text-xs text-content-muted">
                            <th class="w-8 py-2 font-medium">"#"</th>
                            <th class="py-2 font-medium">{l!("invoices.description")}</th>
                            <th class="w-24 py-2 text-right font-medium">
                                {l!("invoices.quantity")}
                            </th>
                            <th class="w-32 py-2 text-right font-medium">
                                {l!("invoices.unit_price")}
                            </th>
                            <th class="w-40 py-2 font-medium">{l!("invoices.tax_group")}</th>
                            <th class="w-8 py-2"></th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let count = draft.with(|d| d.lines.len());
                            (0..count)
                                .map(|index| {
                                    view! {
                                        <LineRow
                                            draft=draft
                                            index=index
                                            treatments=treatments
                                        />
                                    }
                                })
                                .collect_view()
                        }}
                    </tbody>
                </table>
            </div>

            <GhostButton
                label=l!("invoices.line.add")
                icon=Icon::Plus
                on_click=Callback::new(move |()| {
                    draft.update(|d| d.lines.push(InvoiceLineInput::blank()));
                })
            />
        </div>
    }
}

#[component]
fn line_row(
    draft: RwSignal<InvoiceInput>,
    index: usize,
    treatments: StoredValue<Vec<TaxTreatment>>,
) -> impl IntoView {
    // Every read goes through `get` on the index rather than holding a clone:
    // a row that cached its own values would stop updating the moment another
    // row was removed and the indexes shifted.
    let field = move |read: fn(&InvoiceLineInput) -> String| {
        draft.with(|d| d.lines.get(index).map(read).unwrap_or_default())
    };

    view! {
        <tr class="border-b border-edge/60">
            <td class="py-1 text-xs text-content-subtle">{index + 1}</td>
            <td class="py-1 pr-2">
                <input
                    type="text"
                    class="w-full"
                    prop:value=move || field(|line| line.description.clone())
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        draft
                            .update(|d| {
                                if let Some(line) = d.lines.get_mut(index) {
                                    line.description = value;
                                }
                            });
                    }
                />
            </td>
            <td class="py-1 pr-2">
                // Text rather than a number input: `Quantity::parse` refuses a
                // fifth decimal place rather than rounding it, and a browser
                // number input would have rounded before this ever saw it.
                <input
                    type="text"
                    inputmode="decimal"
                    class="w-full text-right tabular-nums"
                    prop:value=move || field(|line| line.quantity.clone())
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        draft
                            .update(|d| {
                                if let Some(line) = d.lines.get_mut(index) {
                                    line.quantity = value;
                                }
                            });
                    }
                />
            </td>
            <td class="py-1 pr-2">
                <input
                    type="text"
                    inputmode="decimal"
                    class="w-full text-right tabular-nums"
                    prop:value=move || field(|line| line.unit_price.clone())
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        draft
                            .update(|d| {
                                if let Some(line) = d.lines.get_mut(index) {
                                    line.unit_price = value;
                                }
                            });
                    }
                />
            </td>
            <td class="py-1 pr-2">
                <select
                    class="w-full"
                    prop:value=move || {
                        field(|line| {
                            line.tax_group_id.map(|id| id.to_string()).unwrap_or_default()
                        })
                    }
                    on:change=move |ev| {
                        let chosen = event_target_value(&ev).parse::<Uuid>().ok();
                        draft
                            .update(|d| {
                                if let Some(line) = d.lines.get_mut(index) {
                                    line.tax_group_id = chosen;
                                }
                            });
                    }
                >
                    // "No tax" is not the same as a zero rate - that is a group
                    // whose rate is zero, and the difference shows on the
                    // document and in the return.
                    <option value="">{l!("invoices.no_tax")}</option>
                    {move || {
                        treatments
                            .get_value()
                            .into_iter()
                            .map(|treatment| {
                                let value = treatment.tax_group_id.to_string();
                                let label = treatment.group_code.clone();
                                view! { <option value=value>{label}</option> }
                            })
                            .collect_view()
                    }}
                </select>
            </td>
            <td class="py-1 text-right">
                <button
                    type="button"
                    class="rounded-control p-1 text-content-subtle hover:bg-surface-hover hover:text-danger"
                    aria-label=l!("common.remove")
                    on:click=move |_| {
                        draft
                            .update(|d| {
                                if index < d.lines.len() {
                                    d.lines.remove(index);
                                }
                                // Never leave the table empty: a form with no
                                // rows has nothing to type into.
                                if d.lines.is_empty() {
                                    d.lines.push(InvoiceLineInput::blank());
                                }
                            });
                    }
                >
                    <Icon icon=Icon::Trash2 size=crate::icons::IconSize::Xs />
                </button>
            </td>
        </tr>
    }
}

/// What the document comes to, as it is typed.
#[component]
fn totals(totals: Memo<Option<DocumentTax>>) -> impl IntoView {
    view! {
        <Panel title=l!("invoices.total")>
            {move || match totals.get() {
                None => {
                    // A total that silently reads zero because the arithmetic
                    // failed is worse than no total: somebody sends it.
                    view! {
                        <p class="text-sm text-content-subtle">{l!("invoices.no_total")}</p>
                    }
                        .into_any()
                }
                Some(computed) => {
                    let net = computed.net.to_display_string();
                    let gross = computed.gross.to_display_string();
                    let code = computed.currency.code().to_owned();
                    let taxes: Vec<(String, String)> = computed
                        .by_tax
                        .iter()
                        .map(|total| {
                            (
                                format!("{} {}", total.code, total.rate.to_percent_string()),
                                total.amount.to_display_string(),
                            )
                        })
                        .collect();

                    view! {
                        <dl class="space-y-1 text-sm tabular-nums">
                            <div class="flex items-baseline justify-between gap-4">
                                <dt class="text-content-muted">{l!("invoices.net")}</dt>
                                <dd class="text-content">{net}</dd>
                            </div>

                            // Every tax on its own line, because that is what a
                            // return is filed from - and what makes a split
                            // charge like CGST and SGST legible.
                            {taxes
                                .into_iter()
                                .map(|(label, amount)| {
                                    view! {
                                        <div class="flex items-baseline justify-between gap-4">
                                            <dt class="text-content-muted">{label}</dt>
                                            <dd class="text-content">{amount}</dd>
                                        </div>
                                    }
                                })
                                .collect_view()}

                            <div class="flex items-baseline justify-between gap-4 border-t border-edge pt-1 font-medium">
                                <dt class="text-content">
                                    {l!("invoices.total")} " " <span class="text-2xs text-content-subtle">{code}</span>
                                </dt>
                                <dd class="text-content">{gross}</dd>
                            </div>
                        </dl>
                    }
                        .into_any()
                }
            }}
        </Panel>
    }
}

// --- the document -------------------------------------------------------

/// A posted or voided invoice: read-only, and everything on it is what was
/// stored rather than what could be looked up now.
#[component]
fn invoice_document(invoice: app_books::invoice::Invoice, reload: Callback<()>) -> impl IntoView {
    let alerts = Alerts::get();
    let viewer = crate::ui::viewer::Viewer::get();
    let may_void = Signal::derive(move || {
        viewer.with(|user| {
            user.as_ref()
                .is_some_and(|user| user.can(permissions::INVOICES_VOID))
        })
    });

    let id = invoice.id;
    let is_posted = invoice.status == InvoiceStatus::Posted;

    let address = invoice.party.address.lines();
    let party_name = invoice.party.name.clone();
    let party_code = invoice.party.code.clone();
    let tax_id = invoice.party.tax_id.clone();
    let issued = invoice.issued_on.to_string();
    let due = invoice.due_on.map(|due| due.to_string());
    let notes = invoice.notes.clone();
    let code = invoice.currency.code().to_owned();
    let net = invoice.totals.net.to_display_string();
    let tax = invoice.totals.tax.to_display_string();
    let gross = invoice.totals.gross.to_display_string();
    let base = invoice.totals.base_gross.map(|amount| {
        format!(
            "{} {}",
            amount.currency().code(),
            amount.to_display_string()
        )
    });
    let lines = invoice.lines.clone();

    let void = move || {
        alerts.ask(
            Confirm::new(l!("invoices.void.confirm"), move || {
                leptos::task::spawn_local(async move {
                    match void_invoice(id).await {
                        Ok(()) => {
                            alerts.post(Alert::success(l!("invoices.voided")));
                            reload.run(());
                        }
                        Err(err) => alerts.post(Alert::failure(err.to_string())),
                    }
                });
            })
            .titled(l!("invoices.void"))
            .confirm_label(l!("invoices.void")),
        );
    };

    view! {
        <div class="space-y-3">
            <div class="grid gap-3 lg:grid-cols-2">
                <Panel title=l!("invoices.customer")>
                    <div class="space-y-1 text-sm">
                        <div class="font-medium text-content">{party_name}</div>
                        <code class="text-2xs text-content-subtle">{party_code}</code>
                        {address
                            .into_iter()
                            .map(|line| {
                                view! { <div class="text-content-muted">{line}</div> }
                            })
                            .collect_view()}
                        {tax_id
                            .map(|tax_id| {
                                view! {
                                    <div class="text-xs text-content-subtle">{tax_id}</div>
                                }
                            })}
                    </div>
                </Panel>

                <Panel title=l!("invoices.header")>
                    <dl class="space-y-1 text-sm">
                        <div class="flex justify-between gap-4">
                            <dt class="text-content-muted">{l!("invoices.issued")}</dt>
                            <dd class="tabular-nums text-content">{issued}</dd>
                        </div>
                        {due
                            .map(|due| {
                                view! {
                                    <div class="flex justify-between gap-4">
                                        <dt class="text-content-muted">{l!("invoices.due")}</dt>
                                        <dd class="tabular-nums text-content">{due}</dd>
                                    </div>
                                }
                            })}
                    </dl>
                </Panel>
            </div>

            <Panel title=l!("invoices.lines")>
                <div class="overflow-x-auto">
                    <table class="w-full min-w-[40rem] text-sm">
                        <thead>
                            <tr class="border-b border-edge text-left text-xs text-content-muted">
                                <th class="w-8 py-2 font-medium">"#"</th>
                                <th class="py-2 font-medium">{l!("invoices.description")}</th>
                                <th class="py-2 text-right font-medium">
                                    {l!("invoices.quantity")}
                                </th>
                                <th class="py-2 text-right font-medium">
                                    {l!("invoices.unit_price")}
                                </th>
                                <th class="py-2 text-right font-medium">{l!("invoices.tax")}</th>
                                <th class="py-2 text-right font-medium">
                                    {l!("invoices.total")}
                                </th>
                            </tr>
                        </thead>
                        <tbody>
                            {lines
                                .into_iter()
                                .map(|line| {
                                    // The rate that was charged, from the
                                    // snapshot. Never re-resolved: a rate that
                                    // changed since must not rewrite a document
                                    // that was already sent.
                                    let rates = line
                                        .taxes
                                        .iter()
                                        .map(|tax| {
                                            format!(
                                                "{} {}",
                                                tax.applied.code,
                                                tax.applied.rate.to_percent_string(),
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                        .join(", ");

                                    view! {
                                        <tr class="border-b border-edge/60">
                                            <td class="py-1.5 text-xs text-content-subtle">
                                                {line.line_no}
                                            </td>
                                            <td class="py-1.5 text-content">
                                                {line.description.clone()}
                                                <div class="text-2xs text-content-subtle">
                                                    {rates}
                                                </div>
                                            </td>
                                            <td class="py-1.5 text-right tabular-nums text-content-muted">
                                                {line.quantity.to_display_string()}
                                            </td>
                                            <td class="py-1.5 text-right tabular-nums text-content-muted">
                                                {line.unit_price.to_display_string()}
                                            </td>
                                            <td class="py-1.5 text-right tabular-nums text-content-muted">
                                                {line.tax.to_display_string()}
                                            </td>
                                            <td class="py-1.5 text-right tabular-nums text-content">
                                                {line.gross.to_display_string()}
                                            </td>
                                        </tr>
                                    }
                                })
                                .collect_view()}
                        </tbody>
                    </table>
                </div>
            </Panel>

            <div class="grid gap-3 lg:grid-cols-[1fr_22rem] lg:items-start">
                {notes
                    .map(|notes| {
                        view! {
                            <Panel title=l!("invoices.notes")>
                                <p class="whitespace-pre-wrap text-sm text-content-muted">
                                    {notes}
                                </p>
                            </Panel>
                        }
                    })}

                <Panel title=l!("invoices.total")>
                    <dl class="space-y-1 text-sm tabular-nums">
                        <div class="flex justify-between gap-4">
                            <dt class="text-content-muted">{l!("invoices.net")}</dt>
                            <dd class="text-content">{net}</dd>
                        </div>
                        <div class="flex justify-between gap-4">
                            <dt class="text-content-muted">{l!("invoices.tax")}</dt>
                            <dd class="text-content">{tax}</dd>
                        </div>
                        <div class="flex justify-between gap-4 border-t border-edge pt-1 font-medium">
                            <dt class="text-content">
                                {l!("invoices.total")} " "
                                <span class="text-2xs text-content-subtle">{code}</span>
                            </dt>
                            <dd class="text-content">{gross}</dd>
                        </div>
                        // The base-currency figure, converted once at the rate
                        // stored on the document. Never recomputed from today's
                        // rate - that is the classic bug, and it silently
                        // rewrites history.
                        {base
                            .map(|base| {
                                view! {
                                    <div class="flex justify-between gap-4 text-xs text-content-subtle">
                                        <dt>{l!("invoices.in_base")}</dt>
                                        <dd>{base}</dd>
                                    </div>
                                }
                            })}
                    </dl>
                </Panel>
            </div>

            // Two conditions, and they are different questions. The *status*
            // decides whether voiding means anything - only a posted document
            // can be withdrawn. The *permission* decides whether this reader
            // may do it, and the service checks it again: hiding a button is a
            // courtesy, `Caller::require` is the control.
            <Show when=move || is_posted && may_void.get() fallback=|| ()>
                <div class="flex justify-end">
                    <GhostButton
                        label=l!("invoices.void")
                        icon=Icon::Ban
                        on_click=Callback::new(move |()| void())
                    />
                </div>
            </Show>

            <Panel title=l!("common.history")>
                <RecordHistory kind=kinds::SALES_INVOICE id=Some(id.to_string()) />
            </Panel>
        </div>
    }
}
