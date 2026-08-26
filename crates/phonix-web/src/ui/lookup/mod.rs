//! Lookups: a field whose choices are another entity.
//!
//! A `<select>` is the right control for a closed set somebody wrote down
//! once: a status, a tone of voice. It is the wrong one the moment the options
//! are *records*. It cannot be searched, it cannot show two columns, it cannot
//! say that the value you are looking for does not exist yet, and on a list of
//! six hundred currencies it is unusable.
//!
//! # Two presentations, one component
//!
//! * [`Choices::List`] - a filtering list. For a lookup you can identify from
//!   its name: a unit of measure, a category, a currency.
//! * [`Choices::Table`] - a grid in the panel. For a lookup you cannot: a
//!   supplier you know by code and region, an item you pick by stock level.
//!
//! The table presentation is a real [`DataGrid`](crate::ui::table::DataGrid)
//! over the entity's own `GridConfig` in
//! [`choosing`](crate::ui::table::GridConfig::choosing) mode, not a second
//! table that happens to look like one. That is the whole reason it is worth
//! building: search, paging, sorting, column choice and the one description of
//! what the entity *is* all arrive with it, and the picker cannot drift from
//! the list screen because it is the list screen.
//!
//! # Quick add, and the seam that keeps it out of the kit
//!
//! Discovering a missing value halfway through a form must not cost the form.
//! Navigating away to add a currency and coming back means retyping
//! everything, so [`QuickAdd::Form`] puts a small form in a dialog over the
//! field: it creates the record, selects it, and closes. [`QuickAdd::Page`] is
//! the fallback for an entity too big for that - it is a link, and it is
//! honest about leaving.
//!
//! Both of those, and the table picker, are the same shape:
//!
//! ```ignore
//! type Picker = Arc<dyn Fn(Callback<Choice>) -> AnyView + Send + Sync>;
//! ```
//!
//! *Something* that renders and eventually calls back with a choice. This is
//! deliberate type erasure, and it is what stops the lookup becoming generic
//! over the entity being looked up. A `Lookup<T>` would have to be generic over
//! the picked entity, and a quick-add would make it generic over that entity's
//! *draft* type as well - which is two type parameters on a field, spreading to
//! every form that holds one. The entity supplies a closure; the lookup wires
//! the answer; neither knows the other's types.
//!
//! # One or many
//!
//! The value is a `Vec<Choice>` in both cases, with [`multiple`] deciding
//! whether choosing replaces or toggles. A separate multi-select component
//! would be this file again with two lines changed - the filtering, the
//! keyboard, the panel placement, the quick add and the two presentations are
//! all the same - and the two would drift.
//!
//! [`multiple`]: LookupField
//!
//! # Everything is closed by anything that would move it
//!
//! The panel is `position: fixed`, for the reason set out in [`place`]. Fixed
//! means it does not travel with the page, so a scroll, a wheel, a resize or a
//! pointer anywhere else closes it rather than leaving it stranded beside the
//! field it used to belong to.

mod panel;
mod place;
mod select;

use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::components::A;

use self::panel::dismiss_when_moved;
use self::place::At;
pub use self::select::SelectField;
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::ui::form::field::Choice;

/// Something that renders into a panel and answers with a choice.
///
/// See the module documentation for why this is erased rather than generic.
pub type Picker = Arc<dyn Fn(Callback<Choice>) -> AnyView + Send + Sync>;

/// Where a lookup's options come from, and how they are shown.
pub enum Choices {
    /// A flat list, filtered here in the browser.
    ///
    /// For a set small enough to send with the page. A lookup whose entity has
    /// thousands of rows wants [`Choices::Table`], which pages.
    List(Vec<Choice>),
    /// A grid in the panel.
    ///
    /// `width` is what the panel asks for in pixels; it still gets cut down to
    /// the window on a phone, and it is never narrower than the field.
    Table { width: f64, view: Picker },
}

// Hand-written, and the reason is worth stating: [`FieldKind`] derives `Debug`,
// and a kind that holds one of these has to be able to answer. The erased
// closure is the one part that cannot describe itself, so it says so rather
// than the whole type going undebuggable.
//
// `PartialEq` is *not* here, and deliberately. Two pickers are two closures,
// and the only equality a closure can offer is pointer identity - which would
// call two identically-built configurations different, every render. There is
// no honest answer, so there is no answer; `FieldKind` dropped its derive.
//
// [`FieldKind`]: crate::ui::form::FieldKind
impl std::fmt::Debug for Choices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::List(choices) => f.debug_tuple("List").field(&choices.len()).finish(),
            Self::Table { width, .. } => f
                .debug_struct("Table")
                .field("width", width)
                .finish_non_exhaustive(),
        }
    }
}

impl Clone for Choices {
    fn clone(&self) -> Self {
        match self {
            Self::List(choices) => Self::List(choices.clone()),
            Self::Table { width, view } => Self::Table {
                width: *width,
                view: Arc::clone(view),
            },
        }
    }
}

impl Choices {
    /// A grid picker, from a closure that builds one.
    ///
    /// The closure is handed the callback to answer with; what it does with it
    /// is the entity's business - in practice `DataGrid` over that entity's
    /// `GridConfig` in `choosing` mode.
    pub fn table(view: impl Fn(Callback<Choice>) -> AnyView + Send + Sync + 'static) -> Self {
        Self::Table {
            width: 640.0,
            view: Arc::new(view),
        }
    }

    /// Ask for a different panel width.
    #[must_use]
    pub const fn wide(mut self, pixels: f64) -> Self {
        if let Self::Table { width, .. } = &mut self {
            *width = pixels;
        }
        self
    }
}

/// What the panel offers when the value somebody wants is not in the list.
pub enum QuickAdd {
    /// A form in a dialog over the field. Creates, selects, closes.
    Form {
        label: String,
        title: String,
        view: Picker,
    },
    /// A link to the entity's own page, for one too big to fit in a dialog.
    ///
    /// This leaves the form, and there is no getting around that - which is
    /// exactly why it is the fallback and not the default.
    Page { label: String, href: String },
}

impl std::fmt::Debug for QuickAdd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Form { label, title, .. } => f
                .debug_struct("Form")
                .field("label", label)
                .field("title", title)
                .finish_non_exhaustive(),
            Self::Page { label, href } => f
                .debug_struct("Page")
                .field("label", label)
                .field("href", href)
                .finish(),
        }
    }
}

impl Clone for QuickAdd {
    fn clone(&self) -> Self {
        match self {
            Self::Form { label, title, view } => Self::Form {
                label: label.clone(),
                title: title.clone(),
                view: Arc::clone(view),
            },
            Self::Page { label, href } => Self::Page {
                label: label.clone(),
                href: href.clone(),
            },
        }
    }
}

impl QuickAdd {
    /// A small form, in a dialog. The closure is handed the callback to answer
    /// with the record it created.
    pub fn form(
        label: impl Into<String>,
        title: impl Into<String>,
        view: impl Fn(Callback<Choice>) -> AnyView + Send + Sync + 'static,
    ) -> Self {
        Self::Form {
            label: label.into(),
            title: title.into(),
            view: Arc::new(view),
        }
    }

    /// A link to the full form for this entity.
    pub fn page(label: impl Into<String>, href: impl Into<String>) -> Self {
        Self::Page {
            label: label.into(),
            href: href.into(),
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::Form { label, .. } | Self::Page { label, .. } => label,
        }
    }
}

/// A field whose choices are records.
///
/// See the [module documentation](self) for the two presentations, the quick
/// add, and why the value is a `Vec` even when only one may be chosen.
#[component]
pub fn lookup_field(
    /// What is chosen. Empty is nothing chosen; with [`multiple`] unset it
    /// holds at most one.
    ///
    /// A `Choice` rather than an id, because the field has to draw a label for
    /// what is selected and a table picker has no id-to-label map to consult -
    /// it has a row. The caller keeps whichever half it needs.
    ///
    /// [`multiple`]: LookupField
    selected: RwSignal<Vec<Choice>>,
    choices: Choices,
    /// Let more than one be chosen. Choosing then toggles rather than
    /// replaces, and the panel stays open.
    #[prop(optional)]
    multiple: bool,
    /// `Option`, and it stays one: the form kit reads this off a
    /// [`FieldKind::Lookup`] where it is already optional, and a prop leptos
    /// had stripped to `QuickAdd` could not be handed that.
    ///
    /// [`FieldKind::Lookup`]: crate::ui::form::FieldKind::Lookup
    #[prop(optional_no_strip)]
    quick_add: Option<QuickAdd>,
    #[prop(optional_no_strip)] placeholder: Option<String>,
    #[prop(optional, into)] disabled: Signal<bool>,
    /// Ties the field to a `<label for>`. The list presentation puts it on the
    /// text box; the table presentation puts it on the button.
    #[prop(optional, into)]
    id: Option<String>,
    #[prop(optional, into)] invalid: Signal<bool>,
    /// Marks the control required for a screen reader. The asterisk beside the
    /// label is drawn by whoever wrote the label, and is not this component's.
    #[prop(optional)]
    required: bool,
    /// The ids of the help line and the error message, when the form has them.
    ///
    /// Carried through rather than assembled here: the form kit is what knows
    /// which of the two exist, and an `aria-describedby` naming an id that is
    /// not on the page is worse than none - a screen reader announces the gap.
    #[prop(optional, into)]
    described_by: Signal<Option<String>>,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let query = RwSignal::new(String::new());
    let active = RwSignal::new(0_usize);
    let at = RwSignal::new(At::default());
    let adding = RwSignal::new(false);

    // Set once, after this subtree has hydrated. Everything expensive hangs
    // off it: a grid built into the panel on the server would fetch a page of
    // rows for a lookup nobody opened, and a subtree that first appears on a
    // click is a subtree leptos would otherwise try to *hydrate* against the
    // comment the server left behind. Rendering it after hydration has
    // finished is neither.
    let ready = RwSignal::new(false);
    Effect::new(move |_| ready.set(true));

    // Latches the first time the panel opens, and never goes back. A picker
    // built from `open` alone would be thrown away and rebuilt on every close,
    // which means a fresh fetch every time somebody glances at the list.
    let opened = RwSignal::new(false);

    let anchor = NodeRef::<leptos::html::Div>::new();
    let panel = NodeRef::<leptos::html::Div>::new();

    let is_table = matches!(choices, Choices::Table { .. });
    let wanted = match &choices {
        Choices::Table { width, .. } => *width,
        Choices::List(_) => 0.0,
    };
    let choices = StoredValue::new(choices);
    let quick_add = StoredValue::new(quick_add);

    // --- choosing ---------------------------------------------------------

    let choose = Callback::new(move |choice: Choice| {
        selected.try_update(|selected| {
            if multiple {
                match selected.iter().position(|held| held.value == choice.value) {
                    // Toggling off, so the same row that added it takes it
                    // away - which is what a person who clicked one by mistake
                    // reaches for first.
                    Some(index) => {
                        selected.remove(index);
                    }
                    None => selected.push(choice.clone()),
                }
            } else {
                *selected = vec![choice.clone()];
            }
        });

        let _ = query.try_set(String::new());
        let _ = active.try_set(0);
        if !multiple {
            let _ = open.try_set(false);
        }
    });

    // The quick add answers the same way a picked row does, and then puts the
    // panel away: somebody who has just created the value they were looking
    // for is finished with the list.
    let created = Callback::new(move |choice: Choice| {
        choose.run(choice);
        let _ = adding.try_set(false);
        let _ = open.try_set(false);
    });

    // --- the filtered list -------------------------------------------------

    let matching = Signal::derive(move || {
        let needle = query.get().trim().to_lowercase();

        choices.with_value(|choices| {
            let Choices::List(list) = choices else {
                return Vec::new();
            };

            list.iter()
                .filter(|choice| {
                    needle.is_empty()
                        || choice.label.to_lowercase().contains(&needle)
                        // The detail line is searched too. It is where a code
                        // or an abbreviation lives - "USD" under "US Dollar" -
                        // and a box that ignores what is on the screen in
                        // front of somebody is worse than no box.
                        || choice
                            .detail
                            .as_deref()
                            .is_some_and(|detail| detail.to_lowercase().contains(&needle))
                })
                .cloned()
                .collect::<Vec<_>>()
        })
    });

    // --- opening and closing ----------------------------------------------

    let show = move || {
        if disabled.get_untracked() {
            return;
        }
        // Measured on the way open and never again: the field's rectangle only
        // matters at the instant the panel is put on the screen, and anything
        // that would move it afterwards closes it instead.
        let _ = at.try_set(place::of(anchor, wanted));
        let _ = open.try_set(true);
        let _ = opened.try_set(true);
        let _ = active.try_set(0);
    };

    let toggle = move || {
        if open.get_untracked() {
            let _ = open.try_set(false);
        } else {
            show();
        }
    };

    // A fixed panel does not travel with the page, so anything that moves the
    // field under it puts it away rather than leaving it stranded. This was a
    // second copy of the four listeners until the select was built; it is the
    // shared one now, which is what stopped a wheel inside the panel closing a
    // lookup that was only being scrolled.
    dismiss_when_moved(open, panel, anchor);

    // --- the keyboard ------------------------------------------------------

    let on_key = move |event: leptos::ev::KeyboardEvent| {
        match event.key().as_str() {
            "ArrowDown" => {
                event.prevent_default();
                if open.get_untracked() {
                    let last = matching.get_untracked().len().saturating_sub(1);
                    let _ = active.try_update(|index| *index = (*index + 1).min(last));
                } else {
                    show();
                }
            }
            "ArrowUp" => {
                event.prevent_default();
                let _ = active.try_update(|index| *index = index.saturating_sub(1));
            }
            "Enter" => {
                // Only when the panel is up. Otherwise this is somebody
                // submitting the form the field is in, and swallowing that
                // would be the field taking over a key it does not own.
                if open.get_untracked()
                    && let Some(choice) = matching.get_untracked().get(active.get_untracked())
                {
                    event.prevent_default();
                    choose.run(choice.clone());
                }
            }
            "Escape" => {
                let _ = open.try_set(false);
            }
            // Backspace on an empty box takes the last chip off, which is what
            // every other control shaped like this does.
            "Backspace" if multiple && query.get_untracked().is_empty() => {
                let _ = selected.try_update(|selected| {
                    selected.pop();
                });
            }
            _ => {}
        }
    };

    // --- the field ---------------------------------------------------------

    // Only what carries state. The resting border and fill come from
    // `.lookup-shell` in the stylesheet, because the `--control-*` tokens are
    // plain custom properties and there is no utility that names them - a
    // class like `bg-control-surface` looks right and compiles to nothing.
    let shell_class = move || {
        let edge = if invalid.get() {
            "border-danger"
        } else if open.get() {
            "border-brand"
        } else {
            ""
        };

        format!("lookup-shell {edge}")
    };

    let chips = move || {
        selected
            .get()
            .into_iter()
            .map(|choice| {
                let value = choice.value.clone();
                view! {
                    <span class="inline-flex max-w-full items-center gap-1 rounded-control bg-surface-sunken px-1.5 py-0.5 text-xs text-content">
                        <span class="truncate">{choice.label.clone()}</span>
                        <button
                            type="button"
                            class="shrink-0 text-content-subtle hover:text-danger"
                            disabled=move || disabled.get()
                            aria-label=l!("lookup.remove", name = choice.label.clone())
                            on:click=move |event| {
                                event.stop_propagation();
                                let value = value.clone();
                                let _ = selected
                                    .try_update(|selected| {
                                        selected.retain(|held| held.value != value);
                                    });
                            }
                        >
                            <Icon icon=Icon::X size=IconSize::Xs />
                        </button>
                    </span>
                }
            })
            .collect::<Vec<_>>()
    };

    let placeholder_text = placeholder.unwrap_or_else(|| l!("lookup.search"));
    let field_id = id.clone();

    view! {
        <div node_ref=anchor class="relative">
            {if is_table {
                // No text box: the grid inside the panel has a search of its
                // own, and two boxes searching the same list is one of them
                // doing nothing.
                let field_id = field_id.clone();
                view! {
                    <button
                        type="button"
                        id=field_id
                        class=shell_class
                        disabled=move || disabled.get()
                        aria-haspopup="dialog"
                        aria-expanded=move || if open.get() { "true" } else { "false" }
                        aria-disabled=move || disabled.get().then_some("true")
                        aria-invalid=move || invalid.get().then_some("true")
                        aria-required=required.then_some("true")
                        aria-describedby=move || described_by.get()
                        on:click=move |_| toggle()
                    >
                        <span class="flex min-w-0 flex-1 flex-wrap items-center gap-1">
                            {move || {
                                if selected.get().is_empty() {
                                    view! {
                                        <span class="truncate text-content-subtle">
                                            {l!("lookup.nothing_chosen")}
                                        </span>
                                    }
                                        .into_any()
                                } else {
                                    chips().into_any()
                                }
                            }}
                        </span>
                        <Icon icon=Icon::ChevronDown size=IconSize::Xs />
                    </button>
                }
                    .into_any()
            } else {
                view! {
                    // A composite that wears the control's clothes rather than
                    // an `<input>` that is one: the chips live inside it, and
                    // the box that is typed into has its own border taken away
                    // in `style/main.css`.
                    <div
                        class=shell_class
                        aria-disabled=move || disabled.get().then_some("true")
                        on:click=move |_| {
                            if !open.get_untracked() {
                                show();
                            }
                        }
                    >
                        <span class="flex min-w-0 flex-1 flex-wrap items-center gap-1">
                            {move || multiple.then(chips)}
                            <input
                                type="text"
                                id=field_id
                                role="combobox"
                                autocomplete="off"
                                class="min-w-16 flex-1"
                                disabled=move || disabled.get()
                                placeholder=placeholder_text
                                aria-expanded=move || if open.get() { "true" } else { "false" }
                                aria-autocomplete="list"
                                aria-invalid=move || invalid.get().then_some("true")
                                aria-required=required.then_some("true")
                                aria-describedby=move || described_by.get()
                                prop:value=move || {
                                    // Open, it holds what is being typed.
                                    // Closed, it holds what was chosen - so
                                    // the field reads as the answer rather
                                    // than as the question that found it.
                                    if open.get() || multiple {
                                        query.get()
                                    } else {
                                        selected
                                            .get()
                                            .first()
                                            .map(|choice| choice.label.clone())
                                            .unwrap_or_default()
                                    }
                                }
                                on:input=move |event| {
                                    let _ = query.try_set(event_target_value(&event));
                                    let _ = active.try_set(0);
                                    if !open.get_untracked() {
                                        show();
                                    }
                                }
                                on:keydown=on_key
                            />
                        </span>

                        {move || {
                            (!selected.get().is_empty() && !multiple && !disabled.get())
                                .then(|| {
                                    view! {
                                        <button
                                            type="button"
                                            class="shrink-0 text-content-subtle hover:text-content"
                                            aria-label=l!("lookup.clear")
                                            title=l!("lookup.clear")
                                            on:click=move |event| {
                                                event.stop_propagation();
                                                let _ = selected.try_set(Vec::new());
                                                let _ = query.try_set(String::new());
                                            }
                                        >
                                            <Icon icon=Icon::X size=IconSize::Xs />
                                        </button>
                                    }
                                })
                        }}
                        <Icon icon=Icon::ChevronDown size=IconSize::Xs />
                    </div>
                }
                    .into_any()
            }}

            // Always in the DOM for the list presentation, shown by a class -
            // the same arrangement, and the same reason, as the grid's row
            // menu. What is *inside* it for a table is deferred; see `ready`.
            <div
                node_ref=panel
                class="alert-enter z-[55] flex flex-col overflow-hidden rounded-card border border-edge bg-surface-raised shadow-pop"
                class:hidden=move || !open.get()
                aria-hidden=move || if open.get() { "false" } else { "true" }
                style=move || at.get().style()
            >
                <div class="min-h-0 flex-1 overflow-auto overscroll-contain">
                    {move || {
                        choices
                            .with_value(|choices| match choices {
                                Choices::List(_) => {
                                    view! {
                                        <ListBody
                                            matching=matching
                                            active=active
                                            choose=choose
                                            selected=selected
                                            multiple=multiple
                                        />
                                    }
                                        .into_any()
                                }
                                Choices::Table { view, .. } => {
                                    let view = Arc::clone(view);
                                    // Built the first time the panel is opened
                                    // and not before: a grid rendered eagerly
                                    // fetches a page of rows for a lookup
                                    // nobody has touched.
                                    view! {
                                        {move || {
                                            (ready.get() && opened.get()).then(|| view(choose))
                                        }}
                                    }
                                        .into_any()
                                }
                            })
                    }}
                </div>

                {move || {
                    quick_add
                        .with_value(|quick_add| {
                            quick_add
                                .as_ref()
                                .map(|add| {
                                    let label = add.label().to_owned();
                                    match add {
                                        QuickAdd::Page { href, .. } => {
                                            view! {
                                                <A
                                                    href=href.clone()
                                                    attr:class="flex shrink-0 items-center gap-2 border-t border-edge px-3 py-2 text-sm text-brand hover:bg-surface-hover"
                                                >
                                                    <Icon icon=Icon::Plus size=IconSize::Xs />
                                                    {label}
                                                </A>
                                            }
                                                .into_any()
                                        }
                                        QuickAdd::Form { .. } => {
                                            view! {
                                                <button
                                                    type="button"
                                                    class="flex shrink-0 items-center gap-2 border-t border-edge px-3 py-2 text-left text-sm text-brand hover:bg-surface-hover"
                                                    on:click=move |_| {
                                                        let _ = adding.try_set(true);
                                                        let _ = open.try_set(false);
                                                    }
                                                >
                                                    <Icon icon=Icon::Plus size=IconSize::Xs />
                                                    {label}
                                                </button>
                                            }
                                                .into_any()
                                        }
                                    }
                                })
                        })
                }}
            </div>

            // The quick-add dialog. Outside the panel, because the panel is
            // what it replaces, and deferred for the same reason the picker is.
            {move || {
                (ready.get() && adding.get())
                    .then(|| {
                        quick_add
                            .with_value(|quick_add| match quick_add {
                                Some(QuickAdd::Form { title, view, .. }) => {
                                    let view = Arc::clone(view);
                                    view! {
                                        <QuickAddDialog
                                            title=title.clone()
                                            close=Callback::new(move |()| {
                                                let _ = adding.try_set(false);
                                            })
                                        >
                                            {view(created)}
                                        </QuickAddDialog>
                                    }
                                        .into_any()
                                }
                                _ => ().into_any(),
                            })
                    })
            }}
        </div>
    }
}

/// The filtering list, and the row that says nothing matched.
#[component]
fn list_body(
    matching: Signal<Vec<Choice>>,
    active: RwSignal<usize>,
    choose: Callback<Choice>,
    selected: RwSignal<Vec<Choice>>,
    /// Whether an entry can be on as well as chosen. With one choice the panel
    /// closes the moment something is picked, so a tick would be drawn for the
    /// instant before it disappeared.
    multiple: bool,
) -> impl IntoView {
    view! {
        <div role="listbox" class="py-1">
            {move || {
                let rows = matching.get();
                if rows.is_empty() {
                    return view! {
                        <p class="px-3 py-4 text-center text-sm text-content-subtle">
                            {l!("lookup.no_matches")}
                        </p>
                    }
                        .into_any();
                }

                rows.into_iter()
                    .enumerate()
                    .map(|(index, choice)| {
                        let picked = choice.clone();
                        let value = choice.value.clone();
                        let ticked = move || {
                            multiple
                                && selected
                                    .get()
                                    .iter()
                                    .any(|held| held.value == value)
                        };
                        view! {
                            <button
                                type="button"
                                role="option"
                                class=move || {
                                    let state = if active.get() == index {
                                        "bg-surface-hover"
                                    } else {
                                        ""
                                    };
                                    format!(
                                        "flex w-full items-baseline justify-between gap-3 px-3 py-1.5 text-left text-sm hover:bg-surface-hover {state}",
                                    )
                                }
                                // Pointer, not click: the panel is dismissed by
                                // a pointerdown anywhere outside it, and that
                                // listener would otherwise race this one.
                                on:pointerdown=move |event| {
                                    event.prevent_default();
                                    choose.run(picked.clone());
                                }
                                on:pointerenter=move |_| {
                                    let _ = active.try_set(index);
                                }
                            >
                                <span class="flex min-w-0 items-center gap-1.5">
                                    {move || {
                                        ticked()
                                            .then(|| {
                                                view! {
                                                    <span class="shrink-0 text-brand">
                                                        <Icon icon=Icon::Check size=IconSize::Xs />
                                                    </span>
                                                }
                                            })
                                    }}
                                    <span class="truncate text-content">{choice.label}</span>
                                </span>
                                {choice
                                    .detail
                                    .map(|detail| {
                                        view! {
                                            <span class="shrink-0 text-xs text-content-subtle">
                                                {detail}
                                            </span>
                                        }
                                    })}
                            </button>
                        }
                    })
                    .collect::<Vec<_>>()
                    .into_any()
            }}
        </div>
    }
}

/// The dialog a [`QuickAdd::Form`] opens into.
///
/// Deliberately plain: it is a frame around whatever the entity supplied, and
/// the buttons inside belong to that form rather than to this. A dialog that
/// drew its own Save would be a dialog that had to know what saving meant.
#[component]
fn quick_add_dialog(
    title: String,
    close: Callback<()>,
    children: Children,
) -> impl IntoView {
    view! {
        <div
            class="fixed inset-0 z-[70] grid place-items-center bg-overlay p-4"
            on:click=move |_| close.run(())
        >
            <div
                class="alert-enter w-[min(32rem,100%)] overflow-hidden rounded-card border border-edge bg-surface-raised shadow-pop"
                role="dialog"
                aria-modal="true"
                // The backdrop closes; the sheet must not, or every click
                // inside the form dismisses the form.
                on:click=move |event| event.stop_propagation()
            >
                <header class="flex items-center justify-between gap-3 border-b border-edge px-4 py-3">
                    <h2 class="text-sm font-semibold text-content">{title}</h2>
                    <button
                        type="button"
                        class="shrink-0 text-content-subtle hover:text-content"
                        aria-label=l!("common.close")
                        on:click=move |_| close.run(())
                    >
                        <Icon icon=Icon::X size=IconSize::Sm />
                    </button>
                </header>
                <div class="p-4">{children()}</div>
            </div>
        </div>
    }
}
