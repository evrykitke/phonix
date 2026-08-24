//! Where an invitation link lands.
//!
//! Reachable with no session - the person following it does not have one, which
//! is the point. The path is
//! [`INVITATION_ACCEPT_PATH`](phonix_core::identity::INVITATION_ACCEPT_PATH),
//! the same constant the service builds the link from and the navigation guard
//! treats as public.
//!
//! # The token stays in the URL and never in a field
//!
//! It is read from `?token=` and submitted from there. Showing it in a box
//! would invite pasting the wrong thing, and a token somebody can edit is a
//! token somebody will try to edit.
//!
//! # A spent link is not retried
//!
//! Accepting consumes the token *before* the password is checked - see the
//! service - so a rejected password leaves the link spent. That is deliberate:
//! a link that survived a failed attempt is a link that can be probed. The
//! screen therefore says to ask for a new invitation rather than offering
//! another go at the same one.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_query_map;
use phonix_core::identity::SIGN_IN_PATH;

use crate::components::forms::PasswordInput;
use crate::components::page::{Notice, Panel, PrimaryButton, Tone};
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::server_fns::auth_fns::{AcceptInvitationResult, accept_invitation};

#[component]
pub fn accept_invitation_page() -> impl IntoView {
    let query = use_query_map();
    let token = move || query.with(|params| params.get("token").unwrap_or_default());

    let password = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());
    let mismatch = RwSignal::new(false);

    let submit = Action::new(|(token, password): &(String, String)| {
        let (token, password) = (token.clone(), password.clone());
        async move { accept_invitation(token, password).await }
    });

    let outcome = move || submit.value().get();

    view! {
        <Title text=format!("{} | Phonix", l!("accept.title")) />

        <div class="mx-auto w-full max-w-measure rounded-card border border-edge bg-surface-raised p-5 shadow-sm sm:p-8">
            {move || match outcome() {
                // Done. Nothing left to fill in, so the form goes away rather
                // than sitting there inviting a second attempt.
                Some(Ok(AcceptInvitationResult::Accepted { email })) => {
                    view! { <Accepted email=email /> }.into_any()
                }
                Some(Ok(AcceptInvitationResult::LinkNotUsable)) => {
                    view! { <NotUsable /> }.into_any()
                }
                _ => {
                    view! {
                        <Panel
                            title=l!("accept.title")
                            description=l!("accept.subtitle")
                        >
                            <form
                                class="space-y-3"
                                on:submit=move |event| {
                                    event.prevent_default();

                                    // Checked here and nowhere else: the server
                                    // has only one password to look at and
                                    // cannot know what was typed twice.
                                    if password.get_untracked() != confirm.get_untracked() {
                                        mismatch.set(true);
                                        return;
                                    }

                                    mismatch.set(false);
                                    submit.dispatch((token(), password.get_untracked()));
                                }
                            >
                                {move || {
                                    matches!(outcome(), Some(Ok(AcceptInvitationResult::Rejected(_))))
                                        .then(|| {
                                            let messages = match outcome() {
                                                Some(Ok(AcceptInvitationResult::Rejected(errors))) => {
                                                    errors
                                                        .into_iter()
                                                        .map(|error| crate::i18n::t(&error.message))
                                                        .collect::<Vec<_>>()
                                                        .join(" ")
                                                }
                                                _ => String::new(),
                                            };

                                            view! {
                                                <Notice
                                                    message=Signal::derive(move || {
                                                        Some(messages.clone())
                                                    })
                                                    tone=Tone::Danger
                                                />
                                                <p class="text-xs text-content-subtle">
                                                    "That link has now been used. If this does not \
                                                     work, ask for a new invitation."
                                                </p>
                                            }
                                        })
                                }}

                                {move || {
                                    submit
                                        .value()
                                        .get()
                                        .and_then(Result::err)
                                        .map(|err| {
                                            view! {
                                                <Notice
                                                    message=Signal::derive(move || {
                                                        Some(err.to_string())
                                                    })
                                                    tone=Tone::Danger
                                                />
                                            }
                                        })
                                }}

                                {move || {
                                    mismatch
                                        .get()
                                        .then(|| {
                                            view! {
                                                <Notice
                                                    message=Signal::derive(move || {
                                                        Some(l!("accept.mismatch"))
                                                    })
                                                    tone=Tone::Danger
                                                />
                                            }
                                        })
                                }}

                                <div class="space-y-1.5">
                                    <label
                                        for="new-password"
                                        class="block text-sm font-medium text-content"
                                    >
                                        {l!("field.password")}
                                    </label>
                                    <PasswordInput
                                        id="new-password"
                                        value=password
                                        autocomplete="new-password"
                                    />
                                </div>

                                <div class="space-y-1.5">
                                    <label
                                        for="confirm-password"
                                        class="block text-sm font-medium text-content"
                                    >
                                        {l!("accept.password_again")}
                                    </label>
                                    <PasswordInput
                                        id="confirm-password"
                                        value=confirm
                                        autocomplete="new-password"
                                    />
                                </div>

                                <PrimaryButton
                                    label=l!("accept.submit")
                                    icon=Icon::Check
                                    button_type="submit"
                                    pending=Signal::derive(move || submit.pending().get())
                                />
                            </form>
                        </Panel>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

#[component]
fn accepted(email: String) -> impl IntoView {
    view! {
        <Panel title=l!("accept.done")>
            <div class="space-y-3">
                // One sentence. Where the address falls in it is grammar,
                // and splitting it around a styled span decides that here.
                <p class="text-sm text-content">
                    {l!("accept.done_detail", email = email)}
                </p>
                <a
                    href=SIGN_IN_PATH
                    class="inline-flex h-8 items-center gap-1.5 rounded-control bg-brand px-3 text-sm font-medium text-on-brand hover:bg-brand-hover"
                >
                    <Icon icon=Icon::ArrowRight size=IconSize::Xs />
                    {l!("accept.go_sign_in")}
                </a>
            </div>
        </Panel>
    }
}

/// Unknown, expired, or already used - said as one thing.
///
/// Distinguishing them would tell whoever intercepted a link that it was real,
/// which is the one fact worth withholding here.
#[component]
fn not_usable() -> impl IntoView {
    view! {
        <Panel title=l!("accept.unusable")>
            <div class="space-y-3">
                <p class="text-sm text-content">
                    {l!("accept.unusable_detail")}
                </p>
                <p class="text-sm text-content-muted">
                    {l!("accept.unusable_help")}
                </p>
            </div>
        </Panel>
    }
}
