//! Choosing a language.
//!
//! # Why a dropdown and not a segmented control
//!
//! The appearance menu next to this uses three buttons in a row, because there
//! are exactly three themes and there always will be. Languages are the
//! opposite: the list grows with the business, and a row of buttons that works
//! at two is unusable at nine.
//!
//! It is the kit's [`SelectField`](crate::ui::lookup::SelectField) and not the
//! browser's `<select>`, like every other dropdown in the app. This is the one
//! control where that costs something real - a native select on a phone opens
//! the platform's own wheel, and this one opens a panel - and it is still the
//! right trade: a menu of languages that does not take the app's theme is a
//! white sheet over a dark page, on the screen whose entire job is to be
//! legible to somebody who cannot read the rest of it.
//!
//! # Labelled in its own language
//!
//! Each option reads `Deutsch`, not `German`. Somebody stranded in a
//! language they cannot read is scanning for the word they *do* recognise, and
//! translating the language names into the language they are trying to leave is
//! precisely backwards.
//!
//! # Changing it reloads the page
//!
//! See `crate::i18n`: the server has to re-render anyway, because `<html lang>`
//! and every string resolved during SSR belong to the old language. Reloading
//! makes the change true everywhere at once.
//!
//! The reload is also why this works on the sign-in screen, where there is no
//! account to store a preference on: the cookie is written first, and the next
//! request - the reload - is already in the new language.

use leptos::prelude::*;
use phonix_core::i18n::Language;

use crate::i18n::Locale;
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::ui::form::field::Choice;
use crate::ui::lookup::SelectField;

/// Write the choice and start again in the new language.
fn switch_to(language: Language) {
    #[cfg(feature = "hydrate")]
    {
        crate::i18n::write_cookie(language);

        // Not a router navigation: the router would re-run the client half
        // against a document whose `<html lang>`, inlined catalog and
        // server-rendered strings are all still the previous language.
        let _ = window().location().reload();
    }

    #[cfg(not(feature = "hydrate"))]
    let _ = language;
}

/// The chooser, as a labelled section of the account menu.
#[component]
pub fn language_section() -> impl IntoView {
    // Only when there is a choice to make. One language is not a decision, and
    // a select with a single option is furniture.
    if Language::ALL.len() < 2 {
        return ().into_any();
    }

    let locale = Locale::get();
    let current = locale.language();
    // From the locale, not from the catalog: this decides whether a node
    // exists, so both halves of the application have to get it from the same
    // place. See `Locale::coverage`.
    let coverage = locale.coverage();

    view! {
        <div class="border-b border-edge px-3 py-2">
            <div class="flex items-center gap-1.5 text-xs font-medium uppercase tracking-wide text-content-subtle">
                <Icon icon=Icon::Languages size=IconSize::Xs />
                {l!("language.name")}
            </div>

            <div class="mt-2">
                <LanguageSelect current=current />
            </div>

            // Only when it would explain something. A complete translation
            // needs no note, and a note under every language would train people
            // to stop reading it.
            {(coverage < 100)
                .then(|| {
                    view! {
                        <p class="mt-1.5 text-2xs leading-snug text-content-subtle">
                            {l!("language.partial")}
                        </p>
                    }
                })}
        </div>
    }
    .into_any()
}

/// The chooser on its own, for screens with no account menu - sign in, sign up,
/// accepting an invitation.
///
/// Somebody who cannot read the sign-in form cannot sign in to change the
/// language, so the control has to exist before the session does.
#[component]
pub fn language_picker() -> impl IntoView {
    if Language::ALL.len() < 2 {
        return ().into_any();
    }

    let current = Locale::get().language();

    view! {
        <div class="flex items-center justify-center gap-2 text-sm text-content-muted">
            <Icon icon=Icon::Languages size=IconSize::Xs />
            <LanguageSelect current=current />
        </div>
    }
    .into_any()
}

/// The control both wrappers draw.
#[component]
fn language_select(current: Language) -> impl IntoView {
    let options = Language::ALL
        .iter()
        .copied()
        .map(|language| Choice::new(language.code(), language.native_name()))
        .collect::<Vec<_>>();

    view! {
        <SelectField
            // Fixed for the life of this control: changing it reloads the page,
            // so the value never moves under the field.
            value=current.code().to_owned()
            on_change=Callback::new(move |value: String| {
                // An unrecognised value can only come from a tampered DOM, and
                // the answer to that is to do nothing rather than to guess.
                if let Some(language) = Language::parse(&value)
                    && language != current
                {
                    switch_to(language);
                }
            })
            options=options
            label=l!("language.choose")
        />
    }
}
