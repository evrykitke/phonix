//! The organization's logo: showing it, replacing it, removing it.
//!
//! # Why it is a panel and not a form field
//!
//! The profile form edits one row and saves it whole. A logo is an upload -
//! bytes that go to a route, wait in quarantine, get verified, and only then
//! become a file this row may point at. None of that fits inside a submit, and
//! putting it there would mean a form whose save sometimes takes ten seconds
//! and sometimes fails for a reason none of its fields can display.
//!
//! It also means a stale draft cannot revert it. Somebody who opened the
//! settings screen before a colleague replaced the logo, then corrected a
//! postcode and saved, would otherwise put the old mark back on every document
//! without ever choosing to.
//!
//! # No cropper
//!
//! Deliberate - see the note in [`browser`]. A wordmark is wide, and squaring
//! it is how a logo becomes unreadable.

use leptos::prelude::*;
use uuid::Uuid;

use crate::components::page::{Notice, Panel, Tone};
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::server_fns::file_fns::{content_url, remove_organization_logo};

/// The bucket logos are uploaded to.
///
/// Named once, here, and read by both halves: the browser posts to it and the
/// service checks the stored row against it. Two spellings of this string would
/// be an upload that succeeds and an attach that refuses.
pub(crate) const BUCKET: &str = "logos";

/// Upload, replace or remove the mark that goes on this workspace's documents.
///
/// `current` is owned by the caller so the surrounding screen can re-read the
/// profile without this panel and the form disagreeing about what is set.
#[component]
pub fn organization_logo(current: RwSignal<Option<Uuid>>) -> impl IntoView {
    let busy = RwSignal::new(false);
    let message = RwSignal::new(None::<(String, Tone)>);

    let remove = Action::new(move |(): &()| async move { remove_organization_logo().await });

    Effect::new(move |_| match remove.value().get() {
        Some(Ok(())) => {
            current.set(None);
            message.set(Some((
                "The logo has been removed.".to_owned(),
                Tone::Success,
            )));
        }
        Some(Err(err)) => message.set(Some((err.to_string(), Tone::Danger))),
        None => {}
    });

    let pending = move || busy.get() || remove.pending().get();

    view! {
        <Panel title=l!("logo.title") description=l!("logo.description")>
            <div class="space-y-3">
                <div class="flex flex-wrap items-center gap-4">
                    <Preview current=current />

                    <div class="space-y-2">
                        // The input is hidden rather than styled: a file input
                        // cannot be made to look like anything, and a
                        // `<label for>` is the one way to trigger it that
                        // keyboards and screen readers both already understand.
                        <label
                            class="inline-flex h-8 cursor-pointer items-center gap-1.5 rounded-control border border-edge px-3 text-sm text-content-muted hover:bg-surface-hover hover:text-content"
                            for="logo-file"
                        >
                            <Icon icon=Icon::Upload size=IconSize::Xs />
                            {move || {
                                if current.get().is_some() {
                                    l!("logo.replace")
                                } else {
                                    l!("logo.choose")
                                }
                            }}
                        </label>

                        <input
                            id="logo-file"
                            type="file"
                            class="sr-only"
                            // Generated from the same table the server checks
                            // against. A courtesy, not a control.
                            accept=accept_attribute()
                            disabled=pending
                            on:change=move |ev| pick_and_upload(ev, busy, message, current)
                        />

                        <p class="text-xs text-content-subtle">{size_hint()}</p>
                    </div>

                    {move || {
                        current
                            .get()
                            .map(|_| {
                                view! {
                                    <button
                                        type="button"
                                        class="inline-flex h-8 items-center gap-1.5 rounded-control border border-edge px-3 text-sm text-content-muted hover:bg-surface-hover hover:text-content disabled:cursor-not-allowed disabled:opacity-60"
                                        disabled=pending
                                        on:click=move |_| {
                                            remove.dispatch(());
                                        }
                                    >
                                        <Icon icon=Icon::Trash2 size=IconSize::Xs />
                                        {l!("common.remove")}
                                    </button>
                                }
                            })
                    }}
                </div>

                {move || {
                    pending()
                        .then(|| {
                            view! {
                                <p class="text-xs text-content-subtle">
                                    {l!("logo.checking")}
                                </p>
                            }
                        })
                }}

                {move || {
                    message
                        .get()
                        .map(|(text, tone)| {
                            view! {
                                <Notice
                                    message=Signal::derive(move || Some(text.clone()))
                                    tone=tone
                                />
                            }
                        })
                }}
            </div>
        </Panel>
    }
}

/// The stored logo, or the space where one would go.
///
/// `object-contain` and not `object-cover`: a logo cropped to fill a box is a
/// logo with its edges cut off, which is the one thing a brand mark must not
/// have done to it.
#[component]
fn preview(current: RwSignal<Option<Uuid>>) -> impl IntoView {
    view! {
        <div class="flex h-20 w-40 shrink-0 items-center justify-center overflow-hidden rounded-control border border-edge bg-surface-sunken">
            {move || match current.get() {
                Some(id) => {
                    view! {
                        <img
                            src=content_url(id)
                            alt=l!("logo.alt")
                            class="max-h-full max-w-full object-contain"
                        />
                    }
                        .into_any()
                }
                None => {
                    view! {
                        <span class="text-xs text-content-subtle">{l!("logo.none")}</span>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

/// The extensions this bucket would actually take.
fn accept_attribute() -> String {
    phonix_core::files::bucket(BUCKET)
        .map(phonix_core::files::BucketPolicy::accept_attribute)
        .unwrap_or_default()
}

/// The limit, in words, straight from the bucket.
///
/// Written from the policy rather than typed out, so raising the ceiling does
/// not leave a sentence underneath it saying the old number.
fn size_hint() -> String {
    match phonix_core::files::bucket(BUCKET) {
        Some(policy) => format!(
            "PNG or JPEG, up to {}. SVG is not accepted.",
            phonix_core::files::human_size(policy.max_bytes),
        ),
        None => String::new(),
    }
}

#[cfg(feature = "hydrate")]
mod browser;

#[cfg(feature = "hydrate")]
use browser::pick_and_upload;

#[cfg(not(feature = "hydrate"))]
fn pick_and_upload(
    _ev: leptos::ev::Event,
    _busy: RwSignal<bool>,
    _message: RwSignal<Option<(String, Tone)>>,
    _current: RwSignal<Option<Uuid>>,
) {
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bucket_this_panel_posts_to_exists() {
        // The string is the contract between this panel and the service that
        // checks `row.bucket`. A mismatch is an upload that succeeds and an
        // attach that refuses, with nothing failing at compile time.
        assert!(
            phonix_core::files::bucket(BUCKET).is_some(),
            "{BUCKET} is not a declared bucket",
        );
    }

    #[test]
    fn the_bucket_refuses_anything_that_could_carry_a_script() {
        // The logo is rendered inline here and embedded in documents. An SVG
        // would be script running on this origin under the organization's name.
        let Some(policy) = phonix_core::files::bucket(BUCKET) else {
            panic!("{BUCKET} is not a declared bucket");
        };

        assert!(!policy.allow_active_content);
        assert!(!accept_attribute().contains("svg"));
    }

    #[test]
    fn the_hint_states_the_limit_the_bucket_actually_enforces() {
        let Some(policy) = phonix_core::files::bucket(BUCKET) else {
            panic!("{BUCKET} is not a declared bucket");
        };

        assert!(size_hint().contains(&phonix_core::files::human_size(policy.max_bytes)));
    }
}
