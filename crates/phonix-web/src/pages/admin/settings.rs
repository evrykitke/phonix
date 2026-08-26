//! Workspace settings: what this workspace requires of people, and how it
//! reaches them.
//!
//! The security half is one form, two policies, one save. They are validated
//! together in [`phonix_core::WorkspaceSecuritySettings::validate`] and stored
//! in one row, so splitting them into two forms would produce two round trips
//! to fix two mistakes an administrator made at the same moment.
//!
//! # Two tabs, by who answers them
//!
//! **Security** holds both policies, in one form under one save. They were once
//! a tab each, which split a single settings object across two `<form>`
//! elements and two submits that each sent the whole thing anyway - the tab
//! strip was drawing a line the data does not have. What earns a tab here is a
//! different *owner* of the answer, not a different subheading.
//!
//! **Communication** is where this workspace is reached from: the mail relay
//! today, and the other channels beside it later. It is a separate tab because
//! it is a separate row, loaded separately and saved separately - one save that
//! half-succeeds is the thing being avoided.
//!
//! The edit buffer is still a set of signals declared in [`settings_form`]
//! rather than read out of the DOM. Only the active tab is rendered, so a
//! buffer that lived in the inputs would lose everything on the way to
//! Communication and back.
//!
//! # Nothing here is a deployment concern
//!
//! Argon2 cost, TOTP digits and the session ceiling are not on this screen and
//! never will be. Those live in `[security]` in the config file, because they
//! depend on the hardware and on decisions an organization is not in a position
//! to make. This screen is *how strict*, not *how expensive*.

use leptos::prelude::*;
use leptos_meta::Title;
use phonix_core::WorkspaceSecuritySettings;
use phonix_core::audit::{AuditPolicy, ENTITY_KINDS, EntityKind, kinds};
use phonix_core::identity::{
    FieldError, MAX_PASSWORD_LEN, MfaEnforcement, MfaPolicy, PasswordPolicy,
};

use crate::components::history::RecordHistory;
use crate::components::page::{
    FormActions, GhostButton, Notice, PageHeader, PrimaryButton, Tone,
};
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::pages::admin::currencies::CurrenciesTab;
use crate::pages::admin::mail_settings::MailSettingsTab;
use crate::pages::admin::numbering::NumberingTab;
use crate::pages::admin::organization::OrganizationTab;
use crate::server_fns::settings_fns::{SettingsSaved, save_workspace_settings, workspace_settings};
use crate::ui::card::CollapsibleCard;
use crate::ui::tabs::{Tab, TabbedPanel};

#[component]
pub fn settings_page() -> impl IntoView {
    let loaded = OnceResource::new(workspace_settings());

    view! {
        // "Phonix" is the product's name, not a word.
        <Title text=format!("{} | Phonix", l!("settings.title")) />

        <PageHeader
            title=l!("settings.title")
            subtitle=l!("settings.subtitle")
            icon=Icon::Settings
        />

        <Suspense fallback=|| {
            view! { <p class="text-sm text-content-subtle">{l!("common.loading")}</p> }
        }>
            {move || Suspend::new(async move {
                match loaded.await {
                    Ok(settings) => view! { <SettingsForm initial=settings /> }.into_any(),
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
    }
}

/// Which of the three cards on the Security tab a rejected field belongs to.
///
/// The three policies validate independently and their field names do not
/// overlap, so this is a match on the name rather than a table to keep in
/// step. It is *total* on purpose: a card arrives collapsed, and a count on
/// the wrong card costs one click, while a field name no arm claimed would be
/// a save that appears to have done nothing at all.
fn card_of(field: &str) -> SecurityCard {
    match field {
        "allow_totp" | "grace_period_days" | "remember_device_days" => SecurityCard::Mfa,
        // The audit policy prefixes its fields, so it needs no list.
        name if name.starts_with("audit_") => SecurityCard::Audit,
        _ => SecurityCard::Password,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SecurityCard {
    Password,
    Mfa,
    Audit,
}

/// The form itself, seeded from what the server returned.
///
/// Separated from the page so it is constructed *after* the settings have
/// loaded: the signals below are the edit buffer, and one created before the
/// data arrived would start at the system default and quietly offer to save it.
#[component]
fn settings_form(initial: WorkspaceSecuritySettings) -> impl IntoView {
    // --- password ---------------------------------------------------------
    let min_length = RwSignal::new(initial.password.min_length);
    let require_lowercase = RwSignal::new(initial.password.require_lowercase);
    let require_uppercase = RwSignal::new(initial.password.require_uppercase);
    let require_digit = RwSignal::new(initial.password.require_digit);
    let require_symbol = RwSignal::new(initial.password.require_symbol);
    let forbid_common = RwSignal::new(initial.password.forbid_common);
    let forbid_personal = RwSignal::new(initial.password.forbid_personal_information);
    let expiry_days = RwSignal::new(initial.password.expiry_days.unwrap_or(0));
    let history_depth = RwSignal::new(initial.password.history_depth);

    // --- mfa --------------------------------------------------------------
    let enforcement = RwSignal::new(initial.mfa.enforcement);
    let allow_totp = RwSignal::new(initial.mfa.allow_totp);
    let allow_recovery = RwSignal::new(initial.mfa.allow_recovery_codes);
    let grace_days = RwSignal::new(initial.mfa.grace_period_days);
    let remember_days = RwSignal::new(initial.mfa.remember_device_days);

    // --- auditing ---------------------------------------------------------
    // The whole policy in one signal rather than a bool per kind, because the
    // kinds are a list this file does not own: `ENTITY_KINDS` grows, and a
    // signal per kind would mean editing this screen every time it does.
    let audit = RwSignal::new(initial.audit.clone());
    // Zero is "keep for ever" on the form, matching how password expiry reads
    // here. `None` is what the policy means by it.
    let retention_days = RwSignal::new(initial.audit.retention_days.unwrap_or(0));

    let errors = RwSignal::new(Vec::<FieldError>::new());
    let notice = RwSignal::new(None::<String>);
    let failed = RwSignal::new(None::<String>);
    let pending = RwSignal::new(false);

    let max_length = initial.password.max_length;

    let collect = move || WorkspaceSecuritySettings {
        password: PasswordPolicy {
            min_length: min_length.get(),
            max_length,
            require_lowercase: require_lowercase.get(),
            require_uppercase: require_uppercase.get(),
            require_digit: require_digit.get(),
            require_symbol: require_symbol.get(),
            forbid_common: forbid_common.get(),
            forbid_personal_information: forbid_personal.get(),
            // Zero is "never expire" on the form, because a checkbox plus a
            // number is two controls for one decision. `None` is what the
            // policy means by it.
            expiry_days: Some(expiry_days.get()).filter(|days| *days > 0),
            history_depth: history_depth.get(),
        },
        mfa: MfaPolicy {
            enforcement: enforcement.get(),
            allow_totp: allow_totp.get(),
            allow_recovery_codes: allow_recovery.get(),
            grace_period_days: grace_days.get(),
            remember_device_days: remember_days.get(),
        },
        audit: AuditPolicy {
            retention_days: Some(retention_days.get()).filter(|days| *days > 0),
            ..audit.get()
        },
    };

    let save = Action::new(move |settings: &WorkspaceSecuritySettings| {
        let settings = settings.clone();
        async move { save_workspace_settings(settings).await }
    });

    Effect::new(move |_| {
        let Some(result) = save.value().get() else {
            return;
        };
        pending.set(false);

        match result {
            Ok(SettingsSaved::Saved(_)) => {
                errors.set(Vec::new());
                failed.set(None);
                notice.set(Some(l!("settings.saved")));
            }
            Ok(SettingsSaved::Rejected(rejected)) => {
                notice.set(None);
                // The sentence names the counts rather than the fields: the
                // cards do that, and they are the thing to look at next.
                failed.set(Some(l!("settings.rejected")));
                errors.set(rejected);
            }
            Err(err) => {
                notice.set(None);
                errors.set(Vec::new());
                failed.set(Some(err.to_string()));
            }
        }
    });

    let error_for = move |field: &'static str| {
        Signal::derive(move || {
            errors.with(|errors| {
                errors
                    .iter()
                    .find(|error| error.field == field)
                    .map(|error| crate::i18n::t(&error.message))
            })
        })
    };

    // A collapsed card still submits everything inside it, and still draws
    // the errors that come back - where nobody can see them. This is what the
    // header shows instead, so a rejected save names the card to open rather
    // than leaving three shut boxes and a red sentence at the top.
    let problems_on = move |card: SecurityCard| {
        Signal::derive(move || {
            let count = errors
                .with(|errors| errors.iter().filter(|error| card_of(&error.field) == card).count());
            u32::try_from(count).unwrap_or(u32::MAX)
        })
    };

    // Rendered inside the Security tab rather than once below the strip,
    // because the Communication tab contains an `EntityForm` - a `<form>` of
    // its own, and a form inside a form is invalid markup whose inner submit
    // fires the outer one. So the `<form>` element starts inside the tab.
    //
    // A closure rather than a value: the tab's render is an `Fn`, called again
    // on every switch back, and a view is consumed by rendering it once.
    let actions = move || {
        view! {
                <div class="rounded-card border border-edge bg-surface-raised">
                    <FormActions>
                        <GhostButton
                            label=l!("settings.reset")
                            icon=Icon::RefreshCw
                            on_click=Callback::new(move |()| {
                                let defaults = WorkspaceSecuritySettings::system_default();
                                min_length.set(defaults.password.min_length);
                                require_lowercase.set(defaults.password.require_lowercase);
                                require_uppercase.set(defaults.password.require_uppercase);
                                require_digit.set(defaults.password.require_digit);
                                require_symbol.set(defaults.password.require_symbol);
                                forbid_common.set(defaults.password.forbid_common);
                                forbid_personal.set(defaults.password.forbid_personal_information);
                                expiry_days.set(defaults.password.expiry_days.unwrap_or(0));
                                history_depth.set(defaults.password.history_depth);
                                enforcement.set(defaults.mfa.enforcement);
                                allow_totp.set(defaults.mfa.allow_totp);
                                allow_recovery.set(defaults.mfa.allow_recovery_codes);
                                grace_days.set(defaults.mfa.grace_period_days);
                                remember_days.set(defaults.mfa.remember_device_days);
                                audit.set(defaults.audit.clone());
                                retention_days.set(defaults.audit.retention_days.unwrap_or(0));
                                // Not saved: this fills the form in, and the
                                // administrator still has to mean it.
                                notice.set(Some(l!("settings.reset_done")));
                            })
                        />
                        <PrimaryButton
                            label=l!("settings.save")
                            icon=Icon::Save
                            button_type="submit"
                            pending=Signal::derive(move || pending.get())
                        />
                    </FormActions>
                </div>
        }
    };

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        pending.set(true);
        notice.set(None);
        save.dispatch(collect());
    };

    view! {
        <div class="space-y-4">
            <Notice message=Signal::derive(move || failed.get()) tone=Tone::Danger />
            <Notice message=Signal::derive(move || notice.get()) tone=Tone::Success />
            <TabbedPanel
                id="settings"
                tabs=vec![
                    // First, because it is the tab a new workspace has to fill
                    // in: the security policy and the relay both have working
                    // defaults, and this one starts empty.
                    Tab::new(
                            "organization",
                            l!("settings.tab.organization"),
                            || view! { <OrganizationTab /> }.into_any(),
                        )
                        .icon(Icon::Building2),
                    Tab::new(
                            "security",
                            l!("settings.tab.security"),
                            move || {
                                view! {
                                <form class="space-y-4" on:submit=submit>
                                // Stacked, not side by side. The two policies
                                // shared a row while they were open panels,
                                // because two full-height forms down a wide
                                // screen was a scroll spent on space that was
                                // already there. Collapsing answers that far
                                // better, and a column pairs badly with it: one
                                // card open beside one card shut is a page of
                                // whitespace with a heading floating in it.
                                //
                                // Width costs nothing here because it is not
                                // spent - every input in the base layer carries
                                // its own ceiling, so a full-width card holds
                                // the same 26rem field it held at half the
                                // width. See `--container-measure`.
                                <CollapsibleCard
                                    title=l!("settings.password.title")
                                    detail=l!("settings.password.description")
                                    icon=Icon::KeyRound
                                    problems=problems_on(SecurityCard::Password)
                                >
                                    <div class="space-y-4">
                                        <NumberField
                                            id="min_length"
                                            label=l!("settings.password.min_length")
                                            hint=l!(
                                                "settings.password.min_length_hint",
                                                max = MAX_PASSWORD_LEN
                                            )
                                            value=min_length
                                            error=error_for("min_length")
                                        />

                                        <fieldset class="space-y-1.5">
                                            <legend class="text-sm font-medium text-content">
                                                {l!("settings.password.required_characters")}
                                            </legend>
                                            <p class="text-xs text-content-subtle">
                                                {l!("settings.password.composition_note")}
                                            </p>
                                            <div class="grid gap-1 sm:grid-cols-2">
                                                <Toggle
                                                    label=l!("settings.password.lowercase")
                                                    value=require_lowercase
                                                />
                                                <Toggle
                                                    label=l!("settings.password.uppercase")
                                                    value=require_uppercase
                                                />
                                                <Toggle
                                                    label=l!("settings.password.digit")
                                                    value=require_digit
                                                />
                                                <Toggle
                                                    label=l!("settings.password.symbol")
                                                    value=require_symbol
                                                />
                                            </div>
                                        </fieldset>

                                        <div class="space-y-1">
                                            <Toggle
                                                label=l!("settings.password.forbid_common")
                                                detail=l!("settings.password.forbid_common_detail")
                                                value=forbid_common
                                            />
                                            <Toggle
                                                label=l!("settings.password.forbid_personal")
                                                detail=l!(
                                                    "settings.password.forbid_personal_detail"
                                                )
                                                value=forbid_personal
                                            />
                                        </div>

                                        <div class="grid gap-4 sm:grid-cols-2">
                                            <NumberField
                                                id="expiry_days"
                                                label=l!("settings.password.expiry")
                                                hint=l!("settings.password.expiry_hint")
                                                value=expiry_days
                                                error=error_for("expiry_days")
                                            />
                                            <NumberField
                                                id="history_depth"
                                                label=l!("settings.password.history")
                                                hint=l!("settings.password.history_hint")
                                                value=history_depth
                                                error=error_for("history_depth")
                                            />
                                        </div>

                                        <p class="text-xs text-content-subtle">
                                            {l!("settings.password.expiry_note")}
                                        </p>
                                    </div>
                                </CollapsibleCard>

                                <CollapsibleCard
                                    title=l!("account.mfa.title")
                                    detail=l!("settings.mfa.description")
                                    icon=Icon::Smartphone
                                    problems=problems_on(SecurityCard::Mfa)
                                >
                                    <div class="space-y-4">
                                        <fieldset class="space-y-1.5">
                                            <legend class="text-sm font-medium text-content">
                                                {l!("settings.mfa.enforcement")}
                                            </legend>
                                            <div class="space-y-1">
                                                {[MfaEnforcement::Disabled, MfaEnforcement::Optional,
                                                  MfaEnforcement::Required]
                                                    .into_iter()
                                                    .map(|option| {
                                                        view! { <EnforcementOption option=option selected=enforcement /> }
                                                    })
                                                    .collect::<Vec<_>>()}
                                            </div>
                                        </fieldset>

                                        <div class="space-y-1">
                                            <Toggle
                                                label=l!("settings.mfa.allow_totp")
                                                detail=l!("settings.mfa.allow_totp_detail")
                                                value=allow_totp
                                            />
                                            <Toggle
                                                label=l!("account.mfa.recovery.title")
                                                detail=l!("settings.mfa.allow_recovery_detail")
                                                value=allow_recovery
                                            />
                                        </div>

                                        {move || {
                                            error_for("allow_totp")
                                                .get()
                                                .map(|message| {
                                                    view! { <p class="text-sm text-danger">{message}</p> }
                                                })
                                        }}

                                        <div class="grid gap-4 sm:grid-cols-2">
                                            <NumberField
                                                id="grace_period_days"
                                                label=l!("settings.mfa.grace")
                                                hint=l!("settings.mfa.grace_hint")
                                                value=grace_days
                                                error=error_for("grace_period_days")
                                            />
                                            <NumberField
                                                id="remember_device_days"
                                                label=l!("settings.mfa.remember")
                                                hint=l!("settings.mfa.remember_hint")
                                                value=remember_days
                                                error=error_for("remember_device_days")
                                            />
                                        </div>

                                        <div class="flex items-start gap-2 rounded-control border border-edge bg-surface-sunken px-3 py-2 text-xs text-content-muted">
                                            <span class="mt-0.5 shrink-0 text-warning">
                                                <Icon icon=Icon::TriangleAlert size=IconSize::Xs />
                                            </span>
                                            <span>{l!("settings.mfa.lockout_warning")}</span>
                                        </div>
                                    </div>
                                </CollapsibleCard>

                                <CollapsibleCard
                                    title=l!("settings.audit.title")
                                    detail=l!("settings.audit.description")
                                    icon=Icon::ScrollText
                                    problems=problems_on(SecurityCard::Audit)
                                >
                                    <AuditingPanel
                                        policy=audit
                                        retention_days=retention_days
                                        error=error_for("audit_retention_days")
                                    />
                                </CollapsibleCard>

                                {actions()}
                                </form>

                                // Outside the form, not inside it. A history is
                                // not a field, and anything inside a `<form>` is
                                // something a stray Enter can submit.
                                <div class="mt-4">
                                <RecordHistory
                                    kind=kinds::SECURITY_POLICY
                                    id=Some(kinds::SECURITY_POLICY.singleton_id().to_owned())
                                />
                                </div>
                                }
                                    .into_any()
                            },
                        )
                        .icon(Icon::ShieldCheck),
                    Tab::new(
                            "communication",
                            l!("settings.tab.communication"),
                            || view! { <MailSettingsTab /> }.into_any(),
                        )
                        .icon(Icon::Mail),
                    // Last two, because they are the ones a workspace touches
                    // once and then leaves: the currency list starts correct
                    // from the organization profile, and a number series
                    // arrives already configured from the app that issues it.
                    Tab::new(
                            "currencies",
                            l!("settings.tab.currencies"),
                            || view! { <CurrenciesTab /> }.into_any(),
                        )
                        .icon(Icon::Boxes),
                    Tab::new(
                            "numbering",
                            l!("settings.tab.numbering"),
                            || view! { <NumberingTab /> }.into_any(),
                        )
                        .icon(Icon::FileText),
                ]
            />
        </div>
    }
}

/// One enforcement level, with the sentence that explains it.
#[component]
fn enforcement_option(option: MfaEnforcement, selected: RwSignal<MfaEnforcement>) -> impl IntoView {
    view! {
        <button
            type="button"
            class=move || {
                let state = if selected.get() == option {
                    "border-brand bg-brand-subtle"
                } else {
                    "border-edge hover:bg-surface-hover"
                };
                format!("flex w-full items-start gap-2 rounded-control border px-3 py-2 text-left {state}")
            }
            aria-pressed=move || if selected.get() == option { "true" } else { "false" }
            on:click=move |_| selected.set(option)
        >
            <span class=move || {
                let state = if selected.get() == option {
                    "border-brand bg-brand"
                } else {
                    "border-edge-strong"
                };
                format!("mt-1 size-3 shrink-0 rounded-full border-4 {state}")
            } />
            <span>
                // `name`, not `as_str`: the latter is the value in the column.
                // No `capitalize` either - a name that needs a CSS rule to look
                // like a word is not a name, and the rule is wrong in any
                // language that does not capitalise the way English does.
                <span class="block text-sm font-medium text-content">
                    {crate::i18n::t(&option.name())}
                </span>
                <span class="block text-xs text-content-subtle">
                    {crate::i18n::t(&option.description())}
                </span>
            </span>
        </button>
    }
}

/// What is recorded, and for how long.
///
/// Separate from the password and MFA panels because it answers a different
/// question - not "what do we require of people" but "what do we keep about
/// ourselves" - and because it is the one panel on this screen whose default
/// nobody should change without a reason.
#[component]
fn auditing_panel(
    policy: RwSignal<AuditPolicy>,
    retention_days: RwSignal<i32>,
    #[prop(into)] error: Signal<Option<String>>,
) -> impl IntoView {
    // Read from the signal rather than from the master checkbox, so the
    // sentence is the same one `AuditPolicy::summary` would print anywhere
    // else - including in the change trail, where this panel's own edits land.
    let summary = move || {
        let policy = AuditPolicy {
            retention_days: Some(retention_days.get()).filter(|d| *d > 0),
            ..policy.get()
        };

        crate::i18n::t(&policy.summary())
    };

    let enabled = move || policy.get().enabled;

    view! {
        <div class="space-y-4">
            <button
                type="button"
                class="flex w-full items-start gap-2 rounded-control px-1 py-1 text-left hover:bg-surface-hover"
                aria-pressed=move || if enabled() { "true" } else { "false" }
                on:click=move |_| {
                    policy.update(|policy| policy.enabled = !policy.enabled);
                }
            >
                <span class=move || {
                    let state = if enabled() {
                        "border-brand bg-brand text-on-brand"
                    } else {
                        "border-edge-strong"
                    };
                    format!("mt-0.5 grid size-4 shrink-0 place-items-center rounded border {state}")
                }>
                    {move || enabled().then(|| view! { <Icon icon=Icon::Check size=IconSize::Xs /> })}
                </span>
                <span class="min-w-0">
                    <span class="block text-sm text-content">
                        {l!("settings.audit.enabled")}
                    </span>
                    <span class="block text-xs text-content-subtle">{summary}</span>
                </span>
            </button>

            // Only when the master switch is on. Rendering the list greyed out
            // would be five controls that look editable and do nothing - and
            // the checkbox above already says what state it is in.
            <Show when=enabled fallback=|| {
                view! {
                    <p class="rounded-control border border-edge bg-surface-sunken px-3 py-2 text-xs text-content-muted">
                        {l!("settings.audit.off_note")}
                    </p>
                }
            }>
                <div>
                    <p class="text-sm font-medium text-content">
                        {l!("settings.audit.which")}
                    </p>
                    <p class="text-xs text-content-subtle">{l!("settings.audit.which_hint")}</p>

                    <div class="mt-2 grid gap-1 sm:grid-cols-2">
                        {ENTITY_KINDS
                            .iter()
                            .map(|kind| view! { <KindToggle kind=*kind policy=policy /> })
                            .collect::<Vec<_>>()}
                    </div>
                </div>

                <div class="max-w-xs">
                    <NumberField
                        id="audit_retention_days"
                        label=l!("settings.audit.retention")
                        hint=l!("settings.audit.retention_hint")
                        value=retention_days
                        error=error
                    />
                    <p class="mt-1 text-xs text-content-subtle">
                        {l!("settings.audit.retention_note")}
                    </p>
                </div>
            </Show>
        </div>
    }
}

/// One kind of record, on or off.
///
/// Reads and writes through [`AuditPolicy::with_kind`] rather than holding its
/// own bool, so the stored shape - a list of *exclusions* - is the only place
/// the answer lives. A local mirror would drift the moment the policy was
/// replaced by "reset to defaults".
#[component]
fn kind_toggle(kind: EntityKind, policy: RwSignal<AuditPolicy>) -> impl IntoView {
    let recorded = move || policy.get().records(kind);

    view! {
        <button
            type="button"
            class="flex w-full items-center gap-2 rounded-control px-1 py-1 text-left hover:bg-surface-hover"
            aria-pressed=move || if recorded() { "true" } else { "false" }
            on:click=move |_| {
                policy.update(|current| *current = current.clone().with_kind(kind, !current.records(kind)));
            }
        >
            <span class=move || {
                let state = if recorded() {
                    "border-brand bg-brand text-on-brand"
                } else {
                    "border-edge-strong"
                };
                format!("grid size-4 shrink-0 place-items-center rounded border {state}")
            }>
                {move || recorded().then(|| view! { <Icon icon=Icon::Check size=IconSize::Xs /> })}
            </span>
            <span class="text-sm text-content">{crate::i18n::t(&kind.plural())}</span>
        </button>
    }
}

/// A labelled checkbox that reads as a sentence.
#[component]
fn toggle(
    #[prop(into)] label: String,
    #[prop(optional, into)] detail: Option<String>,
    value: RwSignal<bool>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class="flex w-full items-start gap-2 rounded-control px-1 py-1 text-left hover:bg-surface-hover"
            aria-pressed=move || if value.get() { "true" } else { "false" }
            on:click=move |_| value.update(|value| *value = !*value)
        >
            <span class=move || {
                let state = if value.get() {
                    "border-brand bg-brand text-on-brand"
                } else {
                    "border-edge-strong"
                };
                format!(
                    "mt-0.5 grid size-4 shrink-0 place-items-center rounded border {state}",
                )
            }>
                {move || {
                    value
                        .get()
                        .then(|| view! { <Icon icon=Icon::Check size=IconSize::Xs /> })
                }}
            </span>
            <span class="min-w-0">
                <span class="block text-sm text-content">{label}</span>
                {detail
                    .map(|detail| {
                        view! { <span class="block text-xs text-content-subtle">{detail}</span> }
                    })}
            </span>
        </button>
    }
}

/// A whole-number field bound to a numeric signal.
///
/// Generic over the integer type so the same control serves `usize`, `u32` and
/// `u8` without three near-identical copies. Text that will not parse leaves
/// the signal alone rather than resetting it to zero, so backspacing to an
/// empty box does not silently mean "never expire".
#[component]
fn number_field<T>(
    #[prop(into)] id: String,
    #[prop(into)] label: String,
    #[prop(optional, into)] hint: Option<String>,
    value: RwSignal<T>,
    #[prop(optional, into)] error: Option<Signal<Option<String>>>,
) -> impl IntoView
where
    T: Copy + std::fmt::Display + std::str::FromStr + Send + Sync + 'static,
{
    let input_id = id.clone();

    view! {
        <div>
            <label for=input_id.clone() class="flex items-baseline justify-between text-sm font-medium text-content">
                <span>{label}</span>
                {hint
                    .map(|hint| {
                        view! {
                            <span class="text-xs font-normal text-content-subtle">{hint}</span>
                        }
                    })}
            </label>
            <input
                id=input_id.clone()
                name=input_id
                type="number"
                inputmode="numeric"
                min="0"
                class=move || {
                    let base = "mt-1";
                    let border = if error.is_some_and(|error| error.get().is_some()) {
                        "border-danger"
                    } else {
                        ""
                    };
                    format!("{base} {border}")
                }
                prop:value=move || value.get().to_string()
                on:input=move |event| {
                    if let Ok(parsed) = event_target_value(&event).trim().parse::<T>() {
                        value.set(parsed);
                    }
                }
            />
            {move || {
                error
                    .and_then(|error| error.get())
                    .map(|message| view! { <p class="mt-1 text-sm text-danger">{message}</p> })
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rejected_field_lands_on_the_card_that_draws_it() {
        for field in ["min_length", "max_length", "expiry_days", "history_depth"] {
            assert_eq!(card_of(field), SecurityCard::Password, "{field}");
        }
        for field in ["allow_totp", "grace_period_days", "remember_device_days"] {
            assert_eq!(card_of(field), SecurityCard::Mfa, "{field}");
        }
        assert_eq!(card_of("audit_retention_days"), SecurityCard::Audit);
    }

    #[test]
    fn a_save_that_breaks_all_three_policies_marks_all_three_cards() {
        // The list above is names copied out of three validators, and a copy
        // goes stale. This runs the validators instead: break one thing in
        // each policy, and every card had better light up. A field renamed on
        // the other side of the crate fails here rather than in a collapsed
        // card that nobody opens.
        let broken = WorkspaceSecuritySettings {
            password: PasswordPolicy {
                min_length: 4,
                ..PasswordPolicy::system_default()
            },
            mfa: MfaPolicy {
                enforcement: MfaEnforcement::Required,
                allow_totp: false,
                allow_recovery_codes: false,
                ..MfaPolicy::system_default()
            },
            audit: AuditPolicy {
                // Below MIN_RETENTION_DAYS, and not zero - zero is "for ever".
                retention_days: Some(1),
                ..AuditPolicy::system_default()
            },
        };

        let Err(errors) = broken.validate() else {
            panic!("these settings are meant to be rejected");
        };

        for card in [SecurityCard::Password, SecurityCard::Mfa, SecurityCard::Audit] {
            assert!(
                errors.iter().any(|error| card_of(&error.field) == card),
                "nothing counted on {card:?}, from {:?}",
                errors.iter().map(|error| &error.field).collect::<Vec<_>>(),
            );
        }
    }
}
