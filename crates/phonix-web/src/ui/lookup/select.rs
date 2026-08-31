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
//!
//! # Every reactive read is guarded, this module's own signals included
//!
//! A select is drawn inside grid rows and inside the grid's pager, and a
//! `Transition` disposes the owner of what is on screen the moment the table
//! starts reloading. The markup stays: old rows are what a transition shows
//! while the new page is on its way, so every attribute closure hanging off
//! them is a live subscriber to signals that have already gone.
//!
//! Two things reach into that window. The obvious one is the signals a
//! *caller* hands in - `value`, `disabled`, `invalid` belong to the grid and
//! outlive the disposal, so they go on notifying subscribers allocated in the
//! owner that has just gone.
//!
//! The other is this module's own signals, which is the one that is easy to
//! argue yourself out of: `open` is disposed alongside everything that reads
//! it, so surely it can never be read late. It can, because marking an effect
//! dirty and running it are not the same tick. `take` closes the panel and
//! tells the caller in the same handler, and whichever order those go in, one
//! of them queues the field's class effect while the arena is alive and the
//! other disposes that arena before the queue is flushed. The effect then
//! runs against an `open` that no longer exists. That is what a page size
//! chosen in a grid panicked on, at the last bare `open.get()` in this file.
//!
//! So: **`try_*` every reactive read a view closure can re-run, and derive
//! nothing.** A `Signal::derive` and a `StoredValue` are arena values too - a
//! plain closure over `Copy` captures answers the same question and owns
//! nothing that can be disposed, which is why `chosen` and `matching` are
//! closures and why the option rows compare two strings instead of sharing a
//! memo. A select whose owner has gone reads as nothing chosen, shows an
//! empty list and drops its border colour, and all of that is honest: it is a
//! picture of a control that is about to be replaced.

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

// The panel asks for no width of its own: `place::fit` never draws one
// narrower than its field, so asking for nothing is asking for exactly the
// trigger's width. That is the instruction - a dropdown is the same size as
// the control it belongs to - and it costs something worth naming: a select
// in a toolbar is as wide as the word inside it, so a long option truncates
// there. Every option carries its full text as a `title` for that reason.
const SAME_AS_FIELD: f64 = 0.0;

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

    let matching = move || {
        let needle = query.try_get().unwrap_or_default().trim().to_lowercase();

        options
            .try_with_value(|options| {
                options
                    .iter()
                    .filter(|choice| {
                        needle.is_empty()
                            || choice.label.to_lowercase().contains(&needle)
                            // The detail line too, for the same reason the
                            // lookup searches it: that is where a code lives.
                            || choice
                                .detail
                                .as_deref()
                                .is_some_and(|detail| detail.to_lowercase().contains(&needle))
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };

    // The label for what is chosen. A value with no matching option reads as
    // nothing chosen rather than as itself: a raw id in a field is a bug
    // report, not a label.
    //
    // A closure, never a `Signal::derive`, for the reason spelled out at the
    // tick in the list below: `value` belongs to the caller and outlives this
    // control, so a signal derived here goes on being notified after its own
    // owner has been disposed. `try_get` is the floor under the same thing - a
    // select whose owner is gone reads as nothing chosen, which is honest,
    // because it is about to be replaced.
    let chosen = move || {
        let wanted = value.try_get()?;

        options.try_with_value(|options| {
            options
                .iter()
                .find(|choice| choice.value == wanted)
                .map(|choice| choice.label.clone())
        })?
    };

    // --- opening and closing -----------------------------------------------

    let show = move || {
        if disabled.try_get_untracked().unwrap_or(false) {
            return;
        }

        let _ = at.try_set(place::of(anchor, SAME_AS_FIELD));
        let _ = query.try_set(String::new());
        let _ = open.try_set(true);

        // Opens on what is chosen, so the arrow keys start where somebody
        // already is rather than at the top of a list they scrolled past.
        let wanted = value.try_get_untracked().unwrap_or_default();
        let at_index = matching()
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
        if open.try_get_untracked().unwrap_or(false) {
            let _ = open.try_set(false);
        } else {
            show();
        }
    };

    // `try_run`, like every other reactive touch in this module: a select can
    // sit inside a grid row or a pager, and a transition disposes the owner of
    // what is on screen the moment the table starts reloading. Running a
    // callback out of a disposed arena is a panic, and a panic in wasm takes
    // the whole page with it. See the note on zombie rows in `ui::table::menu`.
    //
    // Answering with nothing is the honest outcome here anyway: a control whose
    // owner is gone is about to be replaced, and the caller it would report to
    // no longer exists. What must not happen is that the caller is *alive* and
    // the callback is not - which is why a long-lived callback is the fix and
    // this is only the floor under it.
    //
    // The panel is put away first and the caller told second. Either order is
    // safe now that every read is guarded, but only this one actually closes
    // the control: a caller that refetches on the answer disposes this arena
    // while the handler is still running, and a `try_set` after that is a
    // no-op on a signal nobody is left to read.
    let take = move |choice: &Choice| {
        let _ = open.try_set(false);
        let _ = query.try_set(String::new());
        let _ = on_change.try_run(choice.value.clone());
    };

    dismiss_when_moved(open, panel, anchor);

    // --- the keyboard ------------------------------------------------------

    let on_key = move |event: leptos::ev::KeyboardEvent| match event.key().as_str() {
        "ArrowDown" => {
            event.prevent_default();
            if open.try_get_untracked().unwrap_or(false) {
                let last = matching().len().saturating_sub(1);
                let _ = active.try_update(|index| *index = (*index + 1).min(last));
            } else {
                show();
            }
        }
        "ArrowUp" => {
            event.prevent_default();
            if open.try_get_untracked().unwrap_or(false) {
                let _ = active.try_update(|index| *index = index.saturating_sub(1));
            } else {
                show();
            }
        }
        "Enter" => {
            // Only while the panel is up. Closed, this is a button, and Enter
            // on a button belongs to the browser - swallowing it would also
            // swallow the Enter that submits the form the field is in.
            if open.try_get_untracked().unwrap_or(false) {
                let rows = matching();

                if let Some(choice) = active.try_get_untracked().and_then(|at| rows.get(at)) {
                    event.prevent_default();
                    take(choice);
                }
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
        let edge = if invalid.try_get().unwrap_or(false) {
            "border-danger"
        } else if open.try_get().unwrap_or(false) {
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
                disabled=move || disabled.try_get().unwrap_or(false)
                role="combobox"
                aria-haspopup="listbox"
                aria-expanded=move || if open.try_get().unwrap_or(false) { "true" } else { "false" }
                aria-invalid=move || invalid.try_get().unwrap_or(false).then_some("true")
                aria-required=required.then_some("true")
                aria-describedby=move || described_by.try_get().flatten()
                aria-label=label
                on:click=move |_| toggle()
                on:keydown=on_key
            >
                <span class="min-w-0 flex-1 truncate text-left">
                    {move || match chosen() {
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
                                (chosen().is_some() && !disabled.try_get().unwrap_or(false))
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
                                                    let _ = on_change.try_run(String::new());
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
                class:hidden=move || !open.try_get().unwrap_or(false)
                aria-hidden=move || if open.try_get().unwrap_or(false) { "false" } else { "true" }
                style=move || at.try_get().unwrap_or_default().style()
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
                                    prop:value=move || query.try_get().unwrap_or_default()
                                    on:input=move |event| {
                                        let _ = query.try_set(event_target_value(&event));
                                        let _ = active.try_set(0);
                                    }
                                    on:keydown=on_key
                                />
                            </div>
                        }
                    })}

                // `overscroll-contain`: reaching the end of the list does not
                // hand the wheel on to the page, which would scroll the field
                // away and close the panel through the dismissal listener.
                <div
                    role="listbox"
                    class="min-h-0 flex-1 overflow-auto overscroll-contain py-1"
                >
                    {move || {
                        let rows = matching();

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
                                // The panel is the trigger's width, so an
                                // option longer than a narrow select truncates.
                                // This is where the rest of it is.
                                let full = choice.label.clone();
                                // Two closures over a cloned value, never one
                                // `Signal::derive`. A derived signal is an
                                // arena value allocated in whatever owner is
                                // current, and a select is drawn inside a
                                // grid pager and inside grid rows - owners a
                                // `Transition` disposes the moment the table
                                // starts reloading. `value` belongs to the
                                // grid and outlives that, so it goes on
                                // notifying subscribers that no longer exist,
                                // and reading one traps the module. Two string
                                // compares cost less than the signal did.
                                let ticked = {
                                    let is = choice.value.clone();
                                    move || value.try_get().is_some_and(|chosen| chosen == is)
                                };
                                let announced = ticked.clone();

                                view! {
                                    <button
                                        type="button"
                                        role="option"
                                        aria-selected=move || {
                                            if announced() { "true" } else { "false" }
                                        }
                                        class=move || {
                                            let state = if active.try_get() == Some(index) {
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
                                        title=full
                                    >
                                        <span class="flex min-w-0 items-center gap-1.5">
                                            <span
                                                class="w-3 shrink-0 text-brand"
                                                class:invisible=move || !ticked()
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
