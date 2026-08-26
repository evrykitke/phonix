//! Drawing one field: its label, its control, its message.
//!
//! # Every control is wired the same way
//!
//! Whatever the kind, a control reads its value out of the draft and writes it
//! back through the field's own writer. Nothing here knows what a `UserStatus`
//! is; it moves a [`FieldValue`] in one direction and back.
//!
//! # The accessibility wiring is here so it cannot be forgotten
//!
//! A label bound to the control by `for`/`id`, `aria-describedby` pointing at
//! the help text and the error, `aria-invalid` when there is a message, and
//! `aria-required`. Written once here, every field of every form in the
//! application has it; written per screen, roughly none of them would.
//!
//! # A lookup is wired in both directions, and it has to be
//!
//! Every other control reports a string, and the draft is the only thing
//! holding state. A [`LookupField`] owns a signal of its own, because what it
//! holds is records with labels attached and the draft cannot always supply
//! those from a value alone. So the two are kept in step by a pair of effects
//! rather than one.
//!
//! The pair is *necessary*: a one-way binding from the field into the draft
//! looks correct until somebody presses Cancel - [`FormState::reset`] replaces
//! the draft wholesale, and a field that was not listening would go on showing
//! the record the form was reset out of.
//!
//! It is also the one place in this file that can hang the browser, so both
//! halves are worth stating exactly. Two rules, and the first version of this
//! broke both of them:
//!
//! **The write-back must not read the draft reactively.** It writes the draft,
//! and `RwSignal::update` notifies whether or not the value changed - so an
//! effect that both subscribes to the draft and writes it re-runs itself for
//! ever. It reads through [`FormState::held`], which does not track.
//!
//! **The two sides must be able to agree.** They are compared by value and
//! label, not by whole [`Choice`], because a `Choice` may carry a `detail` line
//! that the picker attached and the draft has nowhere to store: rebuilt from
//! the draft it comes back as `None`, so full equality would never once hold
//! and the pair would write at each other for ever. Comparing what the draft
//! can actually round-trip is what makes the fixpoint reachable.

use leptos::prelude::*;

use super::field::{Choice, Field};
use super::kind::FieldKind;
use super::state::FormState;
use super::value::FieldValue;
use crate::l;
use crate::ui::lookup::{LookupField, SelectField};

/// One field, drawn.
#[component]
pub fn form_field<T>(
    form_id: &'static str,
    state: FormState<T>,
    field: Field<T>,
    /// Whether this viewer may change it. Decided by the form, which knows the
    /// viewer, rather than looked up again here.
    editable: bool,
) -> impl IntoView
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    let name = field.name();
    let id = format!("{form_id}-{name}");
    let help_id = format!("{id}-help");
    let error_id = format!("{id}-error");

    let label = field.label.clone();
    let help = field.help.clone();
    let required = field.required;
    let wide = field.wide;

    let error = move || state.error_for(name);

    // `aria-describedby` may name both, one, or neither. An id that points at
    // nothing is worse than an absent attribute: a screen reader announces the
    // gap.
    let described_by = {
        let help_id = help_id.clone();
        let error_id = error_id.clone();

        let has_help = help.is_some();

        move || {
            let mut ids = Vec::new();
            if has_help {
                ids.push(help_id.clone());
            }
            if error().is_some() {
                ids.push(error_id.clone());
            }

            (!ids.is_empty()).then(|| ids.join(" "))
        }
    };

    let control_id = id.clone();
    let field_for_control = field.clone();
    let label_for_field = label.clone();
    let help_text = help.clone();

    view! {
        <div class=move || {
            let span = if wide { "sm:col-span-2" } else { "" };
            format!("min-w-0 space-y-1 {span}")
        }>
            <Label
                id=id.clone()
                label=label_for_field
                required=required
                kind=field.kind.clone()
            />

            <Control
                id=control_id
                state=state
                field=field_for_control
                editable=editable
                invalid=Signal::derive(error)
                described_by=Signal::derive(described_by)
            />

            {help_text
                .map(|help| {
                    view! {
                        <p id=help_id.clone() class="text-xs text-content-subtle">
                            {help}
                        </p>
                    }
                })}

            <Show when=move || error().is_some() fallback=|| ()>
                <p id=error_id.clone() class="text-xs text-danger" role="alert">
                    {move || error()}
                </p>
            </Show>
        </div>
    }
}

/// The label, or nothing for a toggle - which carries its own.
#[component]
fn label(id: String, label: String, required: bool, kind: FieldKind) -> impl IntoView {
    // A checkbox's label belongs beside the box, not above it, so `Control`
    // draws that one itself.
    (!matches!(kind, FieldKind::Toggle)).then(|| {
        view! {
            <label for=id class="block text-xs font-medium text-content-muted">
                {label}
                {required
                    .then(|| {
                        view! {
                            // Marked for sight and for a screen reader, which
                            // reads the `aria-required` on the control itself.
                            <span class="ms-0.5 text-danger" aria-hidden="true">
                                "*"
                            </span>
                        }
                    })}
            </label>
        }
    })
}

#[component]
fn control<T>(
    id: String,
    state: FormState<T>,
    field: Field<T>,
    editable: bool,
    invalid: Signal<Option<String>>,
    described_by: Signal<Option<String>>,
) -> impl IntoView
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    let name = field.name();
    let field = StoredValue::new(field);

    // Read the current value; write it back through the field's own writer.
    let current = move || field.with_value(|field| state.value_of(|draft| field.value(draft)));
    let set = move |input: String| {
        field.with_value(|field| {
            let next = field.value(&state.draft.get_untracked()).with_input(input);

            state.edit(name, |draft| field.apply(draft, &next));
        });
    };

    // What the draft holds for this field, read without subscribing to it.
    // The reactive twin of this is `current`; see the module docs for why a
    // control that writes the draft must not also track it.
    let held = move || field.with_value(|field| state.held(|draft| field.value(draft)));

    // The same write, for a control that reports a whole value rather than a
    // string off the DOM. Only a lookup does: a record carries a label, and
    // `with_input` cannot invent one - see [`FieldValue::Records`].
    let put = move |next: FieldValue| {
        field.with_value(|field| {
            state.edit(name, |draft| field.apply(draft, &next));
        });
    };

    // Border, radius, background, padding and the disabled treatment come from
    // the global `input`/`textarea` rule in `style/main.css`. What is left is
    // the one thing that is this control's own business: whether the field is
    // currently being complained about.
    let class = move || {
        if invalid.get().is_some() {
            "border-danger"
        } else {
            ""
        }
    };

    let kind = field.with_value(|field| field.kind.clone());
    let placeholder = field.with_value(|field| field.placeholder.clone());
    let required = field.with_value(|field| field.required);

    match kind {
        FieldKind::Toggle => view! {
            <label class="flex cursor-pointer items-center gap-2 py-1 text-sm text-content">
                <input
                    type="checkbox"
                    id=id
                    class="size-4 shrink-0 accent-brand disabled:cursor-not-allowed disabled:opacity-60"
                    disabled=!editable
                    aria-describedby=move || described_by.get()
                    prop:checked=move || current().as_bool()
                    on:change=move |event| set(
                        if event_target_checked(&event) { "true".to_owned() } else { "false".to_owned() },
                    )
                />
                {field.with_value(|field| field.label.clone())}
            </label>
        }
            .into_any(),

        FieldKind::Multiline { rows } => view! {
            <textarea
                id=id
                rows=rows
                class=class
                disabled=!editable
                placeholder=placeholder
                aria-required=required.then_some("true")
                aria-invalid=move || invalid.get().is_some().then_some("true")
                aria-describedby=move || described_by.get()
                prop:value=move || current().as_input()
                on:input=move |event| set(event_target_value(&event))
            ></textarea>
        }
            .into_any(),

        FieldKind::Select { choices } => {
            // An unset field needs somewhere to sit, or it silently reads as
            // its first option - a value nobody chose. Where the field is not
            // required that place is also an answer, so it is an entry in the
            // list; where it is required it is only a prompt, and the way out
            // of a chosen value is to choose another one.
            let options = if required {
                choices
            } else {
                let mut options = vec![Choice::new(String::new(), l!("form.none"))];
                options.extend(choices);
                options
            };

            view! {
                <SelectField
                    id=id
                    value=Signal::derive(move || current().as_input())
                    on_change=Callback::new(move |value: String| set(value))
                    options=options
                    placeholder=if required { l!("form.choose_one") } else { l!("form.none") }
                    disabled=!editable
                    invalid=Signal::derive(move || invalid.get().is_some())
                    required=required
                    described_by=described_by
                />
            }
            .into_any()
        }

        FieldKind::Lookup {
            choices,
            quick_add,
            multiple,
        } => {
            // Seeded here rather than in an effect, so both ends start equal
            // and the first run of either has nothing to do. Seeding from an
            // effect would race: whichever ran first would win, and if that
            // were the write-back it would clear a draft that arrived with a
            // record already in it.
            let selected = RwSignal::new(current().as_records());

            // The draft changed under us - a reset, or a save the server
            // answered with a normalised entity. Anything the person had
            // chosen is gone, and the field has to say so.
            Effect::new(move |_| {
                let stored = current().as_records();

                // Only when they name different records. A selection that
                // agrees is left exactly as the picker gave it, which keeps
                // the detail line the draft could not have stored.
                if !same_records(&selected.get_untracked(), &stored) {
                    selected.set(stored);
                }
            });

            // Somebody chose. Read untracked and compared first: this effect
            // writes the draft, so tracking it would re-run this on its own
            // write, for ever. Comparing also stops an unchanged selection
            // clearing the field's error or making an untouched form dirty.
            Effect::new(move |_| {
                let chosen = selected.get();

                if !same_records(&held().as_records(), &chosen) {
                    put(FieldValue::Records(chosen));
                }
            });

            view! {
                <LookupField
                    id=id
                    selected=selected
                    choices=choices
                    multiple=multiple
                    quick_add=quick_add
                    placeholder=placeholder
                    disabled=Signal::derive(move || !editable)
                    invalid=Signal::derive(move || invalid.get().is_some())
                    required=required
                    described_by=described_by
                />
            }
            .into_any()
        }

        FieldKind::MultiSelect { choices } => view! {
            <fieldset
                class="space-y-1 rounded-control border border-edge p-2"
                aria-describedby=move || described_by.get()
            >
                <legend class="sr-only">
                    {field.with_value(|field| field.label.clone())}
                </legend>
                {choices
                    .into_iter()
                    .map(|choice| {
                        view! {
                            <ChoiceRow
                                choice=choice
                                editable=editable
                                current=Signal::derive(current)
                                on_toggle=Callback::new(move |value: String| set(value))
                            />
                        }
                    })
                    .collect::<Vec<_>>()}
            </fieldset>
        }
            .into_any(),

        kind => {
            let input_type = kind.input_type().unwrap_or("text");
            let (min, max, step) = match kind {
                FieldKind::Number { min, max, step } => (min, max, step),
                _ => (None, None, None),
            };

            view! {
                <input
                    type=input_type
                    id=id
                    class=class
                    disabled=!editable
                    placeholder=placeholder
                    min=min.map(|n| n.to_string())
                    max=max.map(|n| n.to_string())
                    step=step.map(|n| n.to_string())
                    aria-required=required.then_some("true")
                    aria-invalid=move || invalid.get().is_some().then_some("true")
                    aria-describedby=move || described_by.get()
                    prop:value=move || current().as_input()
                    on:input=move |event| set(event_target_value(&event))
                />
            }
                .into_any()
        }
    }
}

/// Whether two selections name the same records.
///
/// By value and label, which is all a draft can round-trip. A [`Choice`] may
/// also carry a `detail` line - a picker attaches one from the row it was
/// chosen from - and the reader that rebuilds a `Choice` out of the draft has
/// nowhere to get that from. Comparing whole `Choice`es would therefore report
/// two sides that hold the same record as different, every time, which is a
/// pair of effects that never agree.
fn same_records(held: &[Choice], chosen: &[Choice]) -> bool {
    held.len() == chosen.len()
        && held
            .iter()
            .zip(chosen)
            .all(|(held, chosen)| held.value == chosen.value && held.label == chosen.label)
}

/// One member of a multiple-choice field.
#[component]
fn choice_row(
    choice: Choice,
    editable: bool,
    current: Signal<FieldValue>,
    on_toggle: Callback<String>,
) -> impl IntoView {
    let value = choice.value.clone();
    let checked = {
        let value = value.clone();
        move || current.get().as_set().contains(&value)
    };

    view! {
        <label class="flex cursor-pointer items-start gap-2 rounded-control px-1 py-0.5 text-sm hover:bg-surface-hover">
            <input
                type="checkbox"
                class="mt-0.5 size-3.5 shrink-0 accent-brand disabled:cursor-not-allowed disabled:opacity-60"
                disabled=!editable
                prop:checked=checked
                on:change={
                    let value = value.clone();
                    move |_| on_toggle.run(value.clone())
                }
            />
            <span class="min-w-0">
                <span class="block text-content">{choice.label}</span>
                {choice
                    .detail
                    .map(|detail| {
                        view! { <span class="block text-xs text-content-subtle">{detail}</span> }
                    })}
            </span>
        </label>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usd() -> Choice {
        Choice::new("USD", "US Dollar")
    }

    #[test]
    fn a_record_rebuilt_from_a_draft_still_names_the_same_record() {
        // The bug this exists to stop: a picker attaches a detail line from
        // the row it was chosen from, the draft has nowhere to keep it, and
        // comparing whole `Choice`es made the two sides of the lookup's
        // binding disagree for ever - which hung the tab.
        let from_picker = usd().detail("United States");
        let from_draft = usd();

        assert_ne!(from_picker, from_draft);
        assert!(same_records(&[from_draft], &[from_picker]));
    }

    #[test]
    fn a_different_record_is_a_different_selection() {
        assert!(!same_records(&[usd()], &[Choice::new("EUR", "Euro")]));
        assert!(!same_records(&[usd()], &[]));
        assert!(!same_records(&[], &[usd()]));
    }

    #[test]
    fn a_renamed_record_is_a_change_the_field_has_to_take() {
        // Same id, new name. The draft is what the server corrected, so the
        // control has to follow it rather than keep the label it was given.
        assert!(!same_records(
            &[Choice::new("USD", "United States Dollar")],
            &[usd()]
        ));
    }

    #[test]
    fn nothing_chosen_agrees_with_nothing_chosen() {
        // Otherwise every lookup on a blank form writes on its first render.
        assert!(same_records(&[], &[]));
    }
}
