//! The sign-in screen.
//!
//! Rendered at `/`. It adapts to where it is running: on a workspace host the
//! tenant is known and the form asks for two things, on the bare domain it also
//! asks which workspace. See `server_fns::auth_fns` for why those are two
//! different code paths on the server.
//!
//! Nothing here checks whether the visitor is already signed in. That is
//! `phonix_core::identity::landing`, applied by the layout before this renders,
//! so an established session is turned around with a 302 rather than being
//! shown a form it then navigates away from.
//!
//! # Every string here comes from the catalog
//!
//! This screen is the worked example for `crate::i18n`. Two things about it are
//! worth copying:
//!
//! * `l!` for words this file owns, `t` for a [`Message`] that arrived from the
//!   server. A rejection from `sign_in` is the latter - the service decided
//!   *what* was wrong, and this decides how to say it here.
//! * The language switcher is on the page itself, because this is the one
//!   screen where somebody who cannot read it has no account to change the
//!   setting from.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::A;

use crate::components::forms::{FieldLabel, FormError, SubmitButton, TextInput};
use crate::components::language::LanguagePicker;
use crate::i18n::t;
use crate::l;
use crate::server_fns::auth_fns::{SignInInput, sign_in};
use crate::server_fns::tenant_fns::current_tenant;

#[component]
pub fn sign_in_page() -> impl IntoView {
    // Resolves to the workspace when this page is served from one, and errors
    // on the bare domain - which is not a failure here, it is the signal to ask
    // for the address.
    let tenant = OnceResource::new(current_tenant());

    let email = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let workspace = RwSignal::new(String::new());
    let remember_me = RwSignal::new(false);

    let message = RwSignal::new(Option::<String>::None);
    let submitting = RwSignal::new(false);

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();

        // The browser's own validation is bypassed by prevent_default, so the
        // obvious empty-form case is caught here rather than costing a round
        // trip and an Argon2 verification.
        if email.get().trim().is_empty() || password.get().is_empty() {
            message.set(Some(l!("auth.signin.incomplete")));
            return;
        }

        message.set(None);
        submitting.set(true);

        let input = SignInInput {
            email: email.get(),
            password: password.get(),
            remember_me: remember_me.get(),
            workspace: workspace.get(),
        };

        leptos::task::spawn_local(async move {
            match sign_in(input).await {
                Ok(response) => match (response.result, response.redirect_to) {
                    // Always a full load, never a router navigation - and for
                    // two separate reasons. A cross-origin target (the handoff
                    // to a workspace host) the router cannot reach at all. A
                    // same-origin one it can, but every resource on this page
                    // was resolved during SSR without a session cookie, and the
                    // layout sits outside the router so it would keep serving
                    // its signed-out answer.
                    (_, Some(target)) => {
                        let _ = window().location().set_href(&target);
                    }
                    (result, None) => {
                        submitting.set(false);
                        // The outcome names what happened; this resolves it.
                        // A lockout is a plural sentence, and which form it
                        // takes is the catalog's decision, not this file's.
                        message.set(Some(
                            result
                                .message()
                                .map(|reason| t(&reason))
                                .unwrap_or_else(|| l!("auth.signin.failed")),
                        ));
                    }
                },
                Err(err) => {
                    submitting.set(false);
                    tracing_error(&err);
                    message.set(Some(l!("auth.signin.transport")));
                }
            }
        });
    };

    view! {
        // Not `l!` plus a literal " | Phonix": the product's name is a name,
        // and joining it on here would leave a translator no way to reorder the
        // two halves.
        <Title text=format!("{} | Phonix", l!("auth.signin.title")) />

        <div class="mx-auto w-full max-w-sm py-12">
            <h1 class="text-2xl font-semibold tracking-tight text-content">
                {l!("auth.signin.title")}
            </h1>
            <p class="mt-1 text-sm text-content-muted">{l!("auth.signin.welcome")}</p>

            <form class="mt-8 space-y-5" on:submit=submit>
                // Only on the bare domain: on a workspace host the tenant comes
                // from the request, and a field here could point the attempt at
                // a different workspace than the page the user is looking at.
                <Suspense fallback=|| ()>
                    {move || Suspend::new(async move {
                        tenant
                            .await
                            .is_err()
                            .then(|| {
                                view! {
                                    <div>
                                        <FieldLabel
                                            for_id="workspace"
                                            text=l!("auth.signin.workspace")
                                        />
                                        // The placeholder is an example address,
                                        // not a word: it stays as it is in every
                                        // language, like a name would.
                                        <TextInput
                                            id="workspace"
                                            value=workspace
                                            placeholder=l!("signup.slug_placeholder")
                                            autocomplete="organization"
                                        />
                                        <p class="mt-1 text-xs text-content-subtle">
                                            {l!("auth.signin.workspace_hint")}
                                        </p>
                                    </div>
                                }
                            })
                    })}
                </Suspense>

                <div>
                    <FieldLabel for_id="email" text=l!("auth.signin.email") />
                    <TextInput
                        id="email"
                        input_type="email"
                        value=email
                        autocomplete="username"
                    />
                </div>

                <div>
                    <FieldLabel for_id="password" text=l!("auth.signin.password") />
                    <TextInput
                        id="password"
                        input_type="password"
                        value=password
                        autocomplete="current-password"
                    />
                </div>

                <label class="flex items-center gap-2 text-sm text-content-muted">
                    <input
                        type="checkbox"
                        class="h-4 w-4 rounded border-edge-strong"
                        prop:checked=move || remember_me.get()
                        on:change=move |ev| remember_me.set(event_target_checked(&ev))
                    />
                    {l!("auth.signin.remember")}
                </label>

                <FormError message=message />

                <SubmitButton label=l!("auth.signin.title") pending=submitting />
            </form>

            <p class="mt-6 text-sm text-content-muted">
                {l!("auth.signin.no_workspace")} " "
                <A href="/signup" attr:class="font-medium text-brand hover:underline">
                    {l!("auth.signin.create")}
                </A>
            </p>

            // Below the form rather than above it: somebody who can read the
            // page should reach the password field first, and somebody who
            // cannot is looking for exactly this and will find it either way.
            <div class="mt-10 border-t border-edge pt-4">
                <LanguagePicker />
            </div>
        </div>
    }
}

/// Log a transport failure where it can be seen, without showing it.
///
/// The message may name a host or an endpoint; the user gets a fixed string.
fn tracing_error(err: &ServerFnError) {
    leptos::logging::error!("sign-in request failed: {err}");
}
