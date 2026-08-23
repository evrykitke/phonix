//! The entity form: one component, one configuration per entity.
//!
//! # What it is for
//!
//! The same argument as the [data grid](crate::ui::table). Every module ends up
//! needing the same form: labelled controls, required fields, a save that
//! validates, disables itself, puts the server's complaints back on the right
//! inputs, and does something sensible afterwards. Written per screen those
//! come out slightly different every time - this one shows its errors at the
//! top, that one loses them; this one can be double-submitted, that one cannot.
//!
//! ```ignore
//! <EntityForm config=user_form(roles) value=user />
//! ```
//!
//! # The pieces
//!
//! | Module      | What it decides                                       |
//! | ----------- | ----------------------------------------------------- |
//! | [`field`]   | what a field is, and how one value is read and written |
//! | [`kind`]    | which control it draws                                 |
//! | [`value`]   | what a control holds while it is being edited          |
//! | [`action`]  | what the buttons do, and what happens after            |
//! | [`config`]  | the whole configuration, and one file per entity       |
//! | [`state`]   | what the person has done to it since it opened         |
//! | [`control`] | drawing one field                                      |
//!
//! # The three things worth knowing before changing it
//!
//! **A field is not a column.** They are siblings that share the entity and the
//! field identifiers; neither is derived from the other. See [`config`] for
//! why - the short version is that a `Cell` is one-way and lossy.
//!
//! **Gating a field hides nothing.** A field the viewer may not edit renders
//! disabled, not absent. A hidden field is still submitted, as whatever the
//! draft held, and quietly overwrites a value they were not allowed to see.
//! This is the opposite of the rule for actions, where hiding is right.
//!
//! **Validation here is a courtesy.** [`FormConfig::missing`] saves a round
//! trip and puts the message next to the control. `Caller::require` and the
//! service's own validation are what actually refuse.

pub mod action;
pub mod config;
pub mod control;
pub mod field;
pub mod kind;
pub mod state;
pub mod value;

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

pub use action::{FormAction, Then};
pub use config::FormConfig;
pub use field::{Choice, Field};
pub use kind::FieldKind;
pub use state::FormState;
pub use value::FieldValue;

use self::action::ActionKind;
use self::control::FormField;
use crate::components::page::{Notice, Tone};
use crate::icons::{Icon, IconSize};
use crate::ui::alert::{Alert, Alerts, Channel, Confirm};
use crate::ui::viewer::Viewer;

/// What a draft type has to be for a form to hold it.
///
/// `PartialEq` because that is what "dirty" means, and dirty is what stops an
/// untouched form being written back to the database.
pub trait Draft: Clone + PartialEq + Send + Sync + 'static {}

impl<T: Clone + PartialEq + Send + Sync + 'static> Draft for T {}

/// What a form does to the screen around it.
///
/// A form does not know whether it is a page or a modal, and it must not: the
/// same configuration is used for both. It reports, and whoever put it there
/// decides what closing means.
#[derive(Clone, Copy)]
pub struct FormHost {
    /// Re-read the list this form was opened from. `None` when it was not.
    pub refresh: Option<Callback<()>>,
    /// Take the form off the screen.
    pub close: Option<Callback<()>>,
}

impl FormHost {
    /// A form standing on its own page: nothing to refresh, nothing to close.
    pub const fn page() -> Self {
        Self {
            refresh: None,
            close: None,
        }
    }
}

impl Default for FormHost {
    fn default() -> Self {
        Self::page()
    }
}

/// A configurable form.
///
/// See the [module documentation](self) for what goes in the configuration.
#[component]
pub fn entity_form<T: Draft>(
    config: FormConfig<T>,
    /// What the form opens on. An edit form gets the stored entity; a create
    /// form gets a default.
    value: T,
    #[prop(optional)] host: FormHost,
) -> impl IntoView {
    let state = FormState::new(value);
    let viewer = Viewer::get();
    let alerts = Alerts::get();

    let fields = config.fields().to_vec();
    let names = config.field_names();
    let buttons = config.buttons();
    let note = config.note.clone();
    let columns = config.columns;
    let form_id = config.id();
    let reports = config.reports;
    let config = StoredValue::new(config);

    // Computed here rather than in the attribute: rstml reads the `>` of
    // `columns > 1` as the end of the tag.
    let layout = if columns > 1 {
        "grid gap-3 sm:grid-cols-2"
    } else {
        "grid gap-3"
    };

    // The Real Estate Rule, at the level a form can answer it: a column is a
    // place to put a control, and a control that has to fill 700px of a wide
    // monitor is not answering a question anybody asked. The ceiling is on the
    // `<form>` rather than on the grid so the button row's rule stops where the
    // fields stop, instead of running out into empty space.
    //
    // Two columns need room for two; one column carries the multiline fields,
    // which want a comfortable line rather than a comfortable name.
    let measure = if columns > 1 {
        "max-w-5xl space-y-4"
    } else {
        "max-w-3xl space-y-4"
    };

    let submit = move |chain: Vec<Then<T>>| {
        let user = viewer.get_untracked();

        // Validated, then claimed, then sent. Claiming after validating means a
        // form that fails the check is not left looking busy.
        if !config.with_value(|config| state.check(config, user.as_ref())) {
            return;
        }
        if !state.begin() {
            return;
        }

        let draft = state.draft.get_untracked();
        let sending = config.with_value(|config| config.send(draft));

        leptos::task::spawn_local(async move {
            let outcome = sending.await;
            state.finish();

            match outcome {
                Ok(phonix_core::form::Submission::Saved(saved)) => {
                    state.accept(saved.clone());
                    run_chain(&chain, &saved, state, host, reports, alerts);
                }
                // A rejection is not reported through the channel: it names
                // fields, and a field is a better place for "required" than a
                // card in the corner of the screen. What names nothing this
                // form shows is surfaced above the fields - see
                // `FormState::unplaced`.
                Ok(phonix_core::form::Submission::Rejected(errors)) => state.reject(errors),
                // The request itself failed - not permitted, or the server is
                // unwell. Its own words, which are better than a house phrase,
                // and down the same channel the good news would have taken.
                Err(message) => {
                    report(Alert::failure(message).through(reports), state, alerts);
                }
            }
        });
    };

    view! {
        <form
            class=measure
            // The browser's own validation is turned off: it cannot know the
            // rules the service applies, and its bubbles appear next to a
            // control the page may have scrolled away from.
            novalidate="true"
            on:submit=move |event| {
                event.prevent_default();
                let chain = config
                    .with_value(|config| {
                        config
                            .buttons()
                            .into_iter()
                            .find(FormAction::submits)
                            .map(|action| action.chain().to_vec())
                            .unwrap_or_default()
                    });
                submit(chain);
            }
        >
            {note.map(|note| view! { <p class="text-sm text-content-muted">{note}</p> })}

            {move || {
                state
                    .notice
                    .get()
                    .map(|(message, good)| {
                        view! {
                            <Notice
                                message=Signal::derive(move || Some(message.clone()))
                                tone=if good { Tone::Success } else { Tone::Danger }
                            />
                        }
                    })
            }}

            // Anything the server complained about that names no field here.
            // Dropping these is what makes a save look like a dead button.
            {move || {
                let unplaced = state.unplaced(&names);

                (!unplaced.is_empty())
                    .then(|| {
                        view! {
                            <Notice
                                message=Signal::derive(move || Some(unplaced.join(" ")))
                                tone=Tone::Danger
                            />
                        }
                    })
            }}

            <div class=layout>
                {move || {
                    let user = viewer.get();
                    let draft = state.draft.get();

                    fields
                        .iter()
                        .filter(|field| field.applies_to(&draft))
                        .cloned()
                        .map(|field| {
                            let editable = field.editable_by(user.as_ref());

                            view! {
                                <FormField
                                    form_id=form_id
                                    state=state
                                    field=field
                                    editable=editable
                                />
                            }
                        })
                        .collect::<Vec<_>>()
                }}
            </div>

            <div class="flex flex-wrap items-center justify-end gap-2 border-t border-edge pt-3">
                {move || {
                    let user = viewer.get();

                    buttons
                        .iter()
                        .filter(|action| action.permitted(user.as_ref()))
                        .cloned()
                        .map(|action| {
                            view! {
                                <ActionButton
                                    action=action
                                    state=state
                                    host=host
                                    reports=reports
                                    alerts=alerts
                                    on_submit=Callback::new(submit)
                                />
                            }
                        })
                        .collect::<Vec<_>>()
                }}
            </div>
        </form>
    }
}

/// One button at the foot of the form.
#[component]
fn action_button<T: Draft>(
    action: FormAction<T>,
    state: FormState<T>,
    host: FormHost,
    reports: Channel,
    alerts: Alerts,
    on_submit: Callback<Vec<Then<T>>>,
) -> impl IntoView {
    let primary = "inline-flex h-8 items-center gap-1.5 rounded-control bg-brand px-3 text-sm \
                   font-medium text-on-brand hover:bg-brand-hover \
                   disabled:cursor-not-allowed disabled:opacity-60";
    let quiet = "inline-flex h-8 items-center gap-1.5 rounded-control border border-edge px-3 \
                 text-sm text-content-muted hover:bg-surface-hover hover:text-content \
                 disabled:cursor-not-allowed disabled:opacity-60";

    let label = action.label().to_owned();
    let icon = action.icon;
    let confirm = action.confirm.clone();
    let submits = action.submits();
    let class = if action.primary { primary } else { quiet };
    let action = StoredValue::new(action);

    // What the button does once there is nothing left to ask. It is a closure
    // rather than the body of the click handler because a confirmation dialog
    // cannot be waited on - the deed has to be a thing the dialog can run
    // later. Everything it captures is a copyable handle, so it goes into a
    // `Confirm` without an `Arc` in sight.
    let perform = move || {
        action.with_value(|action| match &action.kind {
            ActionKind::Submit => on_submit.run(action.chain().to_vec()),
            ActionKind::Run(run) => {
                run.run(state.draft.get_untracked());
                run_chain(
                    action.chain(),
                    &state.draft.get_untracked(),
                    state,
                    host,
                    reports,
                    alerts,
                );
            }
            ActionKind::Cancel => {
                run_chain(
                    action.chain(),
                    &state.draft.get_untracked(),
                    state,
                    host,
                    reports,
                    alerts,
                );
            }
        });
    };

    // A save is offered only when there is something to save. A cancel is
    // always offered - a form nobody has touched still has to be leavable.
    let disabled = move || state.is_sending() || (submits && !state.is_dirty());

    view! {
        <button
            type=if submits { "submit" } else { "button" }
            class=class
            disabled=disabled
            on:click=move |event| {
                if submits {
                    // The form's own submit handler runs this one; doing it
                    // here as well would send twice.
                    return;
                }

                event.prevent_default();

                match confirm.clone() {
                    None => perform(),
                    // A dialog cannot be waited on the way `window.confirm`
                    // could, so the deed goes in as a callback and the
                    // question comes back later. See `ui::alert::confirm`.
                    Some(question) => {
                        alerts
                            .ask(
                                Confirm::new(question, perform)
                                    .titled(label.clone())
                                    .confirm_label(label.clone()),
                            )
                    }
                }
            }
        >
            {move || {
                state
                    .is_sending()
                    .then(|| {
                        view! {
                            <span
                                class="size-3.5 animate-spin rounded-full border border-current border-t-transparent"
                                aria-hidden="true"
                            ></span>
                        }
                    })
            }}
            {icon.map(|icon| view! { <Icon icon=icon size=IconSize::Xs /> })}
            {label.clone()}
        </button>
    }
}

/// Run a chain in order.
///
/// The whole runner, in one place, which is what the closed enum buys: there is
/// no dispatch to find and no operator to write. `Navigate` is last in practice
/// because everything after leaving the page is unobservable, but nothing
/// enforces an order - a configuration that puts it first gets what it asked
/// for.
fn run_chain<T: Draft>(
    chain: &[Then<T>],
    saved: &T,
    state: FormState<T>,
    host: FormHost,
    reports: Channel,
    alerts: Alerts,
) {
    for then in chain {
        match then {
            // The words are the action's; where they appear is the form's.
            Then::Say(message) => {
                report(Alert::success(*message).through(reports), state, alerts);
            }
            // Unless the action said otherwise, in which case it wins.
            Then::Alert(alert) => report(alert.clone(), state, alerts),
            Then::Refresh => {
                if let Some(refresh) = host.refresh {
                    refresh.run(());
                }
            }
            Then::Reset => state.reset(),
            Then::Close => {
                if let Some(close) = host.close {
                    close.run(());
                }
            }
            Then::Navigate(to) => {
                use_navigate()(&to(saved), Default::default());
            }
        }
    }
}

/// Put one alert wherever this form reports.
///
/// The single place the channel is honoured, so a form cannot end up with a
/// success that toasts and a failure that does not appear at all: both arrive
/// here as an [`Alert`] and leave by the same door.
///
/// `Inline` is the only channel a form can draw itself - the notice is part of
/// the form's own markup - so it is the only one handled here rather than
/// handed to [`Alerts`].
fn report<T: Draft>(alert: Alert, state: FormState<T>, alerts: Alerts) {
    if alert.channel != Channel::Inline {
        alerts.post(alert);
        return;
    }

    match alert.tone {
        Tone::Danger | Tone::Warning => state.warn(alert.message),
        _ => state.say(alert.message),
    }
}
