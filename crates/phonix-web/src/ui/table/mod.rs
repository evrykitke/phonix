//! The data grid: one component, one configuration per entity.
//!
//! # What it is for
//!
//! Every module ends up needing the same list: searchable, sortable, paged,
//! exportable, with actions on a row that only some people may see. Written per
//! screen, those come out slightly different every time - this one pages and
//! that one does not, this one searches the email column and that one forgot
//! to. Written once and configured, they cannot.
//!
//! ```ignore
//! #[component]
//! pub fn users_page() -> impl IntoView {
//!     view! {
//!         <PageHeader title="Users" icon=Icon::Users />
//!         <DataGrid config=users_grid() />
//!     }
//! }
//! ```
//!
//! The configuration is the extension point: [`config::users`] is what a users
//! grid *is*, and an inventory grid is another file beside it. There is no
//! `UsersDataGrid` type, because a subclass per entity is a copy per entity of
//! everything the entities have in common.
//!
//! # The pieces
//!
//! | Module      | What it decides                                    |
//! | ----------- | -------------------------------------------------- |
//! | [`column`]  | what a column is, and how one value is read         |
//! | [`filter`]  | named narrowings, and where each is answered        |
//! | [`date`]    | a span of time as a narrowing, and the names for it |
//! | [`action`]  | what may be done, and who may see it                |
//! | [`source`]  | where rows come from - all at once, or a page       |
//! | [`config`]  | the whole configuration, and one file per entity    |
//! | [`state`]   | what the viewer has done to the table               |
//! | [`local`]   | searching, sorting and paging in the browser        |
//! | [`export`]  | the table as a CSV file                             |
//! | [`toolbar`] | the bar above, and the column menu                  |
//! | [`date_picker`] | the calendar the span is chosen on              |
//! | [`pager`]   | the strip below                                     |
//! | [`handle`]  | how an action refreshes the grid it ran in          |
//!
//! # The three things worth knowing before changing it
//!
//! **One extractor per column.** Search, sort, export and display all read the
//! same [`Cell`]. A renderer changes how a value looks and never what it is,
//! which is what keeps a status badge sorting under the word inside it. See
//! [`column`].
//!
//! **Gating here is cosmetic.** Actions carry the permission they need and the
//! grid hides the rest, but that is a tidy screen, not a control. The service
//! behind every action must call `Caller::require` itself - a grid cannot stop
//! a request it never rendered a button for.
//!
//! **The table is a `Transition`, not a `Suspense`.** Both wait for rows; only
//! a transition keeps the old ones on screen while the new ones come. Under a
//! `Suspense` every keystroke in the search box would blank the table back to
//! its skeleton and every page turn would flash, because re-suspending is
//! indistinguishable from loading for the first time.
//!
//! **A reloading table does not accept clicks.** That is the price of the line
//! above: the old rows stay on screen, but their reactive owner is disposed the
//! instant the refetch starts, so for one round trip the table is markup with
//! nothing behind it. `set_pending` marks it inert for exactly that window -
//! `pointer-events-none` at once, dimmed a beat later so a fast refetch does
//! not flash. The row handlers guard themselves too, in `menu`, because the
//! flag arrives one tick after the disposal.
//!
//! **Reactive reads happen outside the async block.** The table is rendered
//! inside a `Suspend`, and signals read after an `await` are not tracked. Every
//! piece of state the table depends on - the request, the hidden columns - is
//! read in the closure *around* the `Suspend` and handed in as a value.
//! Forgetting that produces a table that renders once and then ignores its own
//! search box.

pub mod action;
pub mod column;
pub mod config;
pub mod date;
pub mod date_picker;
pub mod export;
pub mod filter;
pub mod handle;
pub mod local;
pub mod menu;
pub mod pager;
pub mod source;
pub mod state;
pub mod toolbar;

use std::collections::BTreeSet;

use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_navigate;
use phonix_core::identity::AuthUser;
use phonix_core::query::{Page, SortDirection};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use action::{ActionKind, RowAction, ToolbarAction};
pub use column::{Align, Cell, Column};
pub use config::{GridConfig, Pagination};
pub use date::{DateFilter, DatePreset};
pub use filter::{Filter, FilterChoice};
pub use handle::GridHandle;
pub use menu::RowMenu;
pub use source::Source;
pub use state::GridState;

use self::date::DateControl;
use self::handle::GridNotice;
use self::pager::{Footing, GridPager};
use self::toolbar::{ColumnChoice, FilterControl, GridToolbar};
use crate::components::page::{EmptyState, Notice, Tone};
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::ui::viewer::Viewer;

/// What a row type has to be for a grid to hold it.
///
/// `Serialize`/`DeserializeOwned` because rows are fetched by a resource, which
/// serialises what the server rendered so the browser does not fetch it again.
/// The padding of one cell, header and body alike.
///
/// Density is the thing a list screen spends its space on, so it is one
/// constant rather than a number repeated at six call sites. `py-1.5` with
/// tight leading inside the cells is about as far as this goes before a row
/// stops being a comfortable touch target - the controls inside it are still
/// 28px, which is what actually has to be hit.
const CELL: &str = "px-2 py-1.5 sm:px-3";

pub trait GridRow: Clone + Send + Sync + Serialize + DeserializeOwned + 'static {}

impl<T: Clone + Send + Sync + Serialize + DeserializeOwned + 'static> GridRow for T {}

/// Which columns are turned off right now.
type Hidden = BTreeSet<&'static str>;

/// The last thing the source handed over.
///
/// Kept so that the export - which runs from a click, long after hydration -
/// has rows to write without reading the resource. Reading a resource outside a
/// `<Transition/>` is exactly how a hydration mismatch is made, and leptos says
/// so in the console every time.
enum Loaded<T> {
    All(Vec<T>),
    Page(Page<T>),
}

/// A configurable table.
///
/// See the [module documentation](self) for what goes in the configuration.
#[component]
pub fn data_grid<T: GridRow>(config: GridConfig<T>) -> impl IntoView {
    let state = GridState::new(&config);
    let viewer = Viewer::get();
    let notice = RwSignal::new(None::<GridNotice>);

    // Whether the rows on the screen are the rows the grid is still fetching.
    // See `reloading` below for what it is for; `<Transition/>` sets it.
    let reloading = RwSignal::new(false);

    // Fixed for the life of the grid, so they are taken now rather than read
    // out of the configuration on every render.
    let choices: Vec<ColumnChoice> = config
        .columns
        .iter()
        .map(|column| ColumnChoice {
            field: column.field,
            header: column.header.clone(),
            hideable: column.hideable,
        })
        .collect();

    let filter_controls: Vec<FilterControl> = config
        .filters
        .iter()
        .map(|filter| FilterControl {
            key: filter.key,
            label: filter.label.clone(),
            choices: filter.choices.clone(),
        })
        .collect();

    let date_controls: Vec<DateControl> =
        config.date_filters.iter().map(DateControl::from).collect();

    let toolbar_actions = config.toolbar.clone();
    let search_placeholder = config.search_placeholder.clone();
    let export_stem = config.export_stem;
    let per_page_choices = config.pagination.choices;
    let grid_id = config.id;

    let rows = Rows::open(&config.source, state);
    let loaded: StoredValue<Option<Loaded<T>>> = StoredValue::new(None);
    let config = StoredValue::new(config);

    let handle = GridHandle {
        refetch: Callback::new(move |()| rows.refetch()),
        notice,
    };

    let on_export =
        export_stem.map(|stem| Callback::new(move |()| export_now(stem, config, state, loaded)));

    view! {
        <div class="space-y-3">
            <GridToolbar
                grid_id=grid_id
                state=state
                actions=toolbar_actions
                user=viewer
                search_placeholder=search_placeholder
                filters=filter_controls
                dates=date_controls
                columns=choices
                on_export=on_export
            />

            {move || {
                notice
                    .get()
                    .map(|notice| {
                        let message = notice.message.clone();

                        view! {
                            <Notice
                                message=Signal::derive(move || Some(message.clone()))
                                tone=notice.tone
                            />
                        }
                    })
            }}

            // Inert while it reloads. A transition holds the previous rows on
            // the screen until the next page arrives, but their reactive owner
            // is disposed the moment the refetch starts - so for one round trip
            // the table looks live and every signal behind it is gone. Clicking
            // one of those rows used to be a panic, and a panic in wasm freezes
            // the whole page.
            //
            // The handlers guard themselves as well (see the note on zombie
            // rows in `menu`), because the pending flag is set by an effect and
            // is therefore a tick behind the disposal. This is the part that
            // makes the window unreachable rather than merely survivable, and
            // it is also the honest answer: rows that are being replaced should
            // not accept a click.
            //
            // Only the table. The toolbar above stays live, so a new search or
            // filter can be typed over a slow load.
            //
            // Dimming is delayed and eased, the block is not: a refetch that
            // finishes in 30ms should not flash, but it must not accept a click
            // in those 30ms either.
            <div
                class="overflow-hidden rounded-card border border-edge bg-surface-raised transition-opacity delay-150 duration-200"
                class:pointer-events-none=move || reloading.get()
                class:opacity-60=move || reloading.get()
                aria-busy=move || if reloading.get() { "true" } else { "false" }
            >
                <Transition fallback=|| view! { <GridSkeleton /> } set_pending=reloading>
                    {move || {
                        // Read reactively *here* and hand plain values to the
                        // async block. See the note in the module docs: a
                        // signal read after an await is not tracked.
                        let request = state.request();
                        let hidden = state.hidden.get();
                        let searching = state.is_searching();
                        let user = viewer.get();

                        Suspend::new(async move {
                            let page = match rows {
                                Rows::All(rows) => {
                                    rows.await
                                        .map(|all| {
                                            // Moved in, then read back out, so
                                            // the whole list is not cloned once
                                            // per keystroke.
                                            loaded.set_value(Some(Loaded::All(all)));

                                            loaded
                                                .with_value(|held| match held {
                                                    Some(Loaded::All(all)) => {
                                                        config
                                                            .with_value(|config| {
                                                                local::apply(
                                                                    &request,
                                                                    &config.columns,
                                                                    &config.filters,
                                                                    &config.date_filters,
                                                                    all,
                                                                )
                                                            })
                                                    }
                                                    _ => Page::empty(&request),
                                                })
                                        })
                                }
                                Rows::Paged(rows) => {
                                    rows.await
                                        .inspect(|page| {
                                            loaded.set_value(Some(Loaded::Page(page.clone())));
                                        })
                                }
                            };

                            match page {
                                Ok(page) => {
                                    view! {
                                        <GridBody
                                            config=config
                                            state=state
                                            user=user
                                            hidden=hidden
                                            searching=searching
                                            handle=handle
                                            page=page
                                            per_page_choices=per_page_choices
                                        />
                                    }
                                        .into_any()
                                }
                                Err(message) => {
                                    view! {
                                        <div class="p-3">
                                            <Notice
                                                message=Signal::derive(move || {
                                                    Some(message.clone())
                                                })
                                                tone=Tone::Danger
                                            />
                                        </div>
                                    }
                                        .into_any()
                                }
                            }
                        })
                    }}
                </Transition>
            </div>
        </div>
    }
}

/// The grid's rows, whichever way they arrive.
///
/// Two resources rather than one because the *key* differs, and the key is the
/// whole difference between the two sources: an in-memory grid fetches once and
/// never again, so its resource is keyed on nothing; a paged grid fetches per
/// request, so its resource is keyed on the request. Keying the first on the
/// request would refetch the entire list on every keystroke.
enum Rows<T: Send + Sync + 'static> {
    All(Resource<Result<Vec<T>, String>>),
    Paged(Resource<Result<Page<T>, String>>),
}

// Copy by hand rather than derived: `#[derive(Copy)]` would demand `T: Copy`,
// which no row type is. The handle inside is a copyable arena index whatever
// the rows are.
impl<T: Send + Sync + 'static> Clone for Rows<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Send + Sync + 'static> Copy for Rows<T> {}

impl<T: GridRow> Rows<T> {
    fn open(source: &Source<T>, state: GridState) -> Self {
        match source.clone() {
            Source::InMemory(load) => Self::All(Resource::new(
                || (),
                move |()| {
                    let load = load.clone();

                    async move { load().await }
                },
            )),
            // Keyed on the request, so changing the page, the sort or the
            // search is what asks the server again.
            Source::Paged(load) => Self::Paged(Resource::new(
                move || state.request(),
                move |request| {
                    let load = load.clone();

                    async move { load(request).await }
                },
            )),
        }
    }

    fn refetch(self) {
        match self {
            Self::All(rows) => rows.refetch(),
            Self::Paged(rows) => rows.refetch(),
        }
    }
}

/// The table itself, once there are rows to put in it.
#[component]
fn grid_body<T: GridRow>(
    config: StoredValue<GridConfig<T>>,
    state: GridState,
    user: Option<AuthUser>,
    hidden: Hidden,
    /// Whether the viewer has typed something - which decides whether an empty
    /// table says "nothing here" or "nothing matches".
    searching: bool,
    handle: GridHandle,
    page: Page<T>,
    per_page_choices: &'static [u32],
) -> impl IntoView {
    if page.is_empty() {
        let empty = config.with_value(|config| {
            if searching {
                config.no_matches.clone()
            } else {
                config.empty.clone()
            }
        });

        return view! { <EmptyState icon=empty.icon title=empty.title detail=empty.detail /> }
            .into_any();
    }

    let footing = Footing {
        page: page.page,
        pages: page.page_count(),
        total: page.total,
        first: page.first_row_number(),
        last: page.last_row_number(),
    };

    // The actions this viewer may see at all. Whether each one applies to a
    // given row is asked per row.
    let actions: Vec<RowAction<T>> = config.with_value(|config| {
        config
            .actions
            .iter()
            .filter(|action| action.permitted(user.as_ref()))
            .cloned()
            .collect()
    });
    // A picker answers with the row instead of offering things to do to it,
    // so the actions column is not drawn at all - see `GridConfig::choosing`.
    let choosing = config.with_value(|config| config.choosing);
    let has_actions = !actions.is_empty() && choosing.is_none();
    let actions = StoredValue::new(actions);

    let visible: Vec<Column<T>> = config.with_value(|config| {
        config
            .columns
            .iter()
            .filter(|column| !hidden.contains(column.field))
            .cloned()
            .collect()
    });
    // What a phone cannot see. Below `sm` the non-essential columns are
    // switched off by CSS, so each row offers to unfold them instead - see
    // `grid_row_view`. If every visible column is essential there is nothing
    // to unfold and the control is not drawn at all.
    let has_detail = visible.iter().any(|column| !column.essential);

    // How many cells wide the row is *on a phone*, which is what the detail
    // row has to span: the unfold control, the essential columns, and the
    // actions.
    let phone_span = usize::from(has_detail)
        + visible.iter().filter(|column| column.essential).count()
        + usize::from(has_actions);

    let (grid_id, min_width) = config.with_value(|config| (config.id, config.min_width));
    let headers = visible.clone();
    let visible = StoredValue::new(visible);

    view! {
        // Scrolls inside its own box rather than pushing the page sideways.
        //
        // The configured `min-w-*` is prefixed `sm:` so it never applies on a
        // phone. It has to be: a document containing anything wider than the
        // screen makes a mobile browser widen the *layout viewport* to fit it,
        // and `position: fixed` is measured against that - so one 48rem table
        // inside a scroller is enough to push a centred modal off the bottom
        // of the screen. Below `sm` the table is only as wide as its essential
        // columns, which is why they exist.
        <div class="overflow-x-auto">
            <table id=format!("{grid_id}-table") class=format!("w-full {min_width} text-sm")>
                <thead class="border-b border-edge text-left text-xs uppercase tracking-wide text-content-subtle">
                    <tr>
                        {has_detail
                            .then(|| {
                                view! {
                                    <th scope="col" class="w-7 px-0.5 py-1.5 sm:hidden">
                                        <span class="sr-only">{l!("grid.details")}</span>
                                    </th>
                                }
                            })}

                        {headers
                            .into_iter()
                            .map(|column| view! { <HeaderCell state=state column=column /> })
                            .collect::<Vec<_>>()}

                        {has_actions
                            .then(|| {
                                view! {
                                    <th scope="col" class="px-3 py-1.5 text-right font-medium">
                                        <span class="sr-only">{l!("grid.row_actions")}</span>
                                    </th>
                                }
                            })}
                    </tr>
                </thead>

                <tbody class="divide-y divide-edge">
                    {page
                        .rows
                        .into_iter()
                        .map(|row| {
                            view! {
                                <GridRowView
                                    row=row
                                    columns=visible
                                    actions=actions
                                    handle=handle
                                    has_actions=has_actions
                                    has_detail=has_detail
                                    phone_span=phone_span
                                    choosing=choosing
                                />
                            }
                        })
                        .collect::<Vec<_>>()}
                </tbody>
            </table>
        </div>

        <GridPager state=state footing=Signal::derive(move || footing) choices=per_page_choices />
    }
    .into_any()
}

#[component]
fn header_cell<T: GridRow>(state: GridState, column: Column<T>) -> impl IntoView {
    let field = column.field;
    let header = column.header.clone();
    let class = format!(
        "{CELL} font-medium {} {} {}",
        column.align.cell_class(),
        column.class,
        column.responsive_class(),
    );

    if !column.sortable {
        return view! { <th scope="col" class=class>{header}</th> }.into_any();
    }

    let direction = move || state.sort_of(field);

    view! {
        <th
            scope="col"
            class=class
            aria-sort=move || {
                match direction() {
                    Some(SortDirection::Ascending) => "ascending",
                    Some(SortDirection::Descending) => "descending",
                    None => "none",
                }
            }
        >
            <button
                type="button"
                class="inline-flex items-center gap-1 uppercase tracking-wide hover:text-content"
                on:click=move |_| state.toggle_sort(field)
            >
                {header}
                // The unsorted arrows are dimmed rather than absent: a heading
                // that only grows an icon on hover hides which columns can be
                // sorted at all until every one of them has been hovered.
                <span class=move || {
                    if direction().is_some() { "shrink-0" } else { "shrink-0 opacity-40" }
                }>
                    {move || {
                        let icon = match direction() {
                            Some(SortDirection::Ascending) => Icon::ChevronUp,
                            Some(SortDirection::Descending) => Icon::ChevronDown,
                            None => Icon::ChevronsUpDown,
                        };

                        view! { <Icon icon=icon size=IconSize::Xs /> }
                    }}
                </span>
            </button>
        </th>
    }
    .into_any()
}

#[component]
fn grid_row_view<T: GridRow>(
    row: T,
    columns: StoredValue<Vec<Column<T>>>,
    actions: StoredValue<Vec<RowAction<T>>>,
    handle: GridHandle,
    has_actions: bool,
    /// Whether any visible column is hidden on a phone, and so whether this row
    /// has anything to unfold.
    has_detail: bool,
    /// How many cells the row occupies on a phone, for the detail row's
    /// `colspan`.
    phone_span: usize,
    /// Set when this grid is a picker: the row answers with itself instead of
    /// going anywhere. See [`GridConfig::choosing`].
    choosing: Option<Callback<T>>,
) -> impl IntoView {
    let open = RwSignal::new(false);

    let cells = columns.with_value(|columns| {
        columns
            .iter()
            .map(|column| {
                let class = format!(
                    "{CELL} {} {} {}",
                    column.align.cell_class(),
                    column.class,
                    column.responsive_class(),
                );

                view! { <td class=class>{column.view(&row)}</td> }
            })
            .collect::<Vec<_>>()
    });

    // The columns a phone is not showing, drawn as label-and-value pairs. Built
    // whether or not the row is open: the alternative is a node that appears on
    // click, and a table whose shape differs between the server and the browser
    // is a table that cannot be hydrated.
    let detail = has_detail.then(|| {
        columns.with_value(|columns| {
            columns
                .iter()
                .filter(|column| !column.essential)
                .map(|column| {
                    view! {
                        <div class="flex items-baseline justify-between gap-3 py-1">
                            <dt class="shrink-0 text-xs uppercase tracking-wide text-content-subtle">
                                {column.header.clone()}
                            </dt>
                            <dd class="min-w-0 text-right">{column.view(&row)}</dd>
                        </div>
                    }
                })
                .collect::<Vec<_>>()
        })
    });

    // Everything this row offers, behind one trigger. A row whose actions were
    // all filtered out - a built-in role with nothing but Delete gated away -
    // draws no trigger at all, because a menu that opens onto nothing is worse
    // than an absent one. The cell itself stays, so the columns still line up.
    let offered = actions.with_value(|actions| {
        actions
            .iter()
            .filter(|action| action.applies_to(&row))
            .cloned()
            .collect::<Vec<_>>()
    });

    // Where a click on the row goes, if an action asked for it.
    //
    // Read out of `offered` rather than out of the configuration, which is the
    // point: that list has already been filtered by permission and by `when`,
    // so a viewer who cannot see Open cannot reach it by clicking either, and
    // a row the action does not apply to is simply not clickable. One
    // destination, declared once, gated once.
    let opens = offered.iter().find_map(|action| match &action.kind {
        ActionKind::Link(href) if action.opens_on_row_click() => Some(href(&row)),
        _ => None,
    });

    let row_menu = (has_actions && !offered.is_empty())
        .then(|| view! { <RowMenu actions=offered row=row.clone() handle=handle /> });

    // Static per row: whether an action claimed the click cannot change while
    // the row is on screen, because the filtering that decided it is the
    // filtering that built the row.
    let row_class = if opens.is_some() || choosing.is_some() {
        "cursor-pointer hover:bg-surface-hover"
    } else {
        "hover:bg-surface-hover"
    };

    let navigate = use_navigate();
    let chosen = row.clone();
    let follow = move |event: leptos::ev::MouseEvent| {
        if !is_a_plain_click(&event) {
            return;
        }

        if let Some(choose) = choosing {
            // `try_run`, because this row's owner may already be disposed
            // while the transition holds its markup on screen - see the note
            // on zombie rows in `menu`.
            let _ = choose.try_run(chosen.clone());
            return;
        }

        if let Some(href) = opens.clone() {
            navigate(&href, NavigateOptions::default());
        }
    };

    view! {
        <tr class=row_class on:click=follow>
            {has_detail
                .then(|| {
                    view! {
                        <td class="px-0.5 py-1.5 align-top sm:hidden">
                            <button
                                type="button"
                                class="grid size-6 place-items-center rounded-control text-content-subtle hover:bg-surface-hover hover:text-content"
                                // `try_update`, because a row's markup outlives
                                // its owner for as long as the transition keeps
                                // the previous rows on the screen. See the note
                                // on zombie rows in `menu`.
                                on:click=move |_| {
                                    open.try_update(|open| *open = !*open);
                                }
                                aria-expanded=move || if open.get() { "true" } else { "false" }
                                aria-label=move || {
                                    if open.get() {
                                        l!("grid.hide_details")
                                    } else {
                                        l!("grid.show_details")
                                    }
                                }
                            >
                                {move || {
                                    let icon = if open.get() {
                                        Icon::ChevronDown
                                    } else {
                                        Icon::ChevronRight
                                    };
                                    view! { <Icon icon=icon size=IconSize::Xs /> }
                                }}
                            </button>
                        </td>
                    }
                })}

            {cells}
            {has_actions
                .then(|| {
                    view! {
                        <td class=CELL>
                            <div class="flex items-center justify-end">{row_menu}</div>
                        </td>
                    }
                })}
        </tr>

        {detail
            .map(|detail| {
                view! {
                    // Always in the DOM, shown by class. `sm:hidden` as well as
                    // the toggle, because above `sm` the columns themselves are
                    // back and repeating them underneath would be nonsense.
                    <tr
                        class="sm:hidden"
                        class:hidden=move || !open.get()
                        // A stable hook so anything walking the table - a test,
                        // a future selector - can tell the unfolded detail
                        // apart from a row of data.
                        data-row-detail="true"
                    >
                        <td class=format!("bg-surface-sunken {CELL}") colspan=phone_span>
                            <dl class="divide-y divide-edge text-sm">{detail}</dl>
                        </td>
                    </tr>
                }
            })}
    }
}

/// Whether a click on a row was somebody asking to open it.
///
/// Three things wear the same event and mean something else entirely, and all
/// three are ordinary use of a table rather than edge cases:
///
/// * a **modified click** - ctrl, meta, shift or alt - means "new tab" or
///   "extend the selection". A router navigation can honour neither, and
///   swallowing it would take away the one thing a link is for. The menu's
///   entry is a real `<a>` and answers all of them.
/// * a click **on something that is already interactive**: the row menu's
///   trigger, a link inside a cell, a checkbox. `closest` walks up from
///   whatever was actually hit, so the icon inside a button counts as the
///   button.
/// * a click that **ends a drag over text**. Somebody copying an identifier out
///   of a cell releases the mouse over the row, and the browser calls that a
///   click on the row.
///
/// Browser only, like `place` above: the interfaces are asked for in the
/// hydrate build alone, and the server never receives a click.
#[cfg(feature = "hydrate")]
fn is_a_plain_click(event: &leptos::ev::MouseEvent) -> bool {
    use leptos::wasm_bindgen::JsCast;

    if event.ctrl_key() || event.meta_key() || event.shift_key() || event.alt_key() {
        return false;
    }

    let interactive = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|element| {
            element
                .closest("a, button, input, select, textarea, label, [role='menu']")
                .ok()
                .flatten()
        });
    if interactive.is_some() {
        return false;
    }

    // A collapsed selection is a caret with nothing in it, which is what an
    // ordinary click leaves behind.
    let selecting = window()
        .get_selection()
        .ok()
        .flatten()
        .is_some_and(|selection| !selection.is_collapsed());

    !selecting
}

#[cfg(not(feature = "hydrate"))]
const fn is_a_plain_click(_event: &leptos::ev::MouseEvent) -> bool {
    false
}

/// Rows are on their way.
///
/// Shaped like the table rather than being the word "Loading", so the page does
/// not jump when the rows arrive - the height is already about right.
#[component]
fn grid_skeleton() -> impl IntoView {
    view! {
        <div class="divide-y divide-edge" aria-hidden="true">
            {(0..5)
                .map(|_| {
                    view! {
                        <div class="flex items-center gap-3 px-3 py-2">
                            <div class="size-7 shrink-0 animate-pulse rounded-full bg-surface-sunken" />
                            <div class="h-3 w-40 animate-pulse rounded bg-surface-sunken" />
                            <div class="ms-auto h-3 w-24 animate-pulse rounded bg-surface-sunken" />
                        </div>
                    }
                })
                .collect::<Vec<_>>()}
        </div>
    }
}

/// Build the CSV and hand it to the browser.
///
/// An in-memory grid exports everything the search matched, on every page. A
/// paged grid holds only the page on screen, so that is all it can honestly
/// write - and the file name says which page it was rather than pretending to
/// be the whole list.
fn export_now<T: GridRow>(
    stem: &'static str,
    config: StoredValue<GridConfig<T>>,
    state: GridState,
    loaded: StoredValue<Option<Loaded<T>>>,
) {
    let request = state.request();

    let Some((rows, name)) = loaded.with_value(|held| match held {
        None => None,
        Some(Loaded::All(all)) => Some((
            config.with_value(|config| {
                local::matched(
                    &request,
                    &config.columns,
                    &config.filters,
                    &config.date_filters,
                    all,
                )
            }),
            export::file_name(stem),
        )),
        Some(Loaded::Page(page)) => Some((
            page.rows.clone(),
            export::file_name(&format!("{stem}-page-{}", page.page)),
        )),
    }) else {
        return;
    };

    let csv = config.with_value(|config| {
        let visible: Vec<&Column<T>> = config
            .columns
            .iter()
            .filter(|column| !state.is_hidden(column.field))
            .collect();

        export::to_csv(&visible, &rows)
    });

    export::download(&name, &csv);
}
