//! Your own account: who you are, your password, and your second factor.
//!
//! # Two tabs, matching the two halves of the subtitle
//!
//! **Profile** is who you are. **Security** is how you prove it - the password
//! and the second factor, which were a tab each until the strip was carrying
//! three entries for two questions.
//!
//! The two panels under Security keep their own state and their own save,
//! unlike the workspace settings screen where a tab covers one settings object
//! under one submit. That difference is the point: what earns a tab is a
//! separate question, not a separate request. Sessions and devices join Security
//! when they exist, and the strip does not grow.
//!
//! # Enrolment is three steps and cannot be shortened
//!
//! ```text
//! 1. start    server mints a secret, seals it, returns it once
//! 2. scan     the app takes the secret
//! 3. confirm  you produce a code from it, and only then is the factor usable
//! ```
//!
//! Step 3 is the whole point. Without it, a secret mistyped into an
//! authenticator app becomes a confirmed factor that produces codes the server
//! will never accept - and the next sign-in is a lockout with no way back.
//!
//! The secret is shown once. Closing the page mid-enrolment leaves an
//! unconfirmed row that satisfies nothing, and starting again is the correct
//! recovery.

use leptos::prelude::*;
use leptos_meta::Title;
use phonix_core::identity::{AuthUser, MfaEnforcement, MfaFactorSummary, MfaStatus, RecoveryCodes};

use crate::components::avatar::{Avatar, ProfilePicture, stored_picture};
use crate::components::page::{Badge, GhostButton, Notice, PageHeader, Panel, PrimaryButton, Tone};
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::server_fns::account_fns::{
    PasswordChangeResult, StartedEnrolment, change_my_password, confirm_totp, my_mfa_status,
    new_recovery_codes, remove_my_factor, start_totp_enrolment,
};
use crate::server_fns::auth_fns::current_user;
use crate::ui::tabs::{Tab, TabbedPanel};

#[component]
pub fn account_page() -> impl IntoView {
    let user = OnceResource::new(current_user());
    // Read once, here, and shared by both panels on the Profile tab: the
    // summary shows the picture and the panel beside it changes the picture,
    // and they must not disagree for the half-second a refetch would take.
    let picture = stored_picture();

    view! {
        // "Phonix" is the product's name, not a word.
        <Title text=format!("{} | Phonix", l!("account.title")) />

        <PageHeader
            title=l!("account.title")
            subtitle=l!("account.subtitle")
            icon=Icon::CircleUser
        />

        <TabbedPanel
            id="account"
            tabs=vec![
                Tab::new(
                        "profile",
                        l!("account.tab.profile"),
                        move || {
                            view! {
                                <Suspense fallback=|| {
                                    view! {
                                        <p class="text-sm text-content-subtle">
                                            {l!("common.loading")}
                                        </p>
                                    }
                                }>
                                    {move || Suspend::new(async move {
                                        user.await
                                            .ok()
                                            .flatten()
                                            .map(|user| {
                                                let initials = user.initials();
                                                view! {
                                                    // Side by side once there is room. Who
                                                    // you are and what you look like are one
                                                    // question, and stacking two short
                                                    // panels down a wide screen spends a
                                                    // screen's height on half a column.
                                                    <div class="grid gap-3 xl:grid-cols-2 xl:items-start">
                                                        <Profile user=user picture=picture />
                                                        <ProfilePicture
                                                            initials=initials
                                                            current=picture
                                                        />
                                                    </div>
                                                }
                                            })
                                    })}
                                </Suspense>
                            }
                                .into_any()
                        },
                    )
                    .icon(Icon::CircleUser),
                Tab::new(
                        "security",
                        l!("account.tab.security"),
                        || {
                            view! {
                                // Two panels, two independent saves - unlike the
                                // workspace settings screen, where one tab covers
                                // one settings object. What groups them here is
                                // that they are the same question: what it takes
                                // to sign in as you. Sessions and devices belong
                                // beside them when they arrive.
                                //
                                // Beside each other rather than stacked once
                                // there is room for both: neither is tall, and
                                // stacking them spends a screen's height on two
                                // panels that between them fill half a column.
                                // `items-start` so the shorter one keeps its
                                // own height instead of being stretched to
                                // match the enrolment flow mid-scan.
                                <div class="grid gap-3 xl:grid-cols-2 xl:items-start">
                                    <ChangePassword />
                                    <TwoFactor />
                                </div>
                            }
                                .into_any()
                        },
                    )
                    .icon(Icon::ShieldCheck),
            ]
        />
    }
}

/// Read-only for now: names and email are not editable until there is a use
/// case that writes them, and a form that silently discards what you typed is
/// worse than no form.
#[component]
fn profile(user: AuthUser, picture: RwSignal<Option<uuid::Uuid>>) -> impl IntoView {
    let initials = user.initials();

    view! {
        <Panel title=l!("account.tab.profile")>
            <div class="flex items-start gap-3">
                <Avatar initials=initials file_id=picture />
                <div class="min-w-0 space-y-1">
                    <div class="flex flex-wrap items-center gap-1.5">
                        <span class="font-medium text-content">{user.display_name.clone()}</span>
                        {user
                            .is_owner
                            .then(|| {
                                view! { <Badge label=l!("account.badge.owner") tone=Tone::Brand /> }
                            })}
                        {(!user.email_verified)
                            .then(|| {
                                view! {
                                    <Badge
                                        label=l!("account.badge.email_unverified")
                                        tone=Tone::Warning
                                    />
                                }
                            })}
                    </div>
                    <div class="text-sm text-content-muted">{user.email.clone()}</div>
                    <div class="flex flex-wrap gap-1 pt-1">
                        {user
                            .roles
                            .iter()
                            .map(|role| view! { <Badge label=role.clone() /> })
                            .collect::<Vec<_>>()}
                    </div>
                </div>
            </div>
        </Panel>
    }
}

#[component]
fn change_password() -> impl IntoView {
    let current = RwSignal::new(String::new());
    let next = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());

    let notice = RwSignal::new(None::<String>);
    let failed = RwSignal::new(None::<String>);
    let pending = RwSignal::new(false);

    let change = Action::new(move |(current, next): &(String, String)| {
        let (current, next) = (current.clone(), next.clone());
        async move { change_my_password(current, next).await }
    });

    Effect::new(move |_| {
        let Some(result) = change.value().get() else {
            return;
        };
        pending.set(false);

        match result {
            Ok(PasswordChangeResult::Changed) => {
                current.set(String::new());
                next.set(String::new());
                confirm.set(String::new());
                failed.set(None);
                notice.set(Some(
                    "Password changed. Your other sessions have been signed out.".to_owned(),
                ));
            }
            Ok(PasswordChangeResult::Rejected(errors)) => {
                notice.set(None);
                failed.set(Some(
                    errors
                        .iter()
                        .map(|error| crate::i18n::t(&error.message))
                        .collect::<Vec<_>>()
                        .join(" "),
                ));
            }
            Err(err) => {
                notice.set(None);
                failed.set(Some(err.to_string()));
            }
        }
    });

    view! {
        <Panel title=l!("account.password.title")>
            <form
                class="space-y-3"
                on:submit=move |event| {
                    event.prevent_default();

                    // Checked here as well as on the server because the server
                    // never sees the confirmation field - it exists to catch a
                    // typo, and only this side knows whether there was one.
                    if next.get_untracked() != confirm.get_untracked() {
                        notice.set(None);
                        failed.set(Some(l!("account.password.mismatch")));
                        return;
                    }

                    pending.set(true);
                    notice.set(None);
                    failed.set(None);
                    change.dispatch((current.get_untracked(), next.get_untracked()));
                }
            >
                <Notice message=Signal::derive(move || failed.get()) tone=Tone::Danger />
                <Notice message=Signal::derive(move || notice.get()) tone=Tone::Success />

                <PasswordField
                    id="current_password"
                    label=l!("field.current_password")
                    value=current
                    autocomplete="current-password"
                />
                <PasswordField
                    id="new_password"
                    label=l!("field.new_password")
                    value=next
                    autocomplete="new-password"
                />
                <PasswordField
                    id="confirm_password"
                    label=l!("field.confirm_new_password")
                    value=confirm
                    autocomplete="new-password"
                />

                <div class="flex justify-end">
                    <PrimaryButton
                        label=l!("account.password.submit")
                        icon=Icon::KeyRound
                        button_type="submit"
                        pending=Signal::derive(move || pending.get())
                    />
                </div>
            </form>
        </Panel>
    }
}

#[component]
fn password_field(
    #[prop(into)] id: String,
    #[prop(into)] label: String,
    value: RwSignal<String>,
    #[prop(into)] autocomplete: String,
) -> impl IntoView {
    let field_id = id.clone();

    view! {
        <div>
            <label for=field_id.clone() class="block text-sm font-medium text-content">
                {label}
            </label>
            <input
                id=field_id.clone()
                name=field_id
                type="password"
                autocomplete=autocomplete
                class="mt-1"
                prop:value=move || value.get()
                on:input=move |event| value.set(event_target_value(&event))
            />
        </div>
    }
}

// ---------------------------------------------------------------------------
// Two-factor authentication
// ---------------------------------------------------------------------------

#[component]
fn two_factor() -> impl IntoView {
    // Reloaded rather than patched after every change: enrolling, confirming
    // and removing all move several things at once - whether a factor exists,
    // how many recovery codes are left, whether the grace period still runs -
    // and re-deriving that in the browser is where the two would drift.
    let reload = RwSignal::new(0u32);
    let status = Resource::new(
        move || reload.get(),
        |_| async move { my_mfa_status().await },
    );

    let refresh = Callback::new(move |()| reload.update(|count| *count += 1));

    view! {
        <Panel title=l!("account.mfa.title") description=l!("account.mfa.subtitle")>
            <Suspense fallback=|| {
                view! { <p class="text-sm text-content-subtle">{l!("common.loading")}</p> }
            }>
                {move || Suspend::new(async move {
                    match status.await {
                        Ok(status) => {
                            view! { <TwoFactorBody status=status refresh=refresh /> }.into_any()
                        }
                        Err(err) => {
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
            </Suspense>
        </Panel>
    }
}

#[component]
fn two_factor_body(status: MfaStatus, refresh: Callback<()>) -> impl IntoView {
    let enrolling = RwSignal::new(None::<StartedEnrolment>);
    let codes = RwSignal::new(None::<RecoveryCodes>);
    let failed = RwSignal::new(None::<String>);

    let policy = status.policy.clone();
    let may_enrol = policy.allows_enrolment();
    let allow_recovery = policy.allow_recovery_codes;
    let enabled = status.enabled;
    let factors = status.factors.clone();
    let codes_remaining = status.recovery_codes_remaining;
    let enrolment_required = status.enrolment_required;
    let grace_days = status.grace_days_remaining;
    let enforcement = policy.enforcement;

    let start = Action::new(|(): &()| async move { start_totp_enrolment().await });

    Effect::new(move |_| {
        let Some(result) = start.value().get() else {
            return;
        };
        match result {
            Ok(started) => {
                failed.set(None);
                enrolling.set(Some(started));
            }
            Err(err) => failed.set(Some(err.to_string())),
        }
    });

    let issue_codes = Action::new(|(): &()| async move { new_recovery_codes().await });

    Effect::new(move |_| {
        let Some(result) = issue_codes.value().get() else {
            return;
        };
        match result {
            Ok(issued) => {
                failed.set(None);
                codes.set(Some(issued));
                refresh.run(());
            }
            Err(err) => failed.set(Some(err.to_string())),
        }
    });

    view! {
        <div class="space-y-3">
            <Notice message=Signal::derive(move || failed.get()) tone=Tone::Danger />

            <div class="flex flex-wrap items-center gap-2">
                {if enabled {
                    view! {
                        <Badge
                            label=l!("account.mfa.on")
                            tone=Tone::Success
                            icon=Icon::ShieldCheck
                        />
                    }
                        .into_any()
                } else {
                    view! {
                        <Badge label=l!("account.mfa.off") tone=Tone::Warning icon=Icon::ShieldOff />
                    }
                        .into_any()
                }}
                <span class="text-xs text-content-subtle">
                    {match enforcement {
                        MfaEnforcement::Required => l!("account.mfa.required"),
                        MfaEnforcement::Optional => l!("account.mfa.optional"),
                        MfaEnforcement::Disabled => l!("account.mfa.disabled"),
                    }}
                </span>
            </div>

            <Show when=move || enrolment_required fallback=|| ()>
                <div class="flex items-start gap-2 rounded-control border border-danger px-3 py-2 text-sm text-danger">
                    <span class="mt-0.5 shrink-0">
                        <Icon icon=Icon::TriangleAlert size=IconSize::Xs />
                    </span>
                    <span>{l!("account.mfa.must_enrol")}</span>
                </div>
            </Show>

            {grace_days
                .map(|days| {
                    view! {
                        <div class="flex items-center gap-2 text-xs text-content-muted">
                            <Icon icon=Icon::Clock size=IconSize::Xs />
                            // A plural pair, not a spliced "s": the word that
                            // changes is not the last one in every language.
                            {crate::lp!("account.mfa.grace", days)}
                        </div>
                    }
                })}

            // --- existing factors --------------------------------------------
            // Not a `Show`: this body is rebuilt from the resource whenever
            // `refresh` ticks, so the list is fixed for the life of one render
            // and a reactive wrapper would only borrow it a second time.
            {(!factors.is_empty())
                .then(|| {
                    view! {
                        <ul class="divide-y divide-edge rounded-control border border-edge">
                            {factors
                                .into_iter()
                                .map(|factor| view! { <FactorRow factor=factor refresh=refresh /> })
                                .collect::<Vec<_>>()}
                        </ul>
                    }
                })}

            // --- enrolment ---------------------------------------------------
            {move || {
                match enrolling.get() {
                    Some(started) => {
                        view! { <EnrolmentSteps started=started enrolling=enrolling refresh=refresh /> }
                            .into_any()
                    }
                    None if may_enrol => {
                        view! {
                            <PrimaryButton
                                label=if enabled {
                                    l!("account.mfa.add_another")
                                } else {
                                    l!("account.mfa.set_up")
                                }
                                icon=Icon::QrCode
                                pending=Signal::derive(move || start.pending().get())
                                on_click=Callback::new(move |()| {
                                    start.dispatch(());
                                })
                            />
                        }
                            .into_any()
                    }
                    None => {
                        view! {
                            <p class="text-sm text-content-subtle">
                                {l!("error.mfa.totp_not_allowed")}
                            </p>
                        }
                            .into_any()
                    }
                }
            }}

            // --- recovery codes ----------------------------------------------
            <Show when=move || allow_recovery && enabled fallback=|| ()>
                <div class="space-y-2 border-t border-edge pt-3">
                    <div class="flex flex-wrap items-center justify-between gap-2">
                        <div>
                            <div class="text-sm font-medium text-content">
                                {l!("account.mfa.recovery.title")}
                            </div>
                            <div class="text-xs text-content-subtle">
                                {crate::lp!(
                                    "account.mfa.recovery.remaining",
                                    i64::try_from(codes_remaining).unwrap_or(i64::MAX)
                                )}
                            </div>
                        </div>
                        <GhostButton
                            label=if codes_remaining == 0 {
                                l!("account.mfa.recovery.generate")
                            } else {
                                l!("account.mfa.recovery.regenerate")
                            }
                            icon=Icon::RefreshCw
                            on_click=Callback::new(move |()| {
                                issue_codes.dispatch(());
                            })
                        />
                    </div>

                    {move || codes.get().map(|issued| view! { <CodeList codes=issued /> })}
                </div>
            </Show>
        </div>
    }
}

#[component]
fn factor_row(factor: MfaFactorSummary, refresh: Callback<()>) -> impl IntoView {
    let factor_id = factor.id;
    let failed = RwSignal::new(None::<String>);

    let remove = Action::new(move |(): &()| async move { remove_my_factor(factor_id).await });

    Effect::new(move |_| {
        let Some(result) = remove.value().get() else {
            return;
        };
        match result {
            Ok(_) => {
                failed.set(None);
                refresh.run(());
            }
            Err(err) => failed.set(Some(err.to_string())),
        }
    });

    view! {
        <li class="flex flex-wrap items-center gap-2 px-3 py-2">
            <span class="shrink-0 text-content-muted">
                <Icon icon=Icon::Smartphone size=IconSize::Sm />
            </span>
            <div class="min-w-0 flex-1">
                <div class="flex flex-wrap items-center gap-1.5">
                    <span class="truncate-fade text-sm text-content">{factor.label.clone()}</span>
                    {(!factor.confirmed)
                        .then(|| {
                            view! {
                                <Badge
                                    label=l!("account.mfa.factor.unconfirmed")
                                    tone=Tone::Warning
                                />
                            }
                        })}
                </div>
                <div class="text-2xs text-content-subtle">
                    // The date goes through the sentence rather than after it:
                    // German puts the preposition in front of it.
                    {l!(
                        "account.mfa.factor.added",
                        date = factor.created_at.format("%Y-%m-%d").to_string()
                    )}
                    {factor
                        .last_used_at
                        .map(|at| {
                            format!(
                                " · {}",
                                l!(
                                    "account.mfa.factor.last_used",
                                    date = at.format("%Y-%m-%d").to_string()
                                ),
                            )
                        })
                        .unwrap_or_default()}
                </div>
                {move || {
                    failed.get().map(|message| view! { <p class="text-xs text-danger">{message}</p> })
                }}
            </div>
            <GhostButton
                label=l!("common.remove")
                icon=Icon::Trash2
                disabled=Signal::derive(move || remove.pending().get())
                on_click=Callback::new(move |()| {
                    remove.dispatch(());
                })
            />
        </li>
    }
}

/// Scan, then prove it worked.
#[component]
fn enrolment_steps(
    started: StartedEnrolment,
    enrolling: RwSignal<Option<StartedEnrolment>>,
    refresh: Callback<()>,
) -> impl IntoView {
    let factor_id = started.factor_id;
    let secret = started.secret_base32.clone();
    let qr = started.qr_svg.clone();
    let digits = started.digits;
    let period = started.period_secs;

    let code = RwSignal::new(String::new());
    let failed = RwSignal::new(None::<String>);

    let confirm = Action::new(move |code: &String| {
        let code = code.clone();
        async move { confirm_totp(factor_id, code).await }
    });

    Effect::new(move |_| {
        let Some(result) = confirm.value().get() else {
            return;
        };
        match result {
            Ok(true) => {
                failed.set(None);
                code.set(String::new());
                enrolling.set(None);
                refresh.run(());
            }
            Ok(false) => {
                // A wrong code leaves the row unconfirmed, so the same QR is
                // still valid and they can simply try again.
                failed.set(Some(l!("account.mfa.enrol.wrong_code")));
            }
            Err(err) => failed.set(Some(err.to_string())),
        }
    });

    view! {
        <div class="space-y-3 rounded-control border border-edge bg-surface-sunken p-3">
            <div>
                <div class="text-sm font-medium text-content">
                    {l!("account.mfa.enrol.scan")}
                </div>
                <p class="text-xs text-content-subtle">{l!("account.mfa.enrol.scan_hint")}</p>
            </div>

            <div class="flex flex-wrap items-start gap-3">
                // The SVG comes from the server, built from the provisioning
                // URI. Nothing the caller supplied reaches it.
                <div class="rounded-control bg-white p-2" inner_html=qr></div>

                <div class="min-w-0 space-y-1">
                    <div class="text-xs text-content-subtle">
                        {l!("account.mfa.enrol.manual")}
                    </div>
                    <code class="block break-all rounded bg-surface px-2 py-1 font-mono text-xs text-content">
                        {secret}
                    </code>
                    <div class="text-2xs text-content-subtle">
                        {l!("account.mfa.enrol.parameters", digits = digits, period = period)}
                    </div>
                </div>
            </div>

            <div class="border-t border-edge pt-3">
                <div class="text-sm font-medium text-content">
                    {l!("account.mfa.enrol.prove")}
                </div>
                <p class="text-xs text-content-subtle">{l!("account.mfa.enrol.prove_hint")}</p>

                <form
                    class="mt-2 flex flex-wrap items-start gap-2"
                    on:submit=move |event| {
                        event.prevent_default();
                        confirm.dispatch(code.get_untracked());
                    }
                >
                    <input
                        type="text"
                        inputmode="numeric"
                        autocomplete="one-time-code"
                        maxlength="10"
                        aria-label=l!("account.mfa.enrol.code_label")
                        placeholder="000000"
                        class="h-8 w-32 py-0 text-center font-mono tracking-widest"
                        prop:value=move || code.get()
                        on:input=move |event| code.set(event_target_value(&event))
                    />
                    <PrimaryButton
                        label=l!("common.confirm")
                        icon=Icon::Check
                        button_type="submit"
                        pending=Signal::derive(move || confirm.pending().get())
                    />
                    <GhostButton
                        label=l!("common.cancel")
                        on_click=Callback::new(move |()| {
                            enrolling.set(None);
                            refresh.run(());
                        })
                    />
                </form>

                {move || {
                    failed.get().map(|message| view! { <p class="mt-2 text-sm text-danger">{message}</p> })
                }}
            </div>
        </div>
    }
}

/// Freshly issued recovery codes, shown once.
#[component]
fn code_list(codes: RecoveryCodes) -> impl IntoView {
    let list = codes.codes.clone();

    view! {
        <div class="space-y-2 rounded-control border border-warning bg-surface-sunken p-3">
            <div class="flex items-start gap-2 text-sm text-content">
                <span class="mt-0.5 shrink-0 text-warning">
                    <Icon icon=Icon::TriangleAlert size=IconSize::Xs />
                </span>
                <span>
                    "Copy these somewhere safe now. They are not shown again, and any codes you had \
                     before no longer work."
                </span>
            </div>

            <ul class="grid grid-cols-2 gap-1 font-mono text-sm sm:grid-cols-3">
                {list
                    .iter()
                    .map(|code| {
                        view! {
                            <li class="rounded bg-surface px-2 py-1 text-center text-content">
                                {code.clone()}
                            </li>
                        }
                    })
                    .collect::<Vec<_>>()}
            </ul>
        </div>
    }
}
