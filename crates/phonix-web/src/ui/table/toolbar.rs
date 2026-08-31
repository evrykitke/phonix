//! The bar above the table: search and filters on the left, actions on the
//! right.
//!
//! # Deliberately not generic
//!
//! Nothing here knows the row type. The toolbar is handed the *decisions* -
//! which columns exist, which actions the viewer may see, what to call when
//! export is chosen - rather than the data, so it is compiled once no matter
//! how many entities have grids. Generic code that does not need to be generic
//! is compile time paid for nothing.
//!
//! # Three kinds of action share one bar
//!
//! * **Configured** - "New user", "Import". They come from the entity and are
//!   permission-gated by it.
//! * **Export** - present when the configuration named a file stem.
//! * **Columns** - present when any column can be turned off.
//!
//! The last two are the grid's own and carry no permission: they act on what
//! the viewer is already looking at. Hiding a column the viewer can see, or
//! saving rows they have already been sent, protects nothing.
//!
//! # The gated ones sit behind a boundary
//!
//! Only the configured actions read the viewer, and what that read decides is
//! whether a button *exists*. That makes it one of the reads that must wait for
//! the session rather than correct itself when it arrives - so it has a
//! `<Suspense>` of its own, and the export and column buttons beside it do not
//! sit inside it. See the note at the read.

use leptos::prelude::*;
use leptos_router::components::A;
use phonix_core::identity::AuthUser;

use super::action::{ToolbarAction, ToolbarKind};
use super::date::DateControl;
use super::date_picker::DateRangePicker;
use super::filter::FilterChoice;
use super::state::GridState;
use crate::icons::{Icon, IconSize};
use crate::ui::form::field::Choice;
use crate::ui::lookup::SelectField;
use crate::l;

/// One filter, as the bar needs to know it.
///
/// The predicate is deliberately not here. Whether a row survives is answered
/// where the rows are - in the browser for an in-memory grid, in SQL for a
/// paged one - and a bar that held the closure would have to be generic over
/// the row type to hold it. See [`Filter`](super::filter::Filter).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterControl {
    pub key: &'static str,
    pub label: String,
    pub choices: Vec<FilterChoice>,
}

/// One column, as the column menu needs to know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnChoice {
    pub field: &'static str,
    pub header: String,
    /// A column that must stay is listed and cannot be unticked, rather than
    /// left out - a menu that silently omits a column is a menu people go
    /// looking through twice.
    pub hideable: bool,
}

#[component]
pub fn grid_toolbar(
    grid_id: &'static str,
    state: GridState,
    /// Every configured action, gated inside rather than out, so the bar keeps
    /// its order as permissions change.
    actions: Vec<ToolbarAction>,
    user: Signal<Option<AuthUser>>,
    /// `None` hides the search box. `optional_no_strip` so the caller can pass
    /// the configuration's own `Option` straight through.
    #[prop(optional_no_strip)]
    search_placeholder: Option<String>,
    /// The narrowings this grid offers. Empty for most grids.
    filters: Vec<FilterControl>,
    /// The spans of time this grid offers. Empty for most grids.
    dates: Vec<DateControl>,
    columns: Vec<ColumnChoice>,
    /// `Some` when the configuration asked for an export.
    #[prop(optional_no_strip)]
    on_export: Option<Callback<()>>,
) -> impl IntoView {
    let has_hideable = columns.iter().any(|column| column.hideable);
    let actions = StoredValue::new(actions);

    view! {
        // Search at the start, every action at the end. `justify-between` puts
        // that in the container rather than in a margin on one child: with two
        // children it is the whole arrangement, and when the bar wraps each
        // line still starts at the left instead of drifting right.
        <div class="flex flex-wrap items-center justify-between gap-2">
            // Search and the filters together at the start: they are one
            // thought - "which rows" - and separating them would put half the
            // narrowing at each end of the bar.
            <div class="flex min-w-0 flex-1 flex-wrap items-center gap-2">
                {search_placeholder
                    .map(|placeholder| {
                        view! { <SearchBox grid_id=grid_id state=state placeholder=placeholder /> }
                    })}

                {filters
                    .into_iter()
                    .map(|filter| view! { <FilterSelect state=state filter=filter /> })
                    .collect::<Vec<_>>()}

                // After the dropdowns: "which rows" reads left to right from
                // the broadest question to the narrowest, and a calendar is
                // the widest control on the bar.
                {dates
                    .into_iter()
                    .map(|control| {
                        view! { <DateRangePicker state=state control=control /> }
                    })
                    .collect::<Vec<_>>()}
            </div>

            <div class="flex flex-wrap items-center gap-2">
                // The viewer is a `Signal` over the shell's *blocking*
                // resource, and this read decides how many buttons exist.
                // Outside a boundary that is fatal rather than cosmetic: on
                // the server `initial` is `None`, so every gated action is
                // filtered out and nothing is drawn, while the browser has the
                // answer already serialized into the document and draws them.
                // The two sides then disagree about the node count, which is
                // an unrecoverable hydration error, and a wasm panic takes the
                // whole page with it.
                //
                // It only ever showed on a *fresh load* of a list screen,
                // because navigating to one inside the app renders rather than
                // hydrates - which is why eight grids carried it for as long
                // as they did.
                //
                // Blocking resources resolve before the first paint, so
                // waiting here costs no flash. Fixed in the kit rather than in
                // a configuration, for the reason the same fix to
                // `ui/form/mod.rs` was: every grid has this bar.
                <Suspense fallback=|| ()>
                    {move || {
                        let user = user.get();

                        actions
                            .with_value(|actions| {
                                actions
                                    .iter()
                                    .filter(|action| action.permitted(user.as_ref()))
                                    .cloned()
                                    .map(|action| view! { <ToolbarButton action=action /> })
                                    .collect::<Vec<_>>()
                            })
                    }}
                </Suspense>

                {on_export
                    .map(|on_export| {
                        view! {
                            <QuietButton
                                label=l!("grid.export")
                                icon=Icon::Download
                                on_click=Callback::new(move |()| on_export.run(()))
                            />
                        }
                    })}

                {has_hideable
                    .then(|| {
                        view! {
                            <QuietButton
                                label=l!("grid.columns")
                                icon=Icon::SlidersHorizontal
                                on_click=Callback::new(move |()| state.columns_open.set(true))
                            />
                        }
                    })}
            </div>
        </div>

        {has_hideable.then(|| view! { <ColumnMenu state=state columns=columns /> })}
    }
}

/// One narrowing, as a dropdown.
///
/// A dropdown rather than a row of chips: the choices are exclusive and there
/// are usually three of them, which is what a dropdown is for. `SelectField`
/// rather than the browser's own - see [`ui::lookup::select`] for why - so this
/// control carries only its size.
///
/// [`ui::lookup::select`]: crate::ui::lookup::SelectField
#[component]
fn filter_select(state: GridState, filter: FilterControl) -> impl IntoView {
    let key = filter.key;
    let options = filter
        .choices
        .iter()
        .map(|choice| Choice::new(choice.value, choice.label.clone()))
        .collect::<Vec<_>>();

    view! {
        <SelectField
            value=Signal::derive(move || state.filter(key))
            on_change=Callback::new(move |value: String| state.set_filter(key, value))
            options=options
            label=filter.label.clone()
            // Only the size. The `all` choice carries the empty value, so an
            // unnarrowed table already reads as "All" without this having to
            // say what nothing means.
            class="h-8 w-auto shrink-0"
        />
    }
}

#[component]
fn search_box(grid_id: &'static str, state: GridState, placeholder: String) -> impl IntoView {
    let id = format!("{grid_id}-search");

    view! {
        <div class="flex h-8 min-w-[10rem] flex-1 items-center gap-2 rounded-control border border-edge bg-surface-raised px-2 sm:max-w-xs">
            <Icon icon=Icon::Search size=IconSize::Xs class="shrink-0 text-content-subtle" />
            <input
                type="search"
                id=id.clone()
                // `type=search` gives Chrome its own clear button; this one is
                // for every other browser, and for the keyboard.
                // `control-bare`: the border and background belong to the box
                // around this input, not to the input.
                class="control-bare w-full bg-transparent text-sm text-content outline-none"
                placeholder=placeholder.clone()
                aria-label=placeholder
                aria-controls=format!("{grid_id}-table")
                prop:value=move || state.search.get()
                on:input=move |event| state.set_search(event_target_value(&event))
            />
            <Show when=move || state.is_searching() fallback=|| ()>
                <button
                    type="button"
                    class="shrink-0 text-content-subtle hover:text-content"
                    aria-label=l!("grid.clear_search")
                    on:click=move |_| state.clear_search()
                >
                    <Icon icon=Icon::X size=IconSize::Xs />
                </button>
            </Show>
        </div>
    }
}

#[component]
fn toolbar_button(action: ToolbarAction) -> impl IntoView {
    let primary = "inline-flex h-8 items-center gap-1.5 rounded-control bg-brand px-3 text-sm \
                   font-medium text-on-brand hover:bg-brand-hover";
    let quiet = "inline-flex h-8 items-center gap-1.5 rounded-control border border-edge px-3 \
                 text-sm text-content-muted hover:bg-surface-hover hover:text-content";
    let class = if action.primary { primary } else { quiet };

    match action.kind {
        ToolbarKind::Link(href) => view! {
            <A href=href attr:class=class>
                <Icon icon=action.icon size=IconSize::Xs />
                {action.label}
            </A>
        }
        .into_any(),
        ToolbarKind::Run(run) => view! {
            <button type="button" class=class on:click=move |_| run.run(())>
                <Icon icon=action.icon size=IconSize::Xs />
                {action.label}
            </button>
        }
        .into_any(),
    }
}

/// The grid's own buttons: quieter than anything the entity contributes.
#[component]
fn quiet_button(#[prop(into)] label: String, icon: Icon, on_click: Callback<()>) -> impl IntoView {
    // Three copies bound up front rather than cloned inside the markup: the
    // view macro decides the order it evaluates attributes in, and a `.clone()`
    // written in the last of them is not guaranteed to run before the move.
    let spoken = label.clone();
    let hover = label.clone();

    view! {
        <button
            type="button"
            class="inline-flex h-8 items-center gap-1.5 rounded-control border border-edge px-2.5 text-sm text-content-muted hover:bg-surface-hover hover:text-content"
            on:click=move |_| on_click.run(())
            // Below `sm` the label is hidden and this is the only name the
            // button has, to a screen reader or to anyone hovering it.
            aria-label=spoken
            title=hover
        >
            <Icon icon=icon size=IconSize::Xs />
            <span class="hidden sm:inline">{label}</span>
        </button>
    }
}

/// Which columns are on, as a modal.
///
/// A modal rather than a dropdown because the list is as long as the table is
/// wide, and a menu that runs off the bottom of a phone is a menu with columns
/// nobody can reach.
#[component]
fn column_menu(state: GridState, columns: Vec<ColumnChoice>) -> impl IntoView {
    let columns = StoredValue::new(columns);

    // Escape closes it. Bound on the window rather than the dialog so it works
    // before anything inside has been focused.
    Effect::new(move |_| {
        let handle = window_event_listener(leptos::ev::keydown, move |event| {
            if event.key() == "Escape" {
                state.columns_open.set(false);
            }
        });

        on_cleanup(move || handle.remove());
    });

    view! {
        <Show when=move || state.columns_open.get() fallback=|| ()>
            <div
                // `inset-0` already sizes this to the viewport; a `max-h-dvh`
                // on top of it only shrinks the box the panel is centred in,
                // which showed up as a sheet sitting slightly low. The height
                // cap belongs on the panel, below.
                class="fixed inset-0 z-50 flex items-center justify-center overflow-y-auto bg-overlay px-4 py-4"
                role="dialog"
                aria-modal="true"
                aria-label=l!("grid.choose_columns")
                on:click=move |_| state.columns_open.set(false)
            >
                <div
                    class="flex max-h-[85dvh] w-full max-w-sm flex-col overflow-hidden rounded-pop border border-edge bg-surface-raised shadow-pop"
                    // The backdrop closes on click; the panel must not, or
                    // ticking a box would dismiss the dialog.
                    on:click=|event| event.stop_propagation()
                >
                    <div class="flex items-center justify-between gap-2 border-b border-edge px-3 py-2">
                        <span class="text-sm font-medium text-content">{l!("grid.columns")}</span>
                        <button
                            type="button"
                            class="grid size-7 place-items-center rounded-control text-content-muted hover:bg-surface-hover hover:text-content"
                            aria-label=l!("common.close")
                            on:click=move |_| state.columns_open.set(false)
                        >
                            <Icon icon=Icon::X size=IconSize::Sm />
                        </button>
                    </div>

                    <ul class="min-h-0 flex-1 overflow-y-auto p-1">
                        {columns
                            .with_value(|columns| {
                                columns
                                    .iter()
                                    .cloned()
                                    .map(|column| view! { <ColumnRow state=state column=column /> })
                                    .collect::<Vec<_>>()
                            })}
                    </ul>

                    <div class="flex items-center justify-between gap-2 border-t border-edge px-3 py-2">
                        <button
                            type="button"
                            class="text-xs font-medium text-brand hover:underline"
                            on:click=move |_| state.show_all_columns()
                        >
                            {l!("grid.show_all_columns")}
                        </button>
                        <button
                            type="button"
                            class="inline-flex h-7 items-center rounded-control border border-edge px-2.5 text-xs text-content-muted hover:bg-surface-hover hover:text-content"
                            on:click=move |_| state.columns_open.set(false)
                        >
                            {l!("common.done")}
                        </button>
                    </div>
                </div>
            </div>
        </Show>
    }
}

#[component]
fn column_row(state: GridState, column: ColumnChoice) -> impl IntoView {
    let checked = move || !state.is_hidden(column.field);

    view! {
        <li>
            <label class=move || {
                let base = "flex items-center gap-2 rounded-control px-2 py-1.5 text-sm";
                if column.hideable {
                    format!("{base} cursor-pointer text-content hover:bg-surface-hover")
                } else {
                    format!("{base} cursor-not-allowed text-content-subtle")
                }
            }>
                <input
                    type="checkbox"
                    class="size-3.5 shrink-0 accent-brand"
                    prop:checked=checked
                    disabled=!column.hideable
                    on:change=move |_| {
                        if column.hideable {
                            state.toggle_column(column.field);
                        }
                    }
                />
                <span class="truncate-fade">{column.header}</span>
                {(!column.hideable)
                    .then(|| {
                        view! {
                            <span class="ms-auto shrink-0 text-2xs uppercase tracking-wide">
                                {l!("grid.column_always")}
                            </span>
                        }
                    })}
            </label>
        </li>
    }
}
