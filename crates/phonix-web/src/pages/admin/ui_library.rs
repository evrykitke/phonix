//! The interface kit, on a page.
//!
//! # What it is for
//!
//! [`crate::ui`] is furniture that has never heard of Phonix, and until now the
//! only way to see a piece of it was to find a screen that happened to use one.
//! That is a poor way to answer the two questions people actually have - *does
//! the kit already do this?* and *what does it look like in the state I need?* -
//! and it is how a codebase ends up with a second dropdown.
//!
//! So: one tab per component, showing it in the states it can be in, beside a
//! sentence saying what it will and will not do. A component is not finished
//! until it is on this page.
//!
//! # The specimen text is the documentation
//!
//! A showcase needs something inside each example, and filler would put a
//! paragraph of nothing into three catalogs forever. What is in the examples
//! here is instead the component's own design notes - why it is built the way
//! it is, and what it refuses to do - so the demonstration and the reference
//! are the same words.
//!
//! # There is no server function behind this page
//!
//! Nothing on it is data, so there is nothing to refuse. The permission is
//! honest all the same: [`phonix_core::permissions::UI_LIBRARY`] keeps it off the sidebar
//! of anybody who has not been granted it, and a workspace that does not want a
//! developer reference in its menu revokes it from the role. Somebody who types
//! the URL sees component specimens, which is the whole of what there is to see.

use leptos::prelude::*;
use leptos_meta::Title;

use phonix_core::i18n::Message;
use phonix_core::locale::Currency;
use phonix_core::money::WorkspaceCurrency;

use crate::components::page::{
    Badge, FormActions, Notice, PageHeader, Panel, PrimaryButton, Tone,
};
use crate::i18n::t;
use crate::icons::Icon;
use crate::l;
use crate::server_fns::currency_fns::{enabled_currencies, save_currency};
use crate::ui::card::CollapsibleCard;
use crate::ui::editor::{EDITOR_GZIP_BYTES, RichText};
use crate::ui::form::field::Choice;
use crate::ui::form::{EntityForm, Field, FieldValue, FormAction, FormConfig};
use crate::ui::lookup::{Choices, LookupField, QuickAdd, SelectField};
use crate::ui::table::DataGrid;
use crate::ui::table::config::currencies::currencies_picker;
use crate::ui::tabs::{Tab, TabbedPanel};

#[component]
pub fn ui_library_page() -> impl IntoView {
    // Ordered as the kit is built, newest last, so a tab does not move under
    // somebody's cursor when the next component lands. The roadmap stays at the
    // end for the same reason.
    let tabs = vec![
        Tab::new("cards", l!("ui_library.tab.cards"), || {
            view! { <CardsTab /> }.into_any()
        })
        .icon(Icon::LayoutGrid),
        Tab::new("editor", l!("ui_library.tab.editor"), || {
            view! { <EditorTab /> }.into_any()
        })
        .icon(Icon::Pencil),
        Tab::new("lookup", l!("ui_library.tab.lookup"), || {
            view! { <LookupTab /> }.into_any()
        })
        .icon(Icon::Search),
        Tab::new("roadmap", l!("ui_library.tab.roadmap"), || {
            view! { <RoadmapTab /> }.into_any()
        })
        .icon(Icon::ListTree),
    ];

    view! {
        // "Phonix" is the product's name, not a word.
        <Title text=format!("{} | Phonix", l!("ui_library.title")) />

        <PageHeader
            title=l!("ui_library.title")
            subtitle=l!("ui_library.subtitle")
            icon=Icon::Palette
        />

        <TabbedPanel id="ui-library" tabs=tabs />
    }
}

/// [`CollapsibleCard`], in each of the shapes it comes in.
#[component]
fn cards_tab() -> impl IntoView {
    view! {
        <Panel title=l!("ui_library.cards.title") description=l!("ui_library.cards.detail")>
            <div class="space-y-3">
                // A stack rather than one specimen: the shape of this component
                // is a list, and a single card would not show that the closed
                // ones stay scannable while one of them is open.
                <CollapsibleCard
                    title=l!("ui_library.cards.why.title")
                    detail=l!("ui_library.cards.detail")
                    icon=Icon::CircleHelp
                >
                    <Prose>{l!("ui_library.cards.why.body")}</Prose>
                </CollapsibleCard>

                <CollapsibleCard
                    title=l!("ui_library.cards.limits.title")
                    icon=Icon::Ban
                    meta="v1".to_owned()
                >
                    <Prose>{l!("ui_library.cards.limits.body")}</Prose>
                </CollapsibleCard>

                // No icon and no meta: the header collapses to a title and a
                // chevron, which is what the component looks like at its
                // smallest and is worth seeing beside the fuller ones.
                <CollapsibleCard title=l!("ui_library.cards.usage.title")>
                    <Prose>{l!("ui_library.cards.usage.body")}</Prose>
                </CollapsibleCard>

                <CollapsibleCard
                    title=l!("ui_library.cards.open.title")
                    detail=l!("ui_library.cards.open.detail")
                    icon=Icon::Eye
                    open=true
                >
                    <Prose>{l!("ui_library.cards.open.body")}</Prose>
                </CollapsibleCard>
            </div>
        </Panel>
    }
}

/// [`LookupField`], in all three presentations, over the currency list.
///
/// Currencies because they are the sample entity for anything in the kit that
/// needs data: a real list, a real service behind the quick add, and a
/// `GridConfig` that already exists. A fixture of the showcase's own would
/// demonstrate the showcase.
///
/// # Why the chosen value is echoed under each field
///
/// A lookup that looks right and reports the wrong thing is the failure worth
/// catching, and it is invisible from the control itself. The line underneath
/// is what the field would put in a draft.
#[component]
fn lookup_tab() -> impl IntoView {
    // Loaded once and shared by the list-shaped specimens. The table one does
    // not use it: it goes through the grid, which fetches its own.
    let currencies = OnceResource::new(enabled_currencies());

    let one = RwSignal::new(Vec::<Choice>::new());
    let many = RwSignal::new(Vec::<Choice>::new());
    let picked = RwSignal::new(Vec::<Choice>::new());
    let added = RwSignal::new(Vec::<Choice>::new());

    view! {
        <div class="space-y-3">
            <Panel title=l!("ui_library.lookup.title") description=l!("ui_library.lookup.detail")>
                <Suspense fallback=|| {
                    view! { <p class="text-sm text-content-subtle">{l!("common.loading")}</p> }
                }>
                    {move || Suspend::new(async move {
                        let choices = currencies
                            .await
                            .unwrap_or_default()
                            .into_iter()
                            .map(|currency| {
                                Choice::new(currency.code(), currency.name())
                                    .detail(currency.code())
                            })
                            .collect::<Vec<_>>();

                        // A clone per specimen, named up here. Each `Specimen`
                        // holds its children in a closure, so one binding
                        // cloned at three call sites is one binding moved into
                        // the first closure and gone by the second.
                        let (for_one, for_many) = (choices.clone(), choices.clone());

                        view! {
                            <div class="grid gap-6 lg:grid-cols-2">
                                <Specimen
                                    title=l!("ui_library.lookup.one.title")
                                    detail=l!("ui_library.lookup.one.detail")
                                    chosen=one
                                >
                                    <LookupField selected=one choices=Choices::List(for_one) />
                                </Specimen>

                                <Specimen
                                    title=l!("ui_library.lookup.many.title")
                                    detail=l!("ui_library.lookup.many.detail")
                                    chosen=many
                                >
                                    <LookupField
                                        selected=many
                                        choices=Choices::List(for_many)
                                        multiple=true
                                    />
                                </Specimen>

                                <Specimen
                                    title=l!("ui_library.lookup.add.title")
                                    detail=l!("ui_library.lookup.add.detail")
                                    chosen=added
                                >
                                    <LookupField
                                        selected=added
                                        choices=Choices::List(choices)
                                        quick_add=Some(
                                            QuickAdd::form(
                                                l!("ui_library.lookup.add.action"),
                                                l!("ui_library.lookup.add.title"),
                                                |answer| {
                                                    view! { <AddCurrency answer=answer /> }
                                                        .into_any()
                                                },
                                            ),
                                        )
                                    />
                                </Specimen>

                                <Specimen
                                    title=l!("ui_library.lookup.table.title")
                                    detail=l!("ui_library.lookup.table.detail")
                                    chosen=picked
                                >
                                    <LookupField
                                        selected=picked
                                        // The panel holds the currency list
                                        // itself: same columns, same search,
                                        // same filter. Nothing written here
                                        // describes a currency, which is the
                                        // whole of the argument for doing it
                                        // this way.
                                        choices=Choices::table(|answer: Callback<Choice>| {
                                            let config = currencies_picker(
                                                Callback::new(move |row: WorkspaceCurrency| {
                                                    answer
                                                        .run(
                                                            Choice::new(
                                                                    row.currency.code(),
                                                                    row.currency.name(),
                                                                )
                                                                .detail(row.display()),
                                                        );
                                                }),
                                            );
                                            view! { <DataGrid config=config /> }.into_any()
                                        })
                                        quick_add=Some(
                                            QuickAdd::page(
                                                l!("ui_library.lookup.page"),
                                                "/admin/settings?tab=currencies",
                                            ),
                                        )
                                    />
                                </Specimen>
                            </div>
                        }
                    })}
                </Suspense>
            </Panel>

            <Panel
                title=l!("ui_library.lookup.form.title")
                description=l!("ui_library.lookup.form.detail")
            >
                <LookupInAForm />
            </Panel>

            <Panel title=l!("ui_library.select.title") description=l!("ui_library.select.detail")>
                <SelectSpecimens />
            </Panel>

            <CollapsibleCard
                title=l!("ui_library.lookup.seam.title")
                detail=l!("ui_library.lookup.seam.detail")
                icon=Icon::Blocks
            >
                <Prose>{l!("ui_library.lookup.seam.body")}</Prose>
            </CollapsibleCard>
        </div>
    }
}

/// What the form specimen edits.
///
/// Two halves of one answer, and that is the point rather than an awkwardness:
/// a table picker hands back a row, and afterwards there is no id-to-label map
/// anywhere for the form to consult. A draft holding `code` alone would redraw
/// the field empty the next time the form rendered. Every real entity that
/// references another one already stores the name for the same reason.
#[derive(Clone, Debug, Default, PartialEq)]
struct Payment {
    reference: String,
    currency_code: String,
    currency_name: String,
}

/// A `FormConfig` that asks for a record picker.
///
/// Nothing is submitted anywhere - the one action reports the draft into
/// `stored` - because what is worth seeing here is the field, not a round trip
/// to a table the showcase would have to invent.
fn payment_form(stored: RwSignal<Option<Payment>>) -> FormConfig<Payment> {
    FormConfig::new("ui-library-payment", |draft: Payment| async move {
        Ok::<_, String>(phonix_core::form::Submission::Saved(draft))
    })
    .field(
        Field::text("reference", l!("ui_library.lookup.form.reference"), |p: &Payment| {
            FieldValue::text(&p.reference)
        })
        .writing(|p, value| p.reference = value.as_input()),
    )
    .field(
        Field::lookup(
            "currency",
            l!("ui_library.lookup.form.currency"),
            Choices::table(|answer: Callback<Choice>| {
                let config = currencies_picker(Callback::new(move |row: WorkspaceCurrency| {
                    answer.run(
                        Choice::new(row.currency.code(), row.currency.name())
                            .detail(row.display()),
                    );
                }));

                view! { <DataGrid config=config /> }.into_any()
            }),
            |p: &Payment| {
                FieldValue::record(
                    (!p.currency_code.is_empty())
                        .then(|| Choice::new(&p.currency_code, &p.currency_name)),
                )
            },
        )
        .writing(|p, value| {
            let chosen = value.as_records().into_iter().next();

            p.currency_name = chosen
                .as_ref()
                .map(|choice| choice.label.clone())
                .unwrap_or_default();
            p.currency_code = chosen.map(|choice| choice.value).unwrap_or_default();
        })
        .required()
        .help(l!("ui_library.lookup.form.currency_help"))
        .adding(QuickAdd::page(
            l!("ui_library.lookup.page"),
            "/admin/settings?tab=currencies",
        )),
    )
    .action(FormAction::run(
        l!("ui_library.lookup.form.action"),
        move |draft: Payment| stored.set(Some(draft)),
    ))
}

/// The form, and the draft it would submit.
///
/// Echoed for the same reason the hand-placed specimens are: a lookup that
/// looks right and writes the wrong thing into the draft is invisible from the
/// control, and this is the only place it shows.
#[component]
fn lookup_in_a_form() -> impl IntoView {
    let stored = RwSignal::new(None::<Payment>);

    view! {
        <div class="space-y-3">
            <EntityForm config=payment_form(stored) value=Payment::default() />

            <p class="font-mono text-2xs text-content-subtle">
                {move || match stored.get() {
                    None => l!("ui_library.lookup.form.nothing"),
                    Some(payment) => format!("{payment:?}"),
                }}
            </p>
        </div>
    }
}

/// One lookup, labelled, with what it currently holds written underneath.
#[component]
fn specimen(
    #[prop(into)] title: String,
    #[prop(into)] detail: String,
    chosen: RwSignal<Vec<Choice>>,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="space-y-1.5">
            <p class="text-sm font-medium text-content">{title}</p>
            <p class="max-w-long text-xs leading-relaxed text-content-subtle">{detail}</p>
            {children()}
            <p class="font-mono text-2xs text-content-subtle">
                {move || {
                    let chosen = chosen.get();
                    if chosen.is_empty() {
                        l!("ui_library.lookup.empty")
                    } else {
                        chosen
                            .iter()
                            .map(|choice| choice.value.clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                }}
            </p>
        </div>
    }
}

/// The quick-add form, for the specimen that has one.
///
/// A real service call, not a stub. The claim being demonstrated is that a
/// value discovered to be missing can be created without leaving the form, and
/// a fake one would demonstrate a dialog closing.
#[component]
fn add_currency(answer: Callback<Choice>) -> impl IntoView {
    let code = RwSignal::new(String::new());
    let symbol = RwSignal::new(String::new());
    let failed = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let typed = code.get_untracked().trim().to_uppercase();
        if typed.is_empty() {
            return;
        }

        saving.set(true);
        failed.set(None);
        let wanted = symbol.get_untracked().trim().to_owned();

        leptos::task::spawn_local(async move {
            let result =
                save_currency(typed.clone(), true, (!wanted.is_empty()).then_some(wanted)).await;
            let _ = saving.try_set(false);

            match result {
                Ok(list) => {
                    // Answered from what the server stored rather than from
                    // what was typed: the service knows the currency's real
                    // name, and echoing the input back would put "usd" in a
                    // field that should read "US Dollar".
                    let stored = list.iter().find(|row| row.currency.code() == typed).map(
                        |row| {
                            Choice::new(row.currency.code(), row.currency.name())
                                .detail(row.display())
                        },
                    );

                    match stored {
                        Some(choice) => answer.run(choice),
                        None => {
                            let _ = failed.try_set(Some(l!("ui_library.lookup.add.missing")));
                        }
                    }
                }
                Err(err) => {
                    let _ = failed.try_set(Some(err.to_string()));
                }
            }
        });
    };

    view! {
        <form class="space-y-3" on:submit=submit>
            <Notice message=Signal::derive(move || failed.get()) tone=Tone::Danger />

            <div>
                <label for="quick-currency-code" class="text-sm font-medium text-content">
                    {l!("field.code")}
                </label>
                <input
                    id="quick-currency-code"
                    class="mt-1"
                    maxlength="3"
                    placeholder="EUR"
                    prop:value=move || code.get()
                    on:input=move |event| code.set(event_target_value(&event))
                />
            </div>

            <div>
                <label for="quick-currency-symbol" class="text-sm font-medium text-content">
                    {l!("field.symbol")}
                </label>
                <input
                    id="quick-currency-symbol"
                    class="mt-1"
                    maxlength="8"
                    prop:value=move || symbol.get()
                    on:input=move |event| symbol.set(event_target_value(&event))
                />
            </div>

            <FormActions>
                <PrimaryButton
                    label=l!("common.add")
                    icon=Icon::Plus
                    button_type="submit"
                    pending=Signal::derive(move || saving.get())
                />
            </FormActions>
        </form>
    }
}

/// What is agreed to be built, and where each one has got to.
///
/// Drawn with the card it is the roadmap for, which is deliberate: the entry
/// for a component nobody has written yet still demonstrates the one that is
/// finished.
#[component]
fn roadmap_tab() -> impl IntoView {
    view! {
        <Panel title=l!("ui_library.roadmap.title") description=l!("ui_library.roadmap.detail")>
            <div class="space-y-3">
                <CollapsibleCard
                    title=l!("ui_library.cards.title")
                    detail=l!("ui_library.cards.detail")
                    icon=Icon::LayoutGrid
                >
                    <Status tone=Tone::Success label=l!("ui_library.status.built") />
                    <Prose>{l!("ui_library.roadmap.cards.body")}</Prose>
                </CollapsibleCard>

                <CollapsibleCard
                    title=l!("ui_library.roadmap.editor.title")
                    detail=l!("ui_library.roadmap.editor.detail")
                    icon=Icon::Pencil
                >
                    <Status tone=Tone::Success label=l!("ui_library.status.built") />
                    <Prose>{l!("ui_library.roadmap.editor.body")}</Prose>
                </CollapsibleCard>

                <CollapsibleCard
                    title=l!("ui_library.roadmap.select.title")
                    detail=l!("ui_library.roadmap.select.detail")
                    icon=Icon::Search
                >
                    <Status tone=Tone::Success label=l!("ui_library.status.built") />
                    <Prose>{l!("ui_library.roadmap.select.body")}</Prose>
                </CollapsibleCard>

                <CollapsibleCard
                    title=l!("ui_library.roadmap.rows.title")
                    detail=l!("ui_library.roadmap.rows.detail")
                    icon=Icon::ClipboardList
                >
                    <Status tone=Tone::Success label=l!("ui_library.status.built") />
                    <Prose>{l!("ui_library.roadmap.rows.body")}</Prose>
                </CollapsibleCard>

                // Not built, and here rather than in a notebook because this
                // page is where the agreed list lives.
                <CollapsibleCard
                    title=l!("ui_library.roadmap.forms.title")
                    detail=l!("ui_library.roadmap.forms.detail")
                    icon=Icon::ClipboardList
                >
                    <Status tone=Tone::Neutral label=l!("ui_library.status.planned") />
                    <Prose>{l!("ui_library.roadmap.forms.body")}</Prose>
                </CollapsibleCard>
            </div>
        </Panel>
    }
}

/// [`RichText`], writable and read-only, with what it is holding underneath.
///
/// Two editors on one page on purpose: the bundle is fetched once and both
/// mount from it, which is the arrangement `ui::editor::browser` exists to
/// guarantee and the only way to see that it does.
#[component]
fn editor_tab() -> impl IntoView {
    // Seeded rather than empty. An editor with nothing in it demonstrates a
    // border; what wants looking at is how a heading, a list and a table are
    // set, and whether the read-only copy sets them the same way.
    let document = RwSignal::new(SPECIMEN.to_owned());

    // Compiled in by the build script rather than guessed at, and gzipped
    // rather than raw: the server compresses what it serves, so that is the
    // number somebody deciding whether to add an extension has to weigh.
    let weight = t(&Message::new("ui_library.editor.weight")
        .arg("size", (EDITOR_GZIP_BYTES / 1024).to_string()));

    view! {
        <div class="space-y-3">
            <Panel title=l!("ui_library.editor.title") description=l!("ui_library.editor.detail")>
                <div class="space-y-3">
                    <p class="text-xs text-content-subtle">{weight}</p>
                    <RichText value=document label=l!("ui_library.editor.title") />
                </div>
            </Panel>

            // Beside the first rather than inside a collapsible card, for a
            // reason worth knowing: ProseMirror measures the width of a table's
            // columns when it mounts, and a card that arrives closed mounts it
            // into a subtree the browser is not laying out. It recovers on the
            // first interaction, but a demonstration that has to be poked
            // before it looks right is not one.
            <Panel
                title=l!("ui_library.editor.disabled")
                description=l!("ui_library.editor.disabled_detail")
            >
                // The same signal as above: typing in the first changes what
                // this one shows, which is also how a form pushes a reset or a
                // reloaded draft into a field.
                <RichText value=document disabled=true label=l!("ui_library.editor.disabled") />
            </Panel>

            <CollapsibleCard
                title=l!("ui_library.editor.source")
                detail=l!("ui_library.editor.stored_detail")
                icon=Icon::FileText
            >
                <pre class="max-h-64 overflow-auto rounded-control bg-surface-sunken p-3 font-mono text-2xs leading-relaxed text-content-muted">
                    {move || document.get()}
                </pre>
            </CollapsibleCard>
        </div>
    }
}

/// What the editor opens holding.
///
/// Markup rather than a catalog key: it is a specimen of *structure* - a
/// heading, a list, a link, a table - and translating the words inside it
/// would not make the structure any clearer to somebody reading it in German.
const SPECIMEN: &str = concat!(
    "<h2>Payment terms</h2>",
    "<p>Net <strong>30 days</strong> from the date of invoice. ",
    "Late payment carries interest at the statutory rate.</p>",
    "<ul><li>Bank transfer preferred</li><li>Reference the invoice number</li></ul>",
    "<table><tbody>",
    "<tr><th>Method</th><th>Settles in</th></tr>",
    "<tr><td>Transfer</td><td>1-2 days</td></tr>",
    "<tr><td>Card</td><td>Same day</td></tr>",
    "</tbody></table>",
    "<p>See <a href=\"https://example.com/terms\">the full terms</a>.</p>",
);

/// A paragraph inside a card, measured so it stays readable.
///
/// `max-w-long` because a card is as wide as the page and a line of prose
/// that wide is one the eye loses its place in. The grids and tables around it
/// want the full width; this does not.
#[component]
fn prose(children: Children) -> impl IntoView {
    view! {
        <p class="max-w-long text-sm leading-relaxed text-content-muted">{children()}</p>
    }
}

/// Where a roadmap entry has got to.
#[component]
fn status(label: String, tone: Tone) -> impl IntoView {
    view! {
        <div class="mb-2">
            <Badge label=label tone=tone />
        </div>
    }
}

/// [`SelectField`] at both sides of its one threshold, and with a way back to
/// nothing.
///
/// Three of them rather than one, because the two things worth seeing are the
/// differences: whether the panel grows a filter box, and whether the field
/// offers an empty answer at all. A single specimen shows a dropdown, which
/// nobody needed a page to see.
#[component]
fn select_specimens() -> impl IntoView {
    let short = RwSignal::new("line".to_owned());
    let long = RwSignal::new(String::new());
    let optional = RwSignal::new(String::new());

    // Under the threshold, so the panel opens without a filter box.
    let rounding = vec![
        Choice::new("line", l!("ui_library.select.short")).detail("1"),
        Choice::new("document", l!("ui_library.select.long")).detail("2"),
        Choice::new("none", l!("ui_library.select.nothing")).detail("3"),
    ];
    let currencies = Currency::all()
        .iter()
        .map(|currency| Choice::new(currency.code(), currency.label()))
        .collect::<Vec<_>>();
    let optional_choices = currencies.clone();

    view! {
        <div class="grid gap-6 lg:grid-cols-3">
            <div class="space-y-1.5">
                <p class="text-sm font-medium text-content">{l!("ui_library.select.short")}</p>
                <p class="text-xs text-content-muted">{l!("ui_library.select.short.detail")}</p>
                <SelectField
                    value=Signal::derive(move || short.get())
                    on_change=Callback::new(move |value: String| short.set(value))
                    options=rounding
                />
                <Echo of=short />
            </div>

            <div class="space-y-1.5">
                <p class="text-sm font-medium text-content">{l!("ui_library.select.long")}</p>
                <p class="text-xs text-content-muted">{l!("ui_library.select.long.detail")}</p>
                <SelectField
                    value=Signal::derive(move || long.get())
                    on_change=Callback::new(move |value: String| long.set(value))
                    options=currencies
                />
                <Echo of=long />
            </div>

            <div class="space-y-1.5">
                <p class="text-sm font-medium text-content">
                    {l!("ui_library.select.clearable")}
                </p>
                <p class="text-xs text-content-muted">
                    {l!("ui_library.select.clearable.detail")}
                </p>
                <SelectField
                    value=Signal::derive(move || optional.get())
                    on_change=Callback::new(move |value: String| optional.set(value))
                    options=optional_choices
                    clearable=true
                />
                <Echo of=optional />
            </div>
        </div>
    }
}

/// What the field is actually holding.
///
/// The point of a showcase is the value underneath, not the control: a select
/// that looks right and answers with the label rather than the value is a bug
/// this page exists to catch.
#[component]
fn echo(of: RwSignal<String>) -> impl IntoView {
    view! {
        <p class="font-mono text-2xs text-content-subtle">
            {move || {
                let held = of.get();
                if held.is_empty() { l!("ui_library.select.nothing") } else { held }
            }}
        </p>
    }
}
