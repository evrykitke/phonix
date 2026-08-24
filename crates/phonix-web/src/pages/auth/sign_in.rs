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
use leptos_router::hooks::use_query_map;

use phonix_core::identity::PASSWORD_RESET_PATH;

use crate::components::forms::{FieldLabel, FormError, SubmitButton, TextInput};
use crate::components::language::LanguagePicker;
use crate::i18n::t;
use crate::l;
use crate::server_fns::auth_fns::{SignInInput, sign_in};
use crate::server_fns::tenant_fns::{current_tenant, google_sign_in_url};

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

    // Where "Continue with Google" points, or `None` when this deployment does
    // not offer it. Cross-host, so only the server can build it.
    let google = OnceResource::new(google_sign_in_url());

    // A Google sign-in that failed comes back here as a query parameter,
    // because the flow finishes on a different host and has nowhere else to
    // put the reason. Read once on mount rather than watched: the browser
    // arrived with it, and nothing on this page changes it.
    Effect::new(move |_| {
        let query = use_query_map();
        let reason = query.with_untracked(|params| params.get("google"));

        if let Some(reason) = reason {
            message.set(Some(match reason.as_str() {
                // Verified by Google, and not a member here. Said plainly -
                // see the note in `phonix_server::google` on why this one
                // outcome is allowed to be specific.
                "no_account" => l!("auth.google.no_account"),
                "refused" => l!("auth.google.refused"),
                "expired" => l!("auth.google.expired"),
                "unavailable" => l!("auth.google.unavailable"),
                _ => l!("auth.google.failed"),
            }));
        }
    });

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

            // Below the password form, not above it. The credential most
            // people here are about to use is the one they typed into this
            // workspace, and a provider button at the top of a sign-in screen
            // reliably collects people who then cannot remember which of the
            // two they used last time.
            <Suspense fallback=|| ()>
                {move || Suspend::new(async move {
                    google.await.ok().flatten().map(|href| {
                        view! {
                            <div class="mt-6 flex items-center gap-3">
                                <span class="h-px flex-1 bg-edge"></span>
                                <span class="text-xs text-content-subtle">
                                    {l!("auth.signin.or")}
                                </span>
                                <span class="h-px flex-1 bg-edge"></span>
                            </div>

                            // A plain link, not a button with a handler: this
                            // is a top-level navigation to another host, which
                            // is exactly what an anchor is.
                            <a
                                href=href
                                class="mt-4 flex w-full items-center justify-center gap-2                                        rounded-md border border-edge-strong px-4 py-2                                        font-medium text-content hover:bg-surface-sunken                                        focus:outline-none focus:ring-2 focus:ring-brand"
                            >
                                <GoogleMark />
                                {l!("auth.google.continue")}
                            </a>
                        }
                    })
                })}
            </Suspense>

            // Only on a workspace host, and unlike the signup link below this
            // is not about spam - it is that the screen would not work. A reset
            // needs an account in one workspace's database, and on the bare
            // domain the request carries no tenant to look in. Somebody who
            // arrives there types their workspace address into the field above
            // and reaches their own host, where this appears.
            <Suspense fallback=|| ()>
                {move || Suspend::new(async move {
                    tenant
                        .await
                        .is_ok()
                        .then(|| {
                            view! {
                                <p class="mt-4 text-sm">
                                    <A
                                        href=PASSWORD_RESET_PATH
                                        attr:class="text-content-muted hover:underline"
                                    >
                                        {l!("auth.signin.forgot")}
                                    </A>
                                </p>
                            }
                        })
                })}
            </Suspense>

            // Only on the bare domain, and for the same reason the workspace
            // field is: this page is somebody's front door to a workspace that
            // already exists, and the one thing behind this link is building
            // another one. Offering it here turns a mis-click into a second
            // Postgres database. `create_workspace` refuses on a tenant host
            // regardless - this is so nobody is invited to find that out.
            <Suspense fallback=|| ()>
                {move || Suspend::new(async move {
                    tenant
                        .await
                        .is_err()
                        .then(|| {
                            view! {
                                <p class="mt-6 text-sm text-content-muted">
                                    {l!("auth.signin.no_workspace")} " "
                                    <A
                                        href="/signup"
                                        attr:class="font-medium text-brand hover:underline"
                                    >
                                        {l!("auth.signin.create")}
                                    </A>
                                </p>
                            }
                        })
                })}
            </Suspense>

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

/// Google's "G", inline.
///
/// Drawn here rather than fetched: the strict content-security policy this app
/// is served under blocks an external image, and a sign-in button whose logo
/// silently fails to load is a button people do not recognise. The four paths
/// are Google's own brand mark and its colours are fixed - it is a logo, so it
/// does not take a theme token and does not change in dark mode.
#[component]
fn google_mark() -> impl IntoView {
    view! {
        <svg
            class="h-4 w-4"
            viewBox="0 0 18 18"
            xmlns="http://www.w3.org/2000/svg"
            aria-hidden="true"
            focusable="false"
        >
            <path
                fill="#4285F4"
                d="M17.64 9.2c0-.64-.06-1.25-.16-1.84H9v3.48h4.84a4.14 4.14 0 0 1-1.8 2.72v2.26h2.92c1.7-1.57 2.68-3.88 2.68-6.62Z"
            />
            <path
                fill="#34A853"
                d="M9 18c2.43 0 4.47-.8 5.96-2.18l-2.92-2.26c-.8.54-1.84.86-3.04.86-2.34 0-4.32-1.58-5.03-3.7H.96v2.33A9 9 0 0 0 9 18Z"
            />
            <path
                fill="#FBBC05"
                d="M3.97 10.72a5.41 5.41 0 0 1 0-3.44V4.95H.96a9 9 0 0 0 0 8.1l3.01-2.33Z"
            />
            <path
                fill="#EA4335"
                d="M9 3.58c1.32 0 2.5.45 3.44 1.35l2.58-2.58C13.46.89 11.43 0 9 0A9 9 0 0 0 .96 4.95l3.01 2.33C4.68 5.16 6.66 3.58 9 3.58Z"
            />
        </svg>
    }
}
