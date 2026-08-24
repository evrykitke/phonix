//! "I forgot my password" - ask for a code, then set a new one.
//!
//! Two steps on one route. The email is typed on the first and carried to the
//! second in a signal rather than in the URL: a reset address in the address
//! bar ends up in browser history, in a screenshot of a support ticket, and in
//! the `Referer` of anything the page links to.
//!
//! # This screen is told nothing, and that is the feature
//!
//! Step one always advances. Not "advances if the address was found" - always,
//! because the server does not say, and it does not say because any difference
//! between a known and an unknown address turns this form into a membership
//! check against the workspace. See
//! [`phonix_services::identity::password_reset`], where the silence is
//! implemented and argued.
//!
//! So the wording matters more than usual. **"If that address has an account,
//! a code is on its way"** is honest about the conditional. "We have sent you a
//! code" would be a lie half the time, and worse, it would be a lie somebody
//! reports as a bug.
//!
//! # A wrong code costs one of five
//!
//! Six digits is a million guesses, and the only thing making that safe is that
//! the code dies after five wrong attempts. The screen does not show a
//! remaining count: it would be a small oracle of its own, and the failure
//! message says the one thing worth acting on, which is to ask for another
//! code.
//!
//! # Why the password is validated here before it is submitted
//!
//! Redeeming spends the code whether or not the new password turns out to be
//! acceptable - the service has no safe way to check them in the other order.
//! The browser therefore applies the system password rules before submitting,
//! so the ordinary case of "that is too short" does not cost somebody their
//! code and a second trip to their mailbox. The workspace's own policy may be
//! stricter than what is checked here, and it is the authority; this catches
//! the common mistake, not every one.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::A;
use phonix_core::identity::{FieldError, SIGN_IN_PATH, validate_password};

use crate::components::forms::{
    FieldLabel, FormError, PasswordInput, SecondaryButton, StrengthMeter, SubmitButton, TextInput,
};
use crate::i18n::t;
use crate::l;
use crate::server_fns::reset_fns::{
    PasswordResetInput, PasswordResetResult, ResetRequested, complete_password_reset,
    request_password_reset,
};

/// Which of the three screens is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Type the address.
    Ask,
    /// Type the code and the new password.
    Verify,
    /// Done. Nothing left to fill in.
    Done,
}

#[component]
pub fn forgot_password_page() -> impl IntoView {
    let step = RwSignal::new(Step::Ask);

    let email = RwSignal::new(String::new());
    let code = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());

    let errors = RwSignal::new(Vec::<FieldError>::new());
    let form_error = RwSignal::new(Option::<String>::None);
    let submitting = RwSignal::new(false);

    // A field's message, for binding to an input. Same shape as the signup
    // wizard's, and for the same reason: the server names a field and the
    // screen decides where that lands.
    let error_for = move |field: &'static str| -> Signal<Option<String>> {
        Signal::derive(move || {
            errors
                .get()
                .iter()
                .find(|err| err.field == field)
                .map(|err| t(&err.message))
        })
    };

    let ask = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();

        let address = email.get().trim().to_owned();
        if address.is_empty() {
            form_error.set(Some(l!("reset.need_email")));
            return;
        }

        form_error.set(None);
        submitting.set(true);

        leptos::task::spawn_local(async move {
            match request_password_reset(address).await {
                // Both outcomes advance, and this is the whole point. The
                // server will not say whether an account exists, so neither
                // can this - the next screen states the condition instead.
                Ok(ResetRequested::Accepted) => {
                    submitting.set(false);
                    step.set(Step::Verify);
                }
                Ok(ResetRequested::Disabled) => {
                    submitting.set(false);
                    form_error.set(Some(l!("reset.disabled")));
                }
                Err(err) => {
                    submitting.set(false);
                    leptos::logging::error!("password reset request failed: {err}");
                    form_error.set(Some(l!("reset.transport")));
                }
            }
        });
    };

    let verify = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();

        errors.set(Vec::new());
        form_error.set(None);

        // Every check that can be made without spending the code is made here.
        // See the module note: redeeming consumes the attempt regardless of
        // what the password turns out to be.
        let mut found = Vec::new();

        if code.get().trim().is_empty() {
            found.push(FieldError::new(
                "code",
                phonix_core::msg!("reset.need_code"),
            ));
        }
        if let Err(problem) = validate_password(&password.get()) {
            found.push(problem);
        }
        if password.get() != confirm.get() {
            found.push(FieldError::new(
                "password_confirmation",
                phonix_core::msg!("validation.password.mismatch"),
            ));
        }

        if !found.is_empty() {
            errors.set(found);
            return;
        }

        submitting.set(true);

        let input = PasswordResetInput {
            email: email.get(),
            code: code.get(),
            password: password.get(),
            password_confirmation: confirm.get(),
        };

        leptos::task::spawn_local(async move {
            match complete_password_reset(input).await {
                Ok(PasswordResetResult::Reset) => {
                    submitting.set(false);
                    // The typed password does not stay in a signal after it has
                    // been set. It buys little on its own - the value is in the
                    // DOM node until the screen changes - but a page that keeps
                    // it around after it is finished with is a page somebody
                    // later reads it out of.
                    password.set(String::new());
                    confirm.set(String::new());
                    code.set(String::new());
                    step.set(Step::Done);
                }
                Ok(PasswordResetResult::CodeNotUsable) => {
                    submitting.set(false);
                    // Wrong, expired, out of attempts, or an address with no
                    // account. One message, and it names the only useful next
                    // action rather than guessing which of the four it was.
                    code.set(String::new());
                    form_error.set(Some(l!("reset.code_not_usable")));
                }
                Ok(PasswordResetResult::Rejected(found)) => {
                    submitting.set(false);
                    // The code was good and is now spent, so there is no second
                    // go at this one. Said plainly, above the field errors,
                    // because otherwise somebody fixes the password and presses
                    // the button into a message that has not changed.
                    form_error.set(Some(l!("reset.password_refused_code_spent")));
                    errors.set(found);
                }
                Ok(PasswordResetResult::Disabled) => {
                    submitting.set(false);
                    form_error.set(Some(l!("reset.disabled")));
                }
                Err(err) => {
                    submitting.set(false);
                    leptos::logging::error!("password reset failed: {err}");
                    form_error.set(Some(l!("reset.transport")));
                }
            }
        });
    };

    view! {
        <Title text=format!("{} | Phonix", l!("reset.title")) />

        // No vertical padding of its own: the public chrome owns the page's
        // margins, and a screen that adds its own ends up with two.
        <div class="mx-auto w-full max-w-measure">
            <div class="rounded-card border border-edge bg-surface-raised p-5 shadow-sm sm:p-8">
            <Show when=move || step.get() == Step::Ask>
                <h1 class="text-2xl font-semibold tracking-tight text-content">
                    {l!("reset.title")}
                </h1>
                <p class="mt-1 text-sm text-content-muted">{l!("reset.ask.subtitle")}</p>

                <form class="mt-8 space-y-5" on:submit=ask>
                    <div>
                        <FieldLabel for_id="reset-email" text=l!("reset.email") />
                        <TextInput
                            id="reset-email"
                            input_type="email"
                            value=email
                            autocomplete="username"
                        />
                    </div>

                    <FormError message=form_error />

                    <SubmitButton
                        label=l!("reset.ask.submit")
                        pending=submitting
                    />
                </form>
            </Show>

            <Show when=move || step.get() == Step::Verify>
                <h1 class="text-2xl font-semibold tracking-tight text-content">
                    {l!("reset.verify.title")}
                </h1>
                // The conditional is not hedging - it is the only honest thing
                // this screen can say. See the module note.
                <p class="mt-1 text-sm text-content-muted">
                    {move || l!("reset.verify.subtitle", email = email.get())}
                </p>

                <form class="mt-8 space-y-5" on:submit=verify>
                    <div>
                        <FieldLabel
                            for_id="reset-code"
                            text=l!("reset.code")
                            hint=l!("reset.code_hint")
                        />
                        <TextInput
                            id="reset-code"
                            value=code
                            // `one-time-code` is what tells a phone to offer
                            // the digits it just saw arrive by SMS or mail,
                            // which is the difference between typing six
                            // digits and tapping once.
                            autocomplete="one-time-code"
                            error=error_for("code")
                        />
                    </div>

                    <div>
                        <FieldLabel for_id="reset-password" text=l!("reset.new_password") />
                        <PasswordInput
                            id="reset-password"
                            value=password
                            autocomplete="new-password"
                            error=error_for("password")
                        />
                        <StrengthMeter password=password />
                    </div>

                    <div>
                        <FieldLabel for_id="reset-confirm" text=l!("reset.confirm_password") />
                        <PasswordInput
                            id="reset-confirm"
                            value=confirm
                            autocomplete="new-password"
                            error=error_for("password_confirmation")
                        />
                    </div>

                    <FormError message=form_error />

                    <SubmitButton
                        label=l!("reset.verify.submit")
                        pending=submitting
                    />

                    <SecondaryButton
                        label=l!("reset.start_over")
                        on_click=Callback::new(move |()| {
                            // Back to the address, with everything the second
                            // screen collected dropped. A code that was typed
                            // against one address must not survive into an
                            // attempt at another.
                            code.set(String::new());
                            password.set(String::new());
                            confirm.set(String::new());
                            errors.set(Vec::new());
                            form_error.set(None);
                            step.set(Step::Ask);
                        })
                    />
                </form>
            </Show>

            <Show when=move || step.get() == Step::Done>
                <h1 class="text-2xl font-semibold tracking-tight text-content">
                    {l!("reset.done.title")}
                </h1>
                <p class="mt-2 text-sm text-content-muted">{l!("reset.done.detail")}</p>

                <A
                    href=SIGN_IN_PATH
                    attr:class="mt-6 inline-block font-medium text-brand hover:underline"
                >
                    {l!("reset.done.sign_in")}
                </A>
            </Show>

            </div>

            // Outside the card: somewhere else to go, not part of resetting.
            <p class="mt-6 text-center text-sm">
                <A href=SIGN_IN_PATH attr:class="text-content-muted hover:underline">
                    {l!("reset.back_to_sign_in")}
                </A>
            </p>
        </div>
    }
}
