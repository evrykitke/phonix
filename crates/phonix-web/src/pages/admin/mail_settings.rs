//! Where this workspace's mail goes.
//!
//! The mail section of the settings screen's Communication tab, and its own
//! file because it loads its own data. The security policy and the relay are
//! separate rows, saved separately, and putting them in one form would mean one
//! save that half-succeeds.
//!
//! It is the whole of that tab today. The tab is named for the channel rather
//! than for the protocol because the ones beside it - SMS, push, whatever is
//! next - are the same kind of setting and belong on the same screen, each as
//! its own panel loading and saving its own row.
//!
//! # Two reads, not one
//!
//! [`mail_settings`] is what this workspace stored. [`mail_relay_in_use`] is
//! what will actually send, which depends on the system default as well. The
//! screen needs both, because "you have configured nothing and mail still
//! works" and "you have configured nothing and nothing is being delivered" look
//! identical from the stored row alone.

use leptos::prelude::*;
use phonix_core::audit::kinds;
use phonix_core::mail::MailSettings;

use crate::components::history::RecordHistory;
use crate::components::page::{Notice, Tone};
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::server_fns::admin_fns::{mail_relay_in_use, mail_settings, send_test_email};
use crate::ui::card::CollapsibleCard;
use crate::ui::form::EntityForm;
use crate::ui::form::config::mail::{draft_from, mail_form};

#[component]
pub fn mail_settings_tab() -> impl IntoView {
    let stored = Resource::new(|| (), |()| mail_settings());
    let in_use = Resource::new(|| (), |()| mail_relay_in_use());

    view! {
        <Transition fallback=|| {
            view! { <p class="text-sm text-content-subtle">"Loading..."</p> }
        }>
            {move || Suspend::new(async move {
                match (stored.await, in_use.await) {
                    (Ok(settings), Ok(relay)) => {
                        view! { <Editor settings=settings description=relay.describe() /> }
                            .into_any()
                    }
                    (Err(err), _) | (_, Err(err)) => {
                        view! {
                            <Notice
                                message=Signal::derive(move || Some(err.to_string()))
                                tone=Tone::Danger
                            />
                        }
                            .into_any()
                    }
                }
            })}
        </Transition>
    }
}

#[component]
fn editor(settings: MailSettings, description: String) -> impl IntoView {
    let has_password = settings.has_password;
    let draft = draft_from(&settings);

    view! {
        // Capped to the form it contains. The relay is eight fields, not a
        // reason to draw a card the width of the monitor.
        <div class="max-w-5xl space-y-3">
            <div class="flex items-start gap-2 rounded-control border border-edge bg-surface-sunken px-3 py-2 text-xs text-content-muted">
                <span class="mt-0.5 shrink-0">
                    <Icon icon=Icon::Mail size=IconSize::Xs />
                </span>
                <span>{description}</span>
            </div>

            // Open, because this card is the tab - see the same note on the
            // organization profile.
            <CollapsibleCard
                title=l!("mail.title")
                detail=l!("mail.description")
                icon=Icon::Mail
                open=true
            >
                <EntityForm config=mail_form(has_password) value=draft />
            </CollapsibleCard>

            <TestMessage />

            // Who pointed this workspace's mail somewhere else, and when. The
            // one setting that can redirect every invitation and every reset
            // link, so its history is worth having on the screen that changes
            // it.
            <RecordHistory
                kind=kinds::MAIL_SETTINGS
                id=Some(kinds::MAIL_SETTINGS.singleton_id().to_owned())
            />
        </div>
    }
}

/// Prove the relay works, without inviting somebody to find out.
///
/// Sends to the caller's own address and nowhere else - see the server function
/// for why a test that took a recipient would be a way to make this server mail
/// strangers.
#[component]
fn test_message() -> impl IntoView {
    let send = Action::new(|(): &()| async move { send_test_email().await });

    let result = move || match send.value().get() {
        None => None,
        Some(Ok(message)) => Some((message, Tone::Success)),
        Some(Err(err)) => Some((err.to_string(), Tone::Danger)),
    };

    view! {
        <CollapsibleCard
            title=l!("mail.test.title")
            detail=l!("mail.test.description")
            icon=Icon::CircleCheck
        >
            <div class="space-y-3">
                <button
                    type="button"
                    class="inline-flex h-8 items-center gap-1.5 rounded-control border border-edge px-3 text-sm text-content-muted hover:bg-surface-hover hover:text-content disabled:cursor-not-allowed disabled:opacity-60"
                    disabled=move || send.pending().get()
                    on:click=move |_| {
                        send.dispatch(());
                    }
                >
                    {move || {
                        send.pending()
                            .get()
                            .then(|| {
                                view! {
                                    <span
                                        class="size-3.5 animate-spin rounded-full border border-current border-t-transparent"
                                        aria-hidden="true"
                                    ></span>
                                }
                            })
                    }}
                    <Icon icon=Icon::Mail size=IconSize::Xs />
                    {l!("mail.test.send")}
                </button>

                // The relay's own words, whichever way it went. A house phrase
                // here would hide the one sentence that says what to fix.
                {move || {
                    result()
                        .map(|(message, tone)| {
                            view! {
                                <Notice
                                    message=Signal::derive(move || Some(message.clone()))
                                    tone=tone
                                />
                            }
                        })
                }}
            </div>
        </CollapsibleCard>
    }
}
