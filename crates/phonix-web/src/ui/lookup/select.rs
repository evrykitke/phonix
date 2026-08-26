//! A dropdown over a closed set of values.
//!
//! # Why this exists next to `LookupField`
//!
//! A [`LookupField`](super::LookupField) picks a *record*: it holds a `Choice`
//! because it has to draw a label for something a table handed it, it can
//! create what is missing, and it can show two columns. A select picks a
//! *value* out of a list somebody wrote down - a theme, a reset period, a page
//! size, a currency code. It answers with a `String` and it never creates
//! anything.
//!
//! They are the same family and share the same panel: the arithmetic that puts
//! it on the screen is in [`place`](super::place), the four ways it goes away
//! are in [`panel`](super::panel). Only the field differs, and it differs
//! enough to be worth its own component - chips and a text box are the wrong
//! shape for a control that sits in a toolbar at `h-8`.
//!
//! # Why not the browser's `<select>`
//!
//! Because it is not ours. A native dropdown is drawn by the operating system:
//! it does not take the app's theme, which on a dark page means a white menu;
//! it does not take the app's type or spacing; it cannot put a line of
//! explanation under an option; and it looks different on every platform the
//! app is used on. The one thing it does better is being a control the
//! platform already knows, and that is what `role="listbox"`, the arrow keys,
//! Enter, Escape and a real `<button>` are here to buy back.
//!
//! ```ignore
//! <SelectField
//!     value=Signal::derive(move || period.get().as_str().to_owned())
//!     on_change=Callback::new(move |value: String| period.set(parse(&value)))
//!     options=vec![Choice::new("never", "Never"), Choice::new("yearly", "Yearly")]
//! />
//! ```

use leptos::prelude::*;

use super::panel::dismiss_when_moved;
use super::place::{self, At};
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::ui::form::field::Choice;

/// Above this many options the panel gets a box to filter them with.
///
/// Below it a filter is furniture: eight entries are read faster than they are
/// typed at, and a box that has to be tabbed past is a cost paid by every
/// three-item dropdown in the app to help the one with a hundred and sixty.
const SEARCH_ABOVE: usize = 8;

/// The narrowest a panel opens, whatever the trigger's width.
///
/// A select in a toolbar is as wide as the word inside it, and a panel that
/// copied that width would truncate every option to fit a trigger that says
/// "All".
const MIN_PANEL: f64 = 176.0;

/// A dropdown over values.
///
/// See the [module documentation](self) for how this differs from a lookup,
/// and why neither of them is a `<select>`.
#[component]
pub fn select_field(
    /// What is chosen. The empty string is nothing.
    #[prop(into)]
    value: Signal<String>,
    /// What to do about a new one. Called with the chosen value, or with the
    /// empty string when the field is cleared.
    ///
    /// A callback rather than a writable signal because the value at the other
    /// end is rarely a `String`: it is an enum, a field inside a draft, a
    /// number. The caller owns that conversion, and it is one line where it
    /// belongs instead of a type parameter here.
    on_change: Callback<String>,
    options: Vec<Choice>,
    /// What the field says when nothing is chosen.
    #[prop(optional, into)]
    placeholder: Option<String>,
    /// Offer a way back to nothing.
    ///
    /// Off by default: most selects are a required field over a closed set,
    /// where "none" is not one of the answers.
    #[prop(optional)]
    clearable: bool,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional, into)] invalid: Signal<bool>,
    /// Whether an answer is compulsory. Announced, not enforced - a dropdown
    /// cannot refuse a form, and the validator is what does.
    #[prop(optional)]
    required: bool,
    /// The ids of whatever explains this field: its help line, its error. Read
    /// out after the value, so somebody who cannot see the sentence under the
    /// control still gets it.
    #[prop(optional, into)]
    described_by: Signal<Option<String>>,
    /// Ties the field to a `<label for>`.
    #[prop(optional, into)]
    id: Option<String>,
    /// Announces the field where there is no visible label - a filter in a
    /// toolbar, the page size under a table.
    #[prop(optional, into)]
    label: Option<String>,
    /// Sizing and nothing else: `h-8 w-auto` for a toolbar, the default inside
    /// a form. The border, fill and radius come from `.lookup-shell` in the
    /// stylesheet - see the note on the `--control-*` tokens there.
    #[prop(optional, into)]
    class: Option<String>,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let query = RwSignal::new(String::new());
    let active = RwSignal::new(0_usize);
    let at = RwSignal::new(At::default());

    let anchor = NodeRef::<leptos::html::Div>::new();
    let panel = NodeRef::<leptos::html::Div>::new();
    let box_ref = NodeRef::<leptos::html::Input>::new();

    let searchable = options.len() > SEARCH_ABOVE;
    let options = StoredValue::new(options);

    // --- what is showing ---------------------------------------------------

    let matching = Signal::derive(move || {
        let needle = query.get().trim().to_lowercase();

        options.with_value(|options| {
            options
                .iter()
                .filter(|choice| {
                    needle.is_empty()
                        || choice.label.to_lowercase().contains(&needle)
                        // The detail line too, for the same reason the lookup
                        // searches it: that is where a code lives.
                        || choice
                            .detail
                            .as_deref()
                            .is_some_and(|detail| detail.to_lowercase().contains(&needle))
                })
                .cloned()
                .collect::<Vec<_>>()
        })
    });

    // The label for what is chosen. A value with no matching option reads as
    // nothing chosen rather than as itself: a raw id in a field is a bug
    // report, not a label.
    let chosen = Signal::derive(move || {
        let wanted = value.get();

        options.with_value(|options| {
            options
                .iter()
                .find(|choice| choice.value == wanted)
                .map(|choice| choice.label.clone())
        })
    });

    // --- opening and closing -----------------------------------------------

    let show = move || {
        if disabled.get_untracked() {
            return;
        }

        let _ = at.try_set(place::of(anchor, MIN_PANEL));
        let _ = query.try_set(String::new());
        let _ = open.try_set(true);

        // Opens on what is chosen, so the arrow keys start where somebody
        // already is rather than at the top of a list they scrolled past.
        let wanted = value.try_get_untracked().unwrap_or_default();
        let at_index = matching
            .try_get_untracked()
            .unwrap_or_default()
            .iter()
            .position(|choice| choice.value == wanted)
            .unwrap_or(0);
        let _ = active.try_set(at_index);

        // The box takes focus so that typing narrows the list instead of going
        // to the trigger. Without one the trigger keeps focus and keeps
        // driving the arrows, which is why this is the only focus move here.
        if searchable && let Some(input) = box_ref.try_get_untracked().flatten() {
            let _ = input.focus();
        }
    };

    let toggle = move || {
        if open.get_untracked() {
            let _ = open.try_set(false);
        } else {
            show();
        }
    };

    let take = move |choice: &Choice| {
        on_change.run(choice.value.clone());
        let _ = open.try_set(false);
        let _ = query.try_set(String::new());
    };

    dismiss_when_moved(open, panel, anchor);

    // --- the keyboard ------------------------------------------------------

    let on_key = move |event: leptos::ev::KeyboardEvent| match event.key().as_str() {
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
            if open.get_untracked() {
                let _ = active.try_update(|index| *index = index.saturating_sub(1));
            } else {
                show();
            }
        }
        "Enter" => {
            // Only while the panel is up. Closed, this is a button, and Enter
            // on a button belongs to the browser - swallowing it would also
            // swallow the Enter that submits the form the field is in.
            if open.get_untracked()
                && let Some(choice) = matching.get_untracked().get(active.get_untracked())
            {
                event.prevent_default();
                take(choice);
            }
        }
        "Escape" => {
            let _ = open.try_set(false);
        }
        _ => {}
    };

    // --- the field ---------------------------------------------------------

    // Only the state. The resting border and fill are `.lookup-shell` in the
    // stylesheet: the `--control-*` tokens are plain custom properties, so a
    // class like `bg-control-surface` looks right and compiles to nothing.
    let shell_class = move || {
        let edge = if invalid.get() {
            "border-danger"
        } else if open.get() {
            "border-brand"
        } else {
            ""
        };
        let sizing = class.clone().unwrap_or_default();

        format!("lookup-shell {sizing} {edge}")
    };

    let empty = placeholder.unwrap_or_else(|| l!("lookup.nothing_chosen"));

    view! {
        <div node_ref=anchor class="relative">
            <button
                type="button"
                id=id
                class=shell_class
                disabled=move || disabled.get()
                role="combobox"
                aria-haspopup="listbox"
                aria-expanded=move || if open.get() { "true" } else { "false" }
                aria-invalid=move || invalid.get().then_some("true")
                aria-required=required.then_some("true")
                aria-describedby=move || described_by.get()
                aria-label=label
                on:click=move |_| toggle()
                on:keydown=on_key
            >
                <span class="min-w-0 flex-1 truncate text-left">
                    {move || match chosen.get() {
                        Some(label) => {
                            view! { <span class="text-content">{label}</span> }.into_any()
                        }
                        None => {
                            view! { <span class="text-content-subtle">{empty.clone()}</span> }
                                .into_any()
                        }
                    }}
                </span>

                {clearable
                    .then(|| {
                        view! {
                            {move || {
                                (chosen.get().is_some() && !disabled.get())
                                    .then(|| {
                                        view! {
                                            // A `<button>` inside a `<button>`
                                            // is markup a browser may render
                                            // either way, so this says what it
                                            // is instead of being it. The
                                            // keyboard reaches the same place
                                            // through the list.
                                            <span
                                                role="button"
                                                tabindex="-1"
                                                class="shrink-0 text-content-subtle hover:text-content"
                                                aria-label=l!("lookup.clear")
                                                title=l!("lookup.clear")
                                                on:click=move |event| {
                                                    event.stop_propagation();
                                                    on_change.run(String::new());
                                                }
                                            >
                                                <Icon icon=Icon::X size=IconSize::Xs />
                                            </span>
                                        }
                                    })
                            }}
                        }
                    })}

                <span class="shrink-0 text-content-subtle" aria-hidden="true">
                    <Icon icon=Icon::ChevronDown size=IconSize::Xs />
                </span>
            </button>

            // In the DOM from the start and hidden by a class, never
            // `{open.then(..)}`: a node that first appears on a click is a node
            // leptos tries to hydrate against the comment the server left.
            <div
                node_ref=panel
                class="alert-enter z-[55] flex flex-col overflow-hidden rounded-card border border-edge bg-surface-raised shadow-pop"
                class:hidden=move || !open.get()
                aria-hidden=move || if open.get() { "false" } else { "true" }
                style=move || at.get().style()
            >
                {searchable
                    .then(|| {
                        view! {
                            <div class="shrink-0 border-b border-edge p-1.5">
                                <input
                                    node_ref=box_ref
                                    type="text"
                                    autocomplete="off"
                                    class="w-full max-w-none text-sm"
                                    placeholder=l!("lookup.search")
                                    prop:value=move || query.get()
                                    on:input=move |event| {
                                        let _ = query.try_set(event_target_value(&event));
                                        let _ = active.try_set(0);
                                    }
                                    on:keydown=on_key
                                />
                            </div>
                        }
                    })}

                <div role="listbox" class="min-h-0 flex-1 overflow-auto py-1">
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
                                // Derived rather than a closure, so both the
                                // tick and the attribute that announces it can
                                // read the same answer.
                                let is = choice.value.clone();
                                let ticked = Signal::derive(move || value.get() == is);

                                view! {
                                    <button
                                        type="button"
                                        role="option"
                                        aria-selected=move || {
                                            if ticked.get() { "true" } else { "false" }
                                        }
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
                                        // Pointer, not click: the dismissal
                                        // listener is a pointerdown on the
                                        // window and would otherwise race this.
                                        on:pointerdown=move |event| {
                                            event.prevent_default();
                                            take(&picked);
                                        }
                                        on:pointerenter=move |_| {
                                            let _ = active.try_set(index);
                                        }
                                    >
                                        <span class="flex min-w-0 items-center gap-1.5">
                                            <span
                                                class="w-3 shrink-0 text-brand"
                                                class:invisible=move || !ticked.get()
                                                aria-hidden="true"
                                            >
                                                <Icon icon=Icon::Check size=IconSize::Xs />
                                            </span>
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
            </div>
        </div>
    }
}
