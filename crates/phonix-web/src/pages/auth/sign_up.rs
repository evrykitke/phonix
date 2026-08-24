//! The signup wizard.
//!
//! Three screens, **one request**. The wizard collects an account on step one,
//! a workspace on step two, and submits both together - a catalog row with no
//! owner, or an owner with no database, is a state nobody wants to reason about
//! later. Step three is the provisioning screen.
//!
//! Validation runs twice by design. The client's copy is
//! `phonix_core::identity::SignupInput::validate` compiled to WebAssembly, so
//! the messages are the same ones the server will produce; the server runs it
//! again because this endpoint is reachable without a browser.
//!
//! # The five seconds on the last screen are deliberate
//!
//! Creating a database, running its migrations, writing a permission tree and
//! the owner account is real work and takes a moment. It is not reliably five
//! seconds - on a warm machine it can be well under one - and the last screen
//! holds for [`PROVISION_HOLD`] regardless.
//!
//! That is a decision, not a measurement, and it is worth being straight about
//! which. The alternative is a form that blinks and is replaced by a different
//! host mid-blink, which reads as a crash: no confirmation that anything was
//! created, no chance to see the workspace's name, and a cross-origin
//! navigation arriving with no warning. The steps listed are the real ones in
//! the real order; their *pacing* is fixed. Nothing here is a progress bar
//! pretending to know something it does not - it is a bar counting down a hold
//! this file chose.
//!
//! If the server takes longer than the hold, the screen simply waits: the
//! redirect needs both the response and the timer, and whichever is slower
//! decides.
//!
//! # Responsive without a single measurement
//!
//! Every width decision below is a Tailwind class. Nothing asks how wide the
//! window is - a viewport read in Rust renders one tree on the server and
//! another in the browser, and that mismatch is a wasm panic that freezes the
//! whole application.

use std::time::Duration;

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::A;
use phonix_core::identity::{FieldError, SignupInput, SignupResult, slug_from_organization_name};

use crate::components::forms::{
    FieldLabel, FormError, PasswordInput, SecondaryButton, StrengthMeter, SubmitButton, TextInput,
};
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::server_fns::onboarding_fns::{check_workspace_address, create_workspace};
use crate::server_fns::public_fns::public_branding;
use crate::server_fns::tenant_fns::current_tenant;

/// How long the provisioning screen is shown before the browser leaves.
///
/// Matched by `--provision-duration` on the progress bar, so the bar reaches
/// full exactly as the navigation happens. Two numbers that disagree would
/// produce a bar that finishes early and then sits there, or one that is cut
/// off part-way - both of which read as something having gone wrong.
const PROVISION_HOLD: Duration = Duration::from_millis(5_000);

/// When each step is marked done, as a fraction of [`PROVISION_HOLD`].
///
/// Front-loaded and uneven on purpose. Evenly spaced ticks read as a
/// progress bar wearing a checklist's clothes; an irregular rhythm reads as
/// separate things finishing at their own speed, which is what is actually
/// happening behind this.
const STEP_MARKS: [u64; 4] = [900, 2_000, 3_200, 4_400];

/// Which screen the wizard is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Account,
    Workspace,
    Provisioning,
}

impl Step {
    fn index(self) -> usize {
        match self {
            Self::Account => 0,
            Self::Workspace => 1,
            Self::Provisioning => 2,
        }
    }
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

        <div class="mx-auto w-full max-w-measure">
            <div class="rounded-card border border-edge bg-surface-raised p-5 text-center shadow-sm sm:p-8">
                <div class="mx-auto grid size-10 place-items-center rounded-full bg-surface-sunken text-content-muted">
                    <Icon icon=Icon::Building2 size=IconSize::Sm />
                </div>

                <h1 class="mt-4 text-xl font-semibold tracking-tight text-content">
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

    // Where to go when both the response and the hold are done. `None` while
    // the request is still in flight.
    let destination = RwSignal::new(Option::<String>::None);
    let hold_elapsed = RwSignal::new(false);

    // The redirect waits on both, so whichever is slower decides - a server
    // that answers in 300ms still gets the full screen, and a server that
    // takes eight seconds is not cut off at five.
    Effect::new(move |_| {
        if hold_elapsed.get()
            && let Some(target) = destination.get()
        {
            // A full page load, not a router navigation: the workspace is a
            // different host, and the session cookie is waiting to be set
            // there by the handoff endpoint.
            let _ = window().location().set_href(&target);
        }
    });

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

    // Anything that puts the wizard back on the form: a rejection, a closed
    // deployment, a transport failure. Collected because each of them has to
    // undo the same three things, and one that forgot to clear `submitting`
    // left a permanently disabled button.
    let back_to_form = move |problem: Option<String>| {
        submitting.set(false);
        hold_elapsed.set(false);
        destination.set(None);
        step.set(Step::Workspace);
        form_error.set(problem);
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
        destination.set(None);
        hold_elapsed.set(false);
        step.set(Step::Provisioning);
        start_hold(hold_elapsed);

        leptos::task::spawn_local(async move {
            match create_workspace(input).await {
                Ok(SignupResult::Created(outcome)) => {
                    // Not a redirect: the effect above owns that, and it fires
                    // when the hold is also done.
                    destination.set(Some(outcome.handoff_url));
                }
                Ok(SignupResult::Rejected(found)) => {
                    back_to_form(summarise(&unshowable_here(&found)));
                    errors.set(found);
                }
                Ok(SignupResult::Closed) => back_to_form(Some(l!("signup.closed"))),
                // Unreachable from this screen, which does not render on a
                // workspace host - but the endpoint is public and the match
                // has to be total, so it says the true thing rather than
                // falling into the generic failure below.
                Ok(SignupResult::NotHere) => back_to_form(Some(l!("signup.not_here.body"))),
                Err(err) => {
                    leptos::logging::error!("workspace creation failed: {err}");
                    back_to_form(Some(l!("signup.failed")));
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

        <div class="mx-auto w-full max-w-measure">
            <StepIndicator step=step />

            <div class="mt-6 rounded-card border border-edge bg-surface-raised p-5 shadow-sm sm:p-8">
                <Show when=move || step.get() == Step::Account>
                    <h1 class="text-2xl font-semibold tracking-tight text-content">
                        {l!("signup.account.title")}
                    </h1>
                    <p class="mt-1 text-sm text-content-muted">
                        {l!("signup.account.subtitle")}
                    </p>

                    <div class="mt-6 space-y-4">
                        // One column on a phone, two from `sm` up. The two
                        // halves of a name are short enough to share a row the
                        // moment there is a row to share.
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
                            <PasswordInput
                                id="password"
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
                            <PasswordInput
                                id="password_confirmation"
                                value=password_confirmation
                                autocomplete="new-password"
                                error=error_for("password_confirmation")
                            />
                        </div>

                        <button
                            type="button"
                            class="w-full rounded-control bg-brand px-4 py-2 font-medium text-on-brand \
                                   hover:bg-brand-hover focus:outline-none focus:ring-2 \
                                   focus:ring-brand focus:ring-offset-2 focus:ring-offset-surface"
                            on:click=continue_to_workspace
                        >
                            {l!("common.continue")}
                        </button>
                    </div>
                </Show>

                <Show when=move || step.get() == Step::Workspace>
                    <form on:submit=submit>
                        <h1 class="text-2xl font-semibold tracking-tight text-content">
                            {l!("signup.workspace.title")}
                        </h1>
                        <p class="mt-1 text-sm text-content-muted">
                            {l!("signup.workspace.subtitle")}
                        </p>

                        <div class="mt-6 space-y-4">
                            <div>
                                <FieldLabel
                                    for_id="organization_name"
                                    text=l!("field.organization_name")
                                />
                                <TextInput
                                    id="organization_name"
                                    value=organization_name
                                    // An example company, not a word.
                                    // Translating it would suggest the field
                                    // wants a translation.
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

                            // Stacked on a phone so neither button is a sliver;
                            // side by side from `sm` up, with the primary
                            // action taking the slack.
                            <div class="flex flex-col gap-3 sm:flex-row">
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
                    <ProvisioningScreen
                        organization_name=organization_name
                        ready=destination
                    />
                </Show>
            </div>

            // Outside the card, and only while there is still a form in it -
            // offering "already have a workspace?" to somebody whose workspace
            // is three seconds from existing is a link to nowhere useful.
            <Show when=move || step.get() != Step::Provisioning>
                <p class="mt-6 text-center text-sm text-content-muted">
                    {l!("signup.have_workspace")} " "
                    <A href="/" attr:class="font-medium text-brand hover:underline">
                        {l!("auth.signin.title")}
                    </A>
                </p>
            </Show>
        </div>
    }
}

/// Start the clock on the provisioning screen.
///
/// Split out because the browser and the server disagree about whether there is
/// a clock at all. During SSR this screen is never the one rendered - the
/// wizard always starts on step one - so there is nothing to time.
#[cfg(feature = "hydrate")]
fn start_hold(elapsed: RwSignal<bool>) {
    leptos::prelude::set_timeout(move || elapsed.set(true), PROVISION_HOLD);
}

/// No timer on the server, so the hold is over before it starts.
///
/// `true` rather than `false`, and the difference matters: were this to run
/// server-side with `false`, the effect that redirects would never fire and the
/// screen would wait for ever.
#[cfg(not(feature = "hydrate"))]
fn start_hold(elapsed: RwSignal<bool>) {
    let _ = PROVISION_HOLD;
    elapsed.set(true);
}

/// Which of the three screens is current.
///
/// Numbered and named, rather than three anonymous bars. A bare progress bar
/// says how far along somebody is; it does not say what is coming, and "what
/// else are you about to ask me for" is the actual question somebody has on the
/// first screen of a signup.
#[component]
fn step_indicator(step: RwSignal<Step>) -> impl IntoView {
    let labels = [
        l!("signup.step.account"),
        l!("signup.step.workspace"),
        l!("signup.step.ready"),
    ];

    view! {
        // Read aloud as a sentence rather than as machinery.
        <ol
            class="flex items-center gap-2"
            aria-label=move || {
                l!(
                    "signup.step.position",
                    current = (step.get().index() + 1).to_string(),
                    total = "3",
                )
            }
        >
            {labels
                .into_iter()
                .enumerate()
                .map(|(position, label)| {
                    let done = move || position < step.get().index();
                    let current = move || position == step.get().index();

                    view! {
                        <li class="flex min-w-0 flex-1 flex-col gap-1.5">
                            <div class=move || {
                                let base = "h-1 rounded-full transition-colors duration-300";
                                if done() || current() {
                                    format!("{base} bg-brand")
                                } else {
                                    format!("{base} bg-surface-sunken")
                                }
                            }></div>

                            // The label is hidden below `sm`, where three of
                            // them will not fit without wrapping into a block
                            // taller than the bars they describe. The bars stay
                            // at every width, and the whole list keeps its
                            // accessible name - so this is hidden visually and
                            // nowhere else.
                            <span class=move || {
                                let base = "hidden truncate-fade text-xs sm:block";
                                if current() {
                                    format!("{base} font-medium text-content")
                                } else {
                                    format!("{base} text-content-subtle")
                                }
                            }>{label}</span>
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
    let checking = RwSignal::new(false);

    // What every workspace address ends in. From the server, because it is
    // built out of `[server]` - this was the literal string ".localhost:3000"
    // until `public_branding` existed, which was correct on one machine and a
    // promise of a broken address to every real customer.
    let branding = OnceResource::new(public_branding());
    let suffix = Signal::derive(move || {
        branding
            .get()
            .and_then(Result::ok)
            .map(|branding| branding.workspace_suffix)
            .unwrap_or_default()
    });

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

        checking.set(true);
        leptos::task::spawn_local(async move {
            if let Ok(answer) = check_workspace_address(candidate).await {
                availability.set(Some((answer.slug, answer.available, answer.reason)));
            }
            checking.set(false);
        });
    };

    view! {
        <div>
            <FieldLabel for_id="workspace_slug" text=l!("field.workspace_address") />

            // `max-w-measure` matches what the global rule gives every other
            // box, so this compound control lines up with the plain ones above
            // it instead of running the full width of the card.
            <div class="mt-1 flex max-w-measure items-center rounded-control border border-edge bg-surface-raised focus-within:border-brand focus-within:ring-2 focus-within:ring-brand">
                <input
                    id="workspace_slug"
                    name="workspace_slug"
                    // `.control-bare` opts out of the global box styling: this
                    // one lives inside its own chrome, and a second border
                    // would be visible.
                    class="control-bare w-full min-w-0 rounded-l-control bg-transparent px-3 py-2 text-content placeholder:text-content-subtle focus:outline-none"
                    placeholder=l!("signup.slug_placeholder")
                    prop:value=move || workspace_slug.get()
                    on:input=move |ev| {
                        edited.set(true);
                        workspace_slug.set(event_target_value(&ev));
                    }
                    on:blur=check
                />

                // `truncate` and `max-w-[45%]`: a production suffix is short,
                // but nothing stops a deployment having a long one, and a
                // suffix that grows without limit would push the box it labels
                // off the side of a phone.
                <span class="max-w-[45%] shrink-0 truncate border-l border-edge px-3 py-2 text-sm text-content-subtle">
                    {move || suffix.get()}
                </span>
            </div>

            {move || {
                error
                    .get()
                    .map(|message| view! { <p class="mt-1 text-sm text-danger">{message}</p> })
            }}

            {move || {
                checking
                    .get()
                    .then(|| {
                        view! {
                            <p class="mt-1 flex items-center gap-1.5 text-sm text-content-subtle">
                                <Icon
                                    icon=Icon::LoaderCircle
                                    size=IconSize::Xs
                                    class="animate-spin"
                                />
                                {l!("signup.address.checking")}
                            </p>
                        }
                    })
            }}

            {move || {
                availability
                    .get()
                    .filter(|_| error.get().is_none() && !checking.get())
                    .map(|(slug, available, reason)| {
                        if available {
                            view! {
                                <p class="mt-1 flex items-center gap-1.5 text-sm text-success">
                                    <Icon icon=Icon::CircleCheck size=IconSize::Xs />
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

/// The third screen: work is happening behind this, and then a hold.
///
/// The steps listed are the real ones, in the order
/// `phonix_services::workspace::onboarding` does them. Their timing is not: see
/// the note at the top of this file on why the pacing is a decision rather than
/// a measurement.
#[component]
fn provisioning_screen(
    organization_name: RwSignal<String>,
    /// `Some` once the server has answered. Only the heading reads it - the
    /// redirect is the wizard's, so this screen cannot navigate on its own.
    ready: RwSignal<Option<String>>,
) -> impl IntoView {
    let workspace_name = move || {
        let name = organization_name.get();
        if name.trim().is_empty() {
            l!("signup.provisioning.your_workspace")
        } else {
            name
        }
    };

    view! {
        // `aria-live="polite"`: the heading changes when the workspace is
        // ready, and somebody using a screen reader should be told without
        // being interrupted.
        <div class="py-2 text-center" aria-live="polite">
            <ProvisioningRing />

            <h1 class="mt-6 text-xl font-semibold tracking-tight text-content">
                {move || {
                    if ready.get().is_some() {
                        l!("signup.provisioning.opening", name = workspace_name())
                    } else {
                        // The name goes through the sentence rather than being
                        // glued to the front of it: German puts it first and
                        // the verb last.
                        l!("signup.provisioning.title", name = workspace_name())
                    }
                }}
            </h1>

            <p class="mt-1 text-sm text-content-muted">
                {l!("signup.provisioning.almost")}
            </p>

            // The bar is the only honest statement of how much longer this
            // screen has, because the hold is the thing being waited on once
            // the server has answered. Its duration is set from the same
            // constant the timer uses, so the two cannot drift.
            <div
                class="mx-auto mt-6 h-1 w-full max-w-xs overflow-hidden rounded-full bg-surface-sunken"
                role="presentation"
            >
                <div
                    class="provision-fill h-full w-full rounded-full bg-brand"
                    style=format!("--provision-duration: {}ms", PROVISION_HOLD.as_millis())
                ></div>
            </div>

            <ul class="mx-auto mt-6 max-w-xs space-y-2.5 text-left text-sm">
                <ProvisioningStep at=STEP_MARKS[0] text=l!("signup.provisioning.database") />
                <ProvisioningStep at=STEP_MARKS[1] text=l!("signup.provisioning.schema") />
                <ProvisioningStep at=STEP_MARKS[2] text=l!("signup.provisioning.roles") />
                <ProvisioningStep at=STEP_MARKS[3] text=l!("signup.provisioning.account") />
            </ul>

            <p class="mt-6 text-xs text-content-subtle">
                {l!("signup.provisioning.duration")}
            </p>
        </div>
    }
}

/// Two arcs turning at different speeds, in opposite directions.
///
/// One rigid spinner is the thing that makes people wonder whether it has
/// frozen; two arcs at different rates never repeat the same silhouette, so it
/// reads as running even when somebody stares at it for five seconds - which is
/// exactly what this screen asks them to do.
#[component]
fn provisioning_ring() -> impl IntoView {
    view! {
        <div class="relative mx-auto size-16" aria-hidden="true">
            <div class="provision-sweep absolute inset-0 rounded-full border-2 border-edge border-t-brand"></div>
            <div class="provision-sweep-reverse absolute inset-2 rounded-full border-2 border-transparent border-b-brand-subtle"></div>
            <div class="absolute inset-0 grid place-items-center text-brand">
                <Icon icon=Icon::Building2 size=IconSize::Sm />
            </div>
        </div>
    }
}

/// One line of the checklist, which completes on its own schedule.
///
/// Each step owns its timer rather than the parent driving all four from one.
/// That keeps the marks declarative - the number is written beside the line it
/// belongs to - and it means a step added or removed changes one place.
#[component]
fn provisioning_step(
    /// Milliseconds after this screen appears at which the step is marked done.
    at: u64,
    #[prop(into)] text: String,
) -> impl IntoView {
    let done = RwSignal::new(false);
    mark_done_after(done, at);

    view! {
        <li class=move || {
            let base = "flex items-center gap-2.5 transition-colors duration-300";
            if done.get() {
                format!("{base} text-content")
            } else {
                format!("{base} text-content-subtle")
            }
        }>
            // A fixed-size box either way, so the row does not shift sideways
            // when the dot becomes a tick.
            <span class="grid size-4 shrink-0 place-items-center">
                {move || {
                    if done.get() {
                        view! {
                            <span class="provision-step-done text-success">
                                <Icon icon=Icon::Check size=IconSize::Xs />
                            </span>
                        }
                            .into_any()
                    } else {
                        view! {
                            <span class="provision-waiting size-1.5 rounded-full bg-brand"></span>
                        }
                            .into_any()
                    }
                }}
            </span>
            {text}
        </li>
    }
}

/// Flip a signal once, after a delay.
#[cfg(feature = "hydrate")]
fn mark_done_after(done: RwSignal<bool>, millis: u64) {
    leptos::prelude::set_timeout(move || done.set(true), Duration::from_millis(millis));
}

/// On the server there is no delay to wait out.
///
/// The steps render already done, which never reaches a browser: this screen
/// only exists after a button press, so it is always hydrated before it is
/// seen. Marking them done rather than pending is still the right server-side
/// answer - a static render of "four things are about to happen" is a lie in a
/// document nobody will ever update.
#[cfg(not(feature = "hydrate"))]
fn mark_done_after(done: RwSignal<bool>, millis: u64) {
    let _ = millis;
    done.set(true);
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

    #[test]
    fn every_step_completes_before_the_hold_ends() {
        // A step still pulsing as the browser navigates away reads as
        // something having been left unfinished.
        let hold = PROVISION_HOLD.as_millis() as u64;

        for mark in STEP_MARKS {
            assert!(mark < hold, "{mark}ms is not inside a {hold}ms hold");
        }
    }

    #[test]
    fn the_steps_are_in_order_and_none_land_together() {
        // Two ticks at the same instant read as one event, which loses the
        // sense of separate things finishing.
        for pair in STEP_MARKS.windows(2) {
            assert!(pair[0] < pair[1], "{:?} is out of order", pair);
        }
    }

    #[test]
    fn the_indicator_position_matches_the_step() {
        assert_eq!(Step::Account.index(), 0);
        assert_eq!(Step::Workspace.index(), 1);
        assert_eq!(Step::Provisioning.index(), 2);
    }
}
