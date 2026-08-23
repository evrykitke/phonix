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
    let pending_label = pending_label.unwrap_or_else(|| "Working...".to_owned());

    view! {
        <button
            type="submit"
            class="w-full rounded-md bg-brand px-4 py-2 font-medium text-on-brand \
                   hover:bg-brand-hover focus:outline-none focus:ring-2 focus:ring-brand \
                   focus:ring-offset-2 focus:ring-offset-surface disabled:opacity-60"
            disabled=move || pending.get()
        >
            {move || if pending.get() { pending_label.clone() } else { label.clone() }}
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
