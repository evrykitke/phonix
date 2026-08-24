//! Form primitives.
//!
//! Small on purpose. These exist so a field looks the same on every screen and
//! so an error message is rendered in one place - not to abstract over forms,
//! which are all different and should stay that way.
//!
//! Classes use the semantic tokens from `style/main.css` (`bg-surface`,
//! `text-content`, `border-edge`), never raw palette classes, so a re-theme is
//! a change to that file alone.

use leptos::prelude::*;
use phonix_core::identity::{PasswordStrength, password_strength};

use crate::icons::{Icon, IconSize};
use crate::l;

/// A label bound to its input.
#[component]
pub fn field_label(
    #[prop(into)] for_id: String,
    #[prop(into)] text: String,
    /// Shown in grey beside the label, for a field that may be left alone.
    #[prop(optional, into)]
    hint: Option<String>,
) -> impl IntoView {
    view! {
        <label for=for_id class="flex items-baseline justify-between text-sm font-medium text-content">
            <span>{text}</span>
            {hint.map(|hint| view! { <span class="text-xs font-normal text-content-subtle">{hint}</span> })}
        </label>
    }
}

/// A text input bound to a signal.
#[component]
pub fn text_input(
    #[prop(into)] id: String,
    value: RwSignal<String>,
    #[prop(optional, into)] input_type: Option<String>,
    #[prop(optional, into)] placeholder: Option<String>,
    #[prop(optional, into)] autocomplete: Option<String>,
    /// The message under the field. Present means the field is also outlined.
    #[prop(optional, into)]
    error: Option<Signal<Option<String>>>,
) -> impl IntoView {
    let name = id.clone();
    let has_error = move || error.is_some_and(|error| error.get().is_some());

    view! {
        <input
            id=id
            name=name
            type=input_type.unwrap_or_else(|| "text".to_owned())
            placeholder=placeholder.unwrap_or_default()
            autocomplete=autocomplete.unwrap_or_else(|| "off".to_owned())
            // Everything a text box looks like is the global rule in
            // `style/main.css`; the outline for a rejected value is not, and
            // the top margin is this component's own spacing.
            class=move || if has_error() { "mt-1 border-danger" } else { "mt-1" }
            prop:value=move || value.get()
            on:input=move |ev| value.set(event_target_value(&ev))
        />
        {move || {
            error
                .and_then(|error| error.get())
                .map(|message| {
                    view! { <p class="mt-1 text-sm text-danger">{message}</p> }
                })
        }}
    }
}

/// A password box with a control that reveals what is in it.
///
/// # Why a password field gets its own component
///
/// Typing a password you cannot see, on a phone keyboard, with a policy that
/// wants a symbol in it, is the single most common way somebody locks
/// themselves out of a screen that was working. The reveal is not a decoration:
/// it is the difference between "wrong password" and "wrong password, and no
/// way to find out why".
///
/// The toggle is a `<button type="button">`. Inside a form, a button with no
/// type is a **submit** button - so the browser's default would have made
/// "show my password" send the form.
///
/// # It is not remembered
///
/// Revealed state is local to this field and resets on every navigation. A
/// preference that persisted would eventually show somebody's password on a
/// shared screen because of a decision they made on their own laptop.
#[component]
pub fn password_input(
    #[prop(into)] id: String,
    value: RwSignal<String>,
    /// `new-password` while choosing one, `current-password` while signing in.
    /// Getting this wrong is what makes a password manager offer to save a
    /// sign-in, or fill a "choose a new password" box with the old one.
    #[prop(into)]
    autocomplete: String,
    #[prop(optional, into)] error: Option<Signal<Option<String>>>,
) -> impl IntoView {
    let name = id.clone();
    let toggle_for = id.clone();
    let revealed = RwSignal::new(false);
    let has_error = move || error.is_some_and(|error| error.get().is_some());

    view! {
        // `relative` so the button can be positioned over the box. The input
        // keeps the global styling from `style/main.css` - it is a plain
        // `input` and not `.control-bare`, so it draws its own border and this
        // wrapper adds nothing but a position.
        <div class="relative mt-1 max-w-measure">
            <input
                id=id
                name=name
                // Reactive: switching the attribute is what reveals it, and it
                // keeps the value, the cursor and the undo stack intact. A
                // second input swapped in would lose all three.
                type=move || if revealed.get() { "text" } else { "password" }
                autocomplete=autocomplete
                // `pr-10` so the text never runs under the button. `mt-0`
                // because the margin is on the wrapper now, not the box.
                class=move || {
                    if has_error() { "mt-0 pr-10 border-danger" } else { "mt-0 pr-10" }
                }
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev))
            />

            <button
                // Without this the browser treats it as a submit button and
                // revealing the password sends the form.
                type="button"
                class="absolute inset-y-0 right-0 grid w-10 place-items-center rounded-r-control text-content-subtle hover:text-content"
                // Named for what pressing it does, not for what it shows: a
                // screen reader announces the action, and an icon alone
                // announces nothing at all.
                aria-label=move || {
                    if revealed.get() { l!("field.password_hide") } else { l!("field.password_show") }
                }
                aria-controls=toggle_for
                aria-pressed=move || revealed.get().to_string()
                on:click=move |_| revealed.update(|shown| *shown = !*shown)
            >
                {move || {
                    // The eye is open when the password is hidden - the icon
                    // shows what the button will do, which is the convention
                    // every browser and password manager already uses.
                    let icon = if revealed.get() { Icon::EyeOff } else { Icon::Eye };
                    view! { <Icon icon=icon size=IconSize::Sm /> }
                }}
            </button>
        </div>

        {move || {
            error
                .and_then(|error| error.get())
                .map(|message| view! { <p class="mt-1 text-sm text-danger">{message}</p> })
        }}
    }
}

/// The form-level error, above the submit button.
#[component]
pub fn form_error(message: RwSignal<Option<String>>) -> impl IntoView {
    view! {
        {move || {
            message
                .get()
                .map(|message| {
                    view! {
                        <div
                            role="alert"
                            class="rounded-md border border-danger bg-danger-subtle px-3 py-2 text-sm text-danger"
                        >
                            {message}
                        </div>
                    }
                })
        }}
    }
}

/// A submit button that disables itself while a request is in flight.
///
/// Disabling matters here rather than being a nicety: a double-submitted signup
/// is a second workspace, and a double-submitted sign-in is a second Argon2
/// verification for no reason.
#[component]
pub fn submit_button(
    #[prop(into)] label: String,
    pending: RwSignal<bool>,
    #[prop(optional, into)] pending_label: Option<String>,
) -> impl IntoView {
    let pending_label = pending_label.unwrap_or_else(|| l!("common.working"));

    view! {
        <button
            type="submit"
            class="w-full rounded-md bg-brand px-4 py-2 font-medium text-on-brand \
                   hover:bg-brand-hover focus:outline-none focus:ring-2 focus:ring-brand \
                   focus:ring-offset-2 focus:ring-offset-surface disabled:opacity-60"
            disabled=move || pending.get()
        >
            // A spinner beside the word, not instead of it. "Working" on its
            // own is a label that could have been there all along; a moving
            // thing is what says the press registered.
            <span class="inline-flex items-center justify-center gap-2">
                {move || {
                    pending
                        .get()
                        .then(|| {
                            view! {
                                <Icon
                                    icon=Icon::LoaderCircle
                                    size=IconSize::Sm
                                    class="animate-spin"
                                />
                            }
                        })
                }}
                {move || if pending.get() { pending_label.clone() } else { label.clone() }}
            </span>
        </button>
    }
}

/// A secondary button, for "Back" in a wizard.
#[component]
pub fn secondary_button(
    #[prop(into)] label: String,
    #[prop(into)] on_click: Callback<()>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class="rounded-md border border-edge-strong px-4 py-2 font-medium text-content \
                   hover:bg-surface-sunken focus:outline-none focus:ring-2 focus:ring-brand"
            on:click=move |_| on_click.run(())
        >
            {label}
        </button>
    }
}

/// The password meter.
///
/// Advisory: it never blocks submission. `password_strength` is the same
/// function the server has, so a green bar and a server-side rejection cannot
/// disagree.
///
/// Shared by every screen that sets a password - signup, and the reset that
/// follows a forgotten one. It lives here rather than beside either of them
/// because two copies would be two sets of thresholds, and a meter that says
/// "good" on one screen and "fair" on another is worse than no meter.
#[component]
pub fn strength_meter(password: RwSignal<String>) -> impl IntoView {
    let strength = move || password_strength(&password.get());

    view! {
        <div class="mt-2" aria-live="polite">
            <div class="flex gap-1">
                {(0..4)
                    .map(|bar| {
                        view! {
                            <div class=move || {
                                let filled = bar < strength().filled_bars();
                                let colour = match strength() {
                                    PasswordStrength::Strong | PasswordStrength::Good => "bg-success",
                                    PasswordStrength::Fair => "bg-warning",
                                    _ => "bg-danger",
                                };
                                format!(
                                    "h-1 flex-1 rounded-full transition-colors {}",
                                    if filled { colour } else { "bg-surface-sunken" },
                                )
                            }></div>
                        }
                    })
                    .collect::<Vec<_>>()}
            </div>
            <p class="mt-1 text-xs text-content-subtle">
                {move || {
                    // An empty box has no strength to report, so it gets the
                    // advice instead of a word.
                    strength()
                        .message()
                        .map_or_else(|| l!("signup.password_hint"), |word| crate::i18n::t(&word))
                }}
            </p>
        </div>
    }
}
