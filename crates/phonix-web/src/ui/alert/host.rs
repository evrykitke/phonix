//! Drawing the alerts: the toast stack, the message box, the confirmation.
//!
//! # One layer, mounted once
//!
//! [`AlertLayer`] sits at the root of the application, above the router, for
//! two reasons. A toast raised by a save that then navigates has to outlive the
//! page that raised it - mounted inside a route, it would be unmounted by its
//! own success. And a dialog rendered inside the page is inside the page's
//! stacking context, where the sidebar or a sticky table header can be painted
//! over it.
//!
//! # Nothing is rendered until something is posted
//!
//! Every surface here is empty on the server and empty in the browser on the
//! first frame, which is what makes the layer hydration-safe: the server cannot
//! post an alert, so both halves agree on nothing. See
//! `[[hashed-bundles-prevent-stale-hydration]]` for what disagreeing costs.
//!
//! # Fixed overlays and the phone viewport
//!
//! Everything here is `position: fixed`, and a fixed overlay is only where you
//! put it while the viewport is the width of the device. Anything inside these
//! cards that cannot shrink - a long unbroken word, a wide table - would widen
//! the page and carry the toast off the side of a phone. Hence
//! `w-[min(22rem,100%)]` rather than a fixed width, and `break-words` on the
//! message: an alert usually carries a sentence from the server, which is not
//! text this file has seen.

use leptos::prelude::*;

use super::{Alerts, Confirm, Posted};
use crate::components::page::Tone;
use crate::icons::{Icon, IconSize};
use crate::l;

/// Every alert surface, mounted once at the root of the application.
#[component]
pub fn alert_layer() -> impl IntoView {
    let alerts = Alerts::get();

    // Escape dismisses whatever is up. On the window rather than on the dialog,
    // because the key has to work whether or not focus made it into the card -
    // and a modal nobody can close without finding a button is a trap.
    //
    // A question is *declined*, never confirmed, by walking away from it.
    Effect::new(move |_| {
        let handle = window_event_listener(leptos::ev::keydown, move |event| {
            if event.key() == "Escape" {
                alerts.close();
                alerts.answer(false);
            }
        });

        on_cleanup(move || handle.remove());
    });

    view! {
        <ToastStack alerts=alerts />
        <MessageBoxAlert alerts=alerts />
        <ConfirmAlert alerts=alerts />
    }
}

/// The cards in the corner.
#[component]
fn toast_stack(alerts: Alerts) -> impl IntoView {
    let toasts = alerts.toasts;

    view! {
        // `pointer-events-none` on the column and `auto` on each card: the
        // container spans the width of the screen so the cards can align to its
        // end, and without this it would swallow every click along the bottom
        // of the page whether or not a toast was showing.
        <div
            class="pointer-events-none fixed inset-x-0 bottom-0 z-[60] flex flex-col items-end gap-2 p-3"
            aria-live="polite"
            aria-atomic="false"
        >
            <For each=move || toasts.get() key=|posted| posted.id let:posted>
                <ToastAlert alerts=alerts posted=posted />
            </For>
        </div>
    }
}

/// One card, which says its piece and leaves.
#[component]
fn toast_alert(alerts: Alerts, posted: Posted) -> impl IntoView {
    let id = posted.id;
    let alert = posted.alert;
    let face = alert.tone.face();
    let title = alert.title.clone();
    let message = alert.message.clone();

    view! {
        <div
            role="status"
            class=format!(
                "alert-enter pointer-events-auto flex w-[min(22rem,100%)] items-start gap-2.5 \
                 rounded-card border bg-surface-raised p-3 shadow-lg {}",
                face.edge,
            )
        >
            <span
                class=format!("grid size-6 shrink-0 place-items-center rounded-full {}", face.disc)
                aria-hidden="true"
            >
                <Icon icon=face.icon size=IconSize::Xs />
            </span>

            <div class="min-w-0 flex-1 space-y-0.5">
                {title
                    .map(|title| {
                        view! { <p class="text-sm font-medium text-content">{title}</p> }
                    })}
                <p class="break-words text-sm text-content-muted">{message}</p>
            </div>

            // Offered even on the ones that fade: five seconds is a long time
            // to sit under a card covering the thing you are reading.
            <button
                type="button"
                class="-m-1 grid size-6 shrink-0 place-items-center rounded-control text-content-subtle hover:bg-surface-hover hover:text-content"
                aria-label=l!("alert.dismiss")
                on:click=move |_| alerts.dismiss(id)
            >
                <Icon icon=Icon::X size=IconSize::Xs />
            </button>
        </div>
    }
}

/// An outcome that has to be read before anything else happens.
#[component]
fn message_box_alert(alerts: Alerts) -> impl IntoView {
    let boxed = alerts.boxed;
    let acknowledge = NodeRef::<leptos::html::Button>::new();

    // The button takes focus when the box opens, so Enter and Escape both close
    // it without anybody reaching for the mouse.
    Effect::new(move |_| {
        if boxed.get().is_some()
            && let Some(button) = acknowledge.get()
        {
            let _ = button.focus();
        }
    });

    view! {
        {move || {
                boxed
                    .get()
                    .map(|posted| {
                        let alert = posted.alert;

                        view! {
                            <div
                                class="fixed inset-0 z-[70] grid place-items-center bg-overlay p-4"
                                // The backdrop closes it. A message box asks for
                                // acknowledgement, and clicking away from it is
                                // one - unlike a question, which has an answer.
                                on:click=move |_| alerts.close()
                            >
                                <div
                                    role="alertdialog"
                                    aria-modal="true"
                                    class="alert-enter w-full max-w-md overflow-hidden rounded-card border border-edge bg-surface-raised shadow-xl"
                                    on:click=|event| event.stop_propagation()
                                >
                                    <AlertBody
                                        tone=alert.tone
                                        heading=alert
                                            .title
                                            .clone()
                                            .unwrap_or_else(|| heading_for(alert.tone))
                                        message=alert.message.clone()
                                    />

                                    <div class="flex justify-end gap-2 border-t border-edge px-4 py-3">
                                        <button
                                            type="button"
                                            node_ref=acknowledge
                                            class="inline-flex h-8 items-center gap-1.5 rounded-control bg-brand px-3 text-sm font-medium text-on-brand hover:bg-brand-hover"
                                            on:click=move |_| alerts.close()
                                        >
                                            "OK"
                                        </button>
                                    </div>
                                </div>
                            </div>
                        }
            })
        }}
    }
}

/// A question with two answers, asked before the deed rather than after.
#[component]
fn confirm_alert(alerts: Alerts) -> impl IntoView {
    let asking = alerts.asking;
    let cancel = NodeRef::<leptos::html::Button>::new();

    // Focus lands on *Cancel*, not on the button that goes ahead. A
    // confirmation is only asked for when the action cannot be undone, so a
    // stray Enter must not be the thing that does it.
    Effect::new(move |_| {
        if asking.get().is_some()
            && let Some(button) = cancel.get()
        {
            let _ = button.focus();
        }
    });

    view! {
        {move || {
                asking
                    .get()
                    .map(|confirm: Confirm| {
                        let confirming = if confirm.tone == Tone::Danger {
                            "inline-flex h-8 items-center gap-1.5 rounded-control bg-danger px-3 \
                             text-sm font-medium text-on-danger hover:bg-danger-hover"
                        } else {
                            "inline-flex h-8 items-center gap-1.5 rounded-control bg-brand px-3 \
                             text-sm font-medium text-on-brand hover:bg-brand-hover"
                        };

                        view! {
                            <div
                                class="fixed inset-0 z-[70] grid place-items-center bg-overlay p-4"
                                // Clicking away is declining. The safe answer is
                                // the one that happens by accident.
                                on:click=move |_| alerts.answer(false)
                            >
                                <div
                                    role="alertdialog"
                                    aria-modal="true"
                                    class="alert-enter w-full max-w-md overflow-hidden rounded-card border border-edge bg-surface-raised shadow-xl"
                                    on:click=|event| event.stop_propagation()
                                >
                                    <AlertBody
                                        tone=confirm.tone
                                        heading=confirm.heading().to_owned()
                                        message=confirm.question.clone()
                                    />

                                    <div class="flex justify-end gap-2 border-t border-edge px-4 py-3">
                                        <button
                                            type="button"
                                            node_ref=cancel
                                            class="inline-flex h-8 items-center gap-1.5 rounded-control border border-edge px-3 text-sm text-content-muted hover:bg-surface-hover hover:text-content"
                                            on:click=move |_| alerts.answer(false)
                                        >
                                            {l!("common.cancel")}
                                        </button>
                                        <button
                                            type="button"
                                            class=confirming
                                            on:click=move |_| alerts.answer(true)
                                        >
                                            {confirm.confirm_label.clone()}
                                        </button>
                                    </div>
                                </div>
                            </div>
                        }
            })
        }}
    }
}

/// The inside of a dialog: the disc, the heading and the words.
///
/// Shared by the message box and the confirmation so the two cannot drift
/// apart - they are the same card with a different number of buttons.
#[component]
fn alert_body(tone: Tone, heading: String, message: String) -> impl IntoView {
    let face = tone.face();

    view! {
        <div class="flex items-start gap-3 p-4">
            <span
                class=format!("grid size-9 shrink-0 place-items-center rounded-full {}", face.disc)
                aria-hidden="true"
            >
                <Icon icon=face.icon size=IconSize::Sm />
            </span>
            <div class="min-w-0 space-y-1 pt-0.5">
                <p class="text-sm font-semibold text-content">{heading}</p>
                <p class="break-words text-sm text-content-muted">{message}</p>
            </div>
        </div>
    }
}

/// What a message box is called when the alert did not name it.
///
/// A dialog with a blank heading reads as a rendering fault, and "Notice" above
/// a failure reads as an application that does not know what it just did.
fn heading_for(tone: Tone) -> String {
    match tone {
        Tone::Danger => "That did not work",
        Tone::Warning => "Worth knowing",
        Tone::Success => "Done",
        _ => "Notice",
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::alert::Alert;

    #[test]
    fn every_tone_names_its_own_dialog() {
        // Nothing here may come back blank: the heading is the largest text on
        // the card.
        for tone in [
            Tone::Neutral,
            Tone::Brand,
            Tone::Success,
            Tone::Warning,
            Tone::Danger,
        ] {
            assert!(!heading_for(tone).is_empty(), "{tone:?}");
        }

        assert_eq!(heading_for(Tone::Danger), "That did not work");
        assert_eq!(heading_for(Tone::Success), "Done");
    }

    #[test]
    fn an_alert_with_a_title_of_its_own_keeps_it() {
        let alert = Alert::success("Saved.").titled("Role saved");

        assert_eq!(alert.title.as_deref(), Some("Role saved"));
    }
}
