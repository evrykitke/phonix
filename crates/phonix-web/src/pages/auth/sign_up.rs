//! The signup wizard.
//!
//! Three screens, **one request**. The wizard collects an account on step one,
//! a workspace on step two, and submits both together - a catalog row with no
//! owner, or an owner with no database, is a state nobody wants to reason about
//! later. Step three is the provisioning screen, which is doing real work:
//! creating a database and running its migrations takes a moment, and the
//! animation is there because that moment is honest, not to pad it.
//!
//! Validation runs twice by design. The client's copy is
//! `phonix_core::identity::SignupInput::validate` compiled to WebAssembly, so
//! the messages are the same ones the server will produce; the server runs it
//! again because this endpoint is reachable without a browser.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::A;
use phonix_core::identity::{FieldError, SignupInput, SignupResult, slug_from_organization_name};

use crate::components::forms::{
    FieldLabel, FormError, SecondaryButton, StrengthMeter, SubmitButton, TextInput,
};
use crate::l;
use crate::server_fns::onboarding_fns::{check_workspace_address, create_workspace};
use crate::server_fns::tenant_fns::current_tenant;

/// Which screen the wizard is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Account,
    Workspace,
    Provisioning,
}

/// The two fields the workspace screen actually renders.
const WORKSPACE_FIELDS: [&str; 2] = ["organization_name", "workspace_slug"];

/// Errors the workspace screen has no field to attach to.
///
/// `validate()` judges the whole form, but the last screen shows two of its
/// seven fields - so a rejection can easily be about a password or an email
/// that is nowhere on the page. Binding those to `error_for` puts them in a
/// DOM node that does not exist, and the submit button appears to do nothing.
/// They are collected here and said at form level instead, where there is
/// somewhere to say them.
fn unshowable_here(errors: &[FieldError]) -> Vec<&FieldError> {
    errors
        .iter()
        .filter(|err| !WORKSPACE_FIELDS.contains(&err.field.as_str()))
        .collect()
}

/// One sentence naming the problems this screen has no field for.
///
/// `None` when there are none, so the caller can hand the result straight to
/// the form-level alert and have it disappear on the next clean submit.
fn summarise(errors: &[&FieldError]) -> Option<String> {
    if errors.is_empty() {
        return None;
    }

    // Each message is already a finished sentence ending in a full stop, so
    // they join with a space rather than a comma.
    let problems = errors
        .iter()
        .map(|err| crate::i18n::t(&err.message))
        .collect::<Vec<_>>()
        .join(" ");

    Some(l!("signup.problems_on_previous_step", problems = problems))
}

/// The signup screen, or the reason there isn't one here.
///
/// Split from the wizard below so that the tenant lookup wraps a component
/// rather than the wizard's body: `Suspense` calls its child more than once,
/// and the wizard owns closures that cannot be moved out of one.
#[component]
pub fn sign_up_page() -> impl IntoView {
    // `Err` on the bare domain, which is where signup belongs. A tenant here
    // means somebody followed a stale link or typed the path into a workspace
    // they are already a member of.
    let tenant = OnceResource::new(current_tenant());

    view! {
        <Suspense fallback=|| ()>
            {move || Suspend::new(async move {
                if tenant.await.is_ok() {
                    Either::Left(view! { <NotHere /> })
                } else {
                    Either::Right(view! { <SignupWizard /> })
                }
            })}
        </Suspense>
    }
}

/// What a workspace host says instead of the wizard.
///
/// No link to the signup host: this deployment cannot name it. `server.host`
/// is a bind address, and `base_domain` is the tenancy root - which is the
/// signup host in development and one label above it in production, where
/// pointing there would send somebody to a different site altogether. So the
/// screen offers the thing the visitor almost certainly wanted, which is to
/// sign in to the workspace they are already looking at.
#[component]
fn not_here() -> impl IntoView {
    view! {
        <Title text=format!("{} | Phonix", l!("signup.not_here.title")) />

        <div class="mx-auto w-full max-w-sm py-12">
            <h1 class="text-2xl font-semibold tracking-tight text-content">
                {l!("signup.not_here.title")}
            </h1>
            <p class="mt-2 text-sm text-content-muted">{l!("signup.not_here.body")}</p>

            <A
                href="/"
                attr:class="mt-6 inline-block font-medium text-brand hover:underline"
            >
                {l!("signup.not_here.sign_in")}
            </A>
        </div>
    }
}

#[component]
fn signup_wizard() -> impl IntoView {
    let step = RwSignal::new(Step::Account);

    let first_name = RwSignal::new(String::new());
    let last_name = RwSignal::new(String::new());
    let email = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let password_confirmation = RwSignal::new(String::new());
    let organization_name = RwSignal::new(String::new());
    let workspace_slug = RwSignal::new(String::new());

    // Server-side field errors, keyed by the field name the server used. The
    // client's own checks write into the same map, so a message looks the same
    // wherever it came from.
    let errors = RwSignal::new(Vec::<FieldError>::new());
    let form_error = RwSignal::new(Option::<String>::None);
    let submitting = RwSignal::new(false);

    let input_of = move || SignupInput {
        first_name: first_name.get(),
        last_name: last_name.get(),
        email: email.get(),
        password: password.get(),
        password_confirmation: password_confirmation.get(),
        organization_name: organization_name.get(),
        workspace_slug: workspace_slug.get(),
    };

    // Only the fields on screen one, so pressing "Continue" does not complain
    // about a workspace name the user has not been asked for yet.
    let continue_to_workspace = move |_| {
        let account_fields = [
            "first_name",
            "last_name",
            "email",
            "password",
            "password_confirmation",
        ];

        let found: Vec<FieldError> = input_of()
            .validate()
            .err()
            .unwrap_or_default()
            .into_iter()
            .filter(|err| account_fields.contains(&err.field.as_str()))
            .collect();

        errors.set(found.clone());
        if found.is_empty() {
            step.set(Step::Workspace);
        }
    };

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();

        let input = input_of();
        if let Err(found) = input.validate() {
            // The user stays on this screen: being thrown back to screen one
            // loses the workspace name they just typed and hides what went
            // wrong. Anything this screen cannot show is said at form level,
            // and "Back" still reaches the field itself with the error on it.
            form_error.set(summarise(&unshowable_here(&found)));
            errors.set(found);
            return;
        }

        errors.set(Vec::new());
        form_error.set(None);
        submitting.set(true);
        step.set(Step::Provisioning);

        leptos::task::spawn_local(async move {
            match create_workspace(input).await {
                Ok(SignupResult::Created(outcome)) => {
                    // A full page load, not a router navigation: the workspace
                    // is a different host, and the session cookie is waiting to
                    // be set there by the handoff endpoint.
                    let _ = window().location().set_href(&outcome.handoff_url);
                }
                Ok(SignupResult::Rejected(found)) => {
                    submitting.set(false);
                    // Off the spinner and back onto the form, with the same
                    // treatment the client-side check gets: the screen keeps
                    // what the user typed and states anything it cannot show.
                    step.set(Step::Workspace);
                    form_error.set(summarise(&unshowable_here(&found)));
                    errors.set(found);
                }
                Ok(SignupResult::Closed) => {
                    submitting.set(false);
                    step.set(Step::Workspace);
                    form_error.set(Some(l!("signup.closed")));
                }
                // Unreachable from this screen, which does not render on a
                // workspace host - but the endpoint is public and the match
                // has to be total, so it says the true thing rather than
                // falling into the generic failure below.
                Ok(SignupResult::NotHere) => {
                    submitting.set(false);
                    step.set(Step::Workspace);
                    form_error.set(Some(l!("signup.not_here.body")));
                }
                Err(err) => {
                    submitting.set(false);
                    step.set(Step::Workspace);
                    leptos::logging::error!("workspace creation failed: {err}");
                    form_error.set(Some(l!("signup.failed")));
                }
            }
        });
    };

    // A field's message, for binding to an input.
    let error_for = move |field: &'static str| -> Signal<Option<String>> {
        Signal::derive(move || {
            errors
                .get()
                .iter()
                .find(|err| err.field == field)
                .map(|err| crate::i18n::t(&err.message))
        })
    };

    view! {
        // "Phonix" is the product's name, not a word: it reads the same in
        // every language and is deliberately outside the catalog.
        <Title text=format!("{} | Phonix", l!("signup.title")) />

        <div class="mx-auto w-full max-w-measure py-12">
            <StepIndicator step=step />

            <Show when=move || step.get() == Step::Account>
                <div class="mt-8">
                    <h1 class="text-2xl font-semibold tracking-tight text-content">
                        {l!("signup.account.title")}
                    </h1>
                    <p class="mt-1 text-sm text-content-muted">
                        {l!("signup.account.subtitle")}
                    </p>

                    <div class="mt-8 space-y-5">
                        <div class="grid gap-4 sm:grid-cols-2">
                            <div>
                                <FieldLabel for_id="first_name" text=l!("field.first_name") />
                                <TextInput
                                    id="first_name"
                                    value=first_name
                                    autocomplete="given-name"
                                    error=error_for("first_name")
                                />
                            </div>
                            <div>
                                <FieldLabel for_id="last_name" text=l!("field.last_name") />
                                <TextInput
                                    id="last_name"
                                    value=last_name
                                    autocomplete="family-name"
                                    error=error_for("last_name")
                                />
                            </div>
                        </div>

                        <div>
                            <FieldLabel for_id="email" text=l!("signup.work_email") />
                            <TextInput
                                id="email"
                                input_type="email"
                                value=email
                                autocomplete="email"
                                error=error_for("email")
                            />
                        </div>

                        <div>
                            <FieldLabel for_id="password" text=l!("field.password") />
                            <TextInput
                                id="password"
                                input_type="password"
                                value=password
                                autocomplete="new-password"
                                error=error_for("password")
                            />
                            <StrengthMeter password=password />
                        </div>

                        <div>
                            <FieldLabel
                                for_id="password_confirmation"
                                text=l!("field.password_confirmation")
                            />
                            <TextInput
                                id="password_confirmation"
                                input_type="password"
                                value=password_confirmation
                                autocomplete="new-password"
                                error=error_for("password_confirmation")
                            />
                        </div>

                        <button
                            type="button"
                            class="w-full rounded-md bg-brand px-4 py-2 font-medium text-on-brand hover:bg-brand-hover focus:outline-none focus:ring-2 focus:ring-brand focus:ring-offset-2 focus:ring-offset-surface"
                            on:click=continue_to_workspace
                        >
                            {l!("common.continue")}
                        </button>
                    </div>

                    <p class="mt-6 text-sm text-content-muted">
                        {l!("signup.have_workspace")} " "
                        <A href="/" attr:class="font-medium text-brand hover:underline">
                            {l!("auth.signin.title")}
                        </A>
                    </p>
                </div>
            </Show>

            <Show when=move || step.get() == Step::Workspace>
                <form class="mt-8" on:submit=submit>
                    <h1 class="text-2xl font-semibold tracking-tight text-content">
                        {l!("signup.workspace.title")}
                    </h1>
                    <p class="mt-1 text-sm text-content-muted">
                        {l!("signup.workspace.subtitle")}
                    </p>

                    <div class="mt-8 space-y-5">
                        <div>
                            <FieldLabel
                                for_id="organization_name"
                                text=l!("field.organization_name")
                            />
                            <TextInput
                                id="organization_name"
                                value=organization_name
                                // An example company, not a word. Translating it
                                // would suggest the field wants a translation.
                                placeholder=l!("signup.organization_placeholder")
                                autocomplete="organization"
                                error=error_for("organization_name")
                            />
                        </div>

                        <WorkspaceAddressField
                            organization_name=organization_name
                            workspace_slug=workspace_slug
                            error=error_for("workspace_slug")
                        />

                        <FormError message=form_error />

                        <div class="flex gap-3">
                            <SecondaryButton
                                label=l!("common.back")
                                on_click=Callback::new(move |()| step.set(Step::Account))
                            />
                            <div class="flex-1">
                                <SubmitButton
                                    label=l!("signup.submit")
                                    pending=submitting
                                    pending_label=l!("signup.submitting")
                                />
                            </div>
                        </div>
                    </div>
                </form>
            </Show>

            <Show when=move || step.get() == Step::Provisioning>
                <ProvisioningScreen organization_name=organization_name />
            </Show>
        </div>
    }
}

/// Which of the three screens is current.
#[component]
fn step_indicator(step: RwSignal<Step>) -> impl IntoView {
    let index = move || match step.get() {
        Step::Account => 0,
        Step::Workspace => 1,
        Step::Provisioning => 2,
    };

    view! {
        // Read aloud, so it is a sentence and not machinery.
        <ol class="flex items-center gap-2" aria-label=l!("signup.progress")>
            {(0..3)
                .map(|position| {
                    view! {
                        <li class="flex-1">
                            <div class=move || {
                                let base = "h-1 rounded-full transition-colors";
                                if position <= index() {
                                    format!("{base} bg-brand")
                                } else {
                                    format!("{base} bg-surface-sunken")
                                }
                            }></div>
                        </li>
                    }
                })
                .collect::<Vec<_>>()}
        </ol>
    }
}

/// The subdomain field, with its availability check.
///
/// Suggests an address from the organization name until the user edits it, then
/// leaves it alone - a field that keeps overwriting what somebody typed is
/// worse than one that never suggested anything.
#[component]
fn workspace_address_field(
    organization_name: RwSignal<String>,
    workspace_slug: RwSignal<String>,
    error: Signal<Option<String>>,
) -> impl IntoView {
    let edited = RwSignal::new(false);
    let availability = RwSignal::new(Option::<(String, bool, Option<String>)>::None);

    // Follow the organization name until the address is touched.
    Effect::new(move |_| {
        let name = organization_name.get();
        if !edited.get() {
            workspace_slug.set(slug_from_organization_name(&name).unwrap_or_default());
        }
    });

    Effect::new(move |_| {
        let _ = workspace_slug.get();
        edited.set(true);
    });

    let check = move |_| {
        let candidate = workspace_slug.get();
        if candidate.trim().is_empty() {
            availability.set(None);
            return;
        }

        leptos::task::spawn_local(async move {
            if let Ok(answer) = check_workspace_address(candidate).await {
                availability.set(Some((answer.slug, answer.available, answer.reason)));
            }
        });
    };

    view! {
        <div>
            <FieldLabel for_id="workspace_slug" text=l!("field.workspace_address") />
            <div class="mt-1 flex items-center rounded-md border border-edge-strong bg-surface focus-within:border-brand focus-within:ring-2 focus-within:ring-brand">
                <input
                    id="workspace_slug"
                    name="workspace_slug"
                    class="control-bare w-full rounded-l-md bg-transparent px-3 py-2 text-content placeholder:text-content-subtle focus:outline-none"
                    placeholder=l!("signup.slug_placeholder")
                    prop:value=move || workspace_slug.get()
                    on:input=move |ev| {
                        edited.set(true);
                        workspace_slug.set(event_target_value(&ev));
                    }
                    on:blur=check
                />
                <span class="shrink-0 border-l border-edge px-3 py-2 text-sm text-content-subtle">
                    ".localhost:3000"
                </span>
            </div>

            {move || {
                error
                    .get()
                    .map(|message| view! { <p class="mt-1 text-sm text-danger">{message}</p> })
            }}

            {move || {
                availability
                    .get()
                    .filter(|_| error.get().is_none())
                    .map(|(slug, available, reason)| {
                        if available {
                            view! {
                                <p class="mt-1 text-sm text-success">
                                    {l!("signup.address.available", slug = slug)}
                                </p>
                            }
                                .into_any()
                        } else {
                            // The server may have said *why* it is unavailable;
                            // that sentence arrives already resolved. Otherwise
                            // the generic answer.
                            view! {
                                <p class="mt-1 text-sm text-danger">
                                    {reason.unwrap_or_else(|| l!("signup.address.taken"))}
                                </p>
                            }
                                .into_any()
                        }
                    })
            }}
        </div>
    }
}

/// The third screen: work is actually happening behind this.
///
/// Creating a database, running its migrations, writing the permission tree and
/// the owner account is a second or two of real work. The steps listed here are
/// the real ones, in the order `phonix_services::workspace::onboarding` does
/// them.
#[component]
fn provisioning_screen(organization_name: RwSignal<String>) -> impl IntoView {
    view! {
        <div class="mt-8 text-center" aria-live="polite">
            <div class="mx-auto h-10 w-10 animate-spin rounded-full border-2 border-edge border-t-brand"></div>

            // The name goes through the sentence rather than being glued to
            // the front of it: German puts it first and the verb last.
            <h1 class="mt-6 text-xl font-semibold tracking-tight text-content">
                {move || {
                    let name = organization_name.get();
                    let name = if name.trim().is_empty() {
                        l!("signup.provisioning.your_workspace")
                    } else {
                        name
                    };
                    l!("signup.provisioning.title", name = name)
                }}
            </h1>

            <ul class="mx-auto mt-6 max-w-xs space-y-2 text-left text-sm text-content-muted">
                <ProvisioningStep text=l!("signup.provisioning.database") />
                <ProvisioningStep text=l!("signup.provisioning.schema") />
                <ProvisioningStep text=l!("signup.provisioning.roles") />
                <ProvisioningStep text=l!("signup.provisioning.account") />
            </ul>

            <p class="mt-6 text-xs text-content-subtle">
                {l!("signup.provisioning.duration")}
            </p>
        </div>
    }
}

#[component]
fn provisioning_step(#[prop(into)] text: String) -> impl IntoView {
    view! {
        <li class="flex items-center gap-2">
            <span class="h-1.5 w-1.5 shrink-0 rounded-full bg-brand"></span>
            {text}
        </li>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(field: &str) -> FieldError {
        FieldError::new(field, phonix_core::msg!("validation.field.required"))
    }

    fn fields<'a>(errors: &[&'a FieldError]) -> Vec<&'a str> {
        errors.iter().map(|err| err.field.as_str()).collect()
    }

    #[test]
    fn the_workspace_screens_own_fields_are_shown_on_it() {
        let found = vec![err("organization_name"), err("workspace_slug")];
        assert!(unshowable_here(&found).is_empty());
    }

    /// The regression this guards. `password_echoes_identity` compares the
    /// password against the organization name, which is only collected on
    /// screen two - so a password containing it clears screen one and fails on
    /// submit with an error against `password`, a field screen two does not
    /// render. Binding that to a non-existent node is why the button appeared
    /// to do nothing.
    #[test]
    fn a_password_error_raised_on_the_workspace_screen_is_reported_not_dropped() {
        let found = vec![err("workspace_slug"), err("password")];
        assert_eq!(fields(&unshowable_here(&found)), ["password"]);
    }

    #[test]
    fn every_account_field_is_unshowable_on_the_workspace_screen() {
        for field in [
            "first_name",
            "last_name",
            "email",
            "password",
            "password_confirmation",
        ] {
            let found = vec![err(field)];
            assert_eq!(fields(&unshowable_here(&found)), [field], "{field}");
        }
    }

    #[test]
    fn nothing_to_report_leaves_the_alert_empty() {
        assert!(summarise(&[]).is_none());
    }
}
