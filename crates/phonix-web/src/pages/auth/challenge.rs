//! The second-factor challenge, at `/auth/challenge`.
//!
//! # Where this sits
//!
//! ```text
//! password accepted, no MFA   ->  /dashboard
//! password accepted, MFA on   ->  /auth/challenge  ->  /dashboard
//! ```
//!
//! Between those two arrows the session exists but is *half* authenticated:
//! [`AuthUser::can`] answers false for every permission. That is why this page
//! asks the server who is waiting ([`pending_challenge`]) rather than reading
//! it from `current_user`, which reports nobody for such a session.
//!
//! Whether this screen should be showing at all is not decided here. That is
//! `phonix_core::identity::landing`, applied by the layout before a byte is
//! written - see [`crate::components::layout`]. A second answer in this file
//! would race the first, and did: it sent an anonymous visitor to the dashboard
//! while the layout was sending them to the form.
//!
//! # The code field takes both kinds of code
//!
//! Six digits from an authenticator app, or a recovery code. There is no radio
//! button choosing between them: the server decides by shape. A control the
//! caller sets would be one more thing to get wrong at the worst moment, and
//! it would tell an attacker which kind of secret the field is willing to
//! accept.

use leptos::prelude::*;
use leptos_meta::Title;
use phonix_core::identity::{DASHBOARD_PATH, MfaChallengeResult, SIGN_IN_PATH};

use crate::components::forms::{FieldLabel, FormError, SubmitButton, TextInput};
use crate::l;
use crate::server_fns::auth_fns::{PendingChallengeInfo, answer_mfa_challenge, pending_challenge};

#[component]
pub fn challenge_page() -> impl IntoView {
    // Blocking: this decides whether the page exists at all, and a redirect
    // that arrives after the form has been painted is a flash of a screen the
    // visitor should never have seen.
    let pending = OnceResource::new_blocking(pending_challenge());

    view! {
        <Title text=format!("{} | Phonix", l!("challenge.title")) />

        <div class="mx-auto w-full max-w-sm py-12">
            <Suspense fallback=|| {
                view! { <p class="text-sm text-content-subtle">"Loading..."</p> }
            }>
                {move || Suspend::new(async move {
                    match pending.await {
                        Ok(Some(info)) => view! { <ChallengeForm info=info /> }.into_any(),
                        // Nobody is waiting - the session finished
                        // authenticating in another tab, or expired. The layout
                        // has already redirected; rendering nothing is what
                        // keeps this from arguing with it.
                        Ok(None) => ().into_any(),
                        Err(err) => {
                            leptos::logging::error!("challenge lookup failed: {err}");
                            ().into_any()
                        }
                    }
                })}
            </Suspense>
        </div>
    }
}

#[component]
fn challenge_form(info: PendingChallengeInfo) -> impl IntoView {
    let code = RwSignal::new(String::new());
    let message = RwSignal::new(None::<String>);
    let submitting = RwSignal::new(false);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();

        let entered = code.get().trim().to_owned();
        if entered.is_empty() {
            message.set(Some(
                "Enter the code from your authenticator app.".to_owned(),
            ));
            return;
        }

        message.set(None);
        submitting.set(true);

        leptos::task::spawn_local(async move {
            match answer_mfa_challenge(entered).await {
                // A full load rather than a router navigation, for the same
                // reason sign-in does one: everything the layout resolved was
                // resolved for a session that had not finished authenticating.
                Ok(MfaChallengeResult::Accepted(_)) => leave_for(DASHBOARD_PATH),
                // The session is gone in both of these. Sending them back to
                // the form to try an eleventh code would be a lie.
                Ok(result @ (MfaChallengeResult::Exhausted | MfaChallengeResult::NoChallenge)) => {
                    submitting.set(false);
                    message.set(result.message());
                    leave_for(SIGN_IN_PATH);
                }
                Ok(result) => {
                    submitting.set(false);
                    code.set(String::new());
                    message.set(result.message());
                }
                Err(err) => {
                    submitting.set(false);
                    leptos::logging::error!("challenge request failed: {err}");
                    message.set(Some("Something went wrong. Try again.".to_owned()));
                }
            }
        });
    };

    let greeting = if info.display_name.trim().is_empty() {
        info.email.clone()
    } else {
        info.display_name.clone()
    };

    view! {
        <h1 class="text-2xl font-semibold tracking-tight text-content">
            {l!("challenge.title")}
        </h1>
        <p class="mt-1 text-sm text-content-muted">
            {l!("challenge.greeting", name = greeting)}
        </p>

        <form class="mt-8 space-y-5" on:submit=submit>
            <div>
                <FieldLabel for_id="code" text=l!("challenge.code") />
                <TextInput
                    id="code"
                    value=code
                    placeholder="000000"
                    // The standard token name: password managers and both
                    // mobile keyboards recognise it and offer the code.
                    autocomplete="one-time-code"
                />
                <p class="mt-1 text-xs text-content-subtle">
                    {if info.recovery_codes_allowed {
                        l!("challenge.code_help_recovery")
                    } else {
                        l!("challenge.code_help")
                    }}
                </p>
            </div>

            <FormError message=message />

            <SubmitButton label=l!("challenge.verify") pending=submitting />
        </form>

        <p class="mt-6 text-sm text-content-muted">
            {l!("challenge.lost_device")}
        </p>

        <button
            type="button"
            class="mt-3 text-sm font-medium text-brand hover:underline"
            on:click=move |_| {
                leptos::task::spawn_local(async move {
                    // Abandoning the half-authenticated session rather than
                    // just navigating away: leaving it open would let anyone
                    // at this browser return and finish it.
                    let _ = crate::server_fns::auth_fns::sign_out().await;
                    leave_for(SIGN_IN_PATH);
                });
            }
        >
            {l!("challenge.other_account")}
        </button>
    }
}

/// Leave this page for `path`.
///
/// A real 302 during SSR, so the form is never rendered; a full load in the
/// browser, so the layout re-resolves who is signed in.
#[cfg(feature = "ssr")]
fn leave_for(path: &str) {
    leptos_axum::redirect(path);
}

#[cfg(not(feature = "ssr"))]
fn leave_for(path: &str) {
    let _ = window().location().set_href(path);
}
