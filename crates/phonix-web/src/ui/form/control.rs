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

use leptos::prelude::*;

use super::field::{Choice, Field};
use super::kind::FieldKind;
use super::state::FormState;
use super::value::FieldValue;
use crate::l;
use crate::ui::lookup::SelectField;

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
