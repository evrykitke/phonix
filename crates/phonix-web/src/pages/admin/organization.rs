//! Who this workspace legally is.
//!
//! The Organization tab of the settings screen, and its own file because it
//! loads its own data. The security policy, the mail relay and this are three
//! separate rows saved separately; putting any two in one form would mean one
//! save that half-succeeds.
//!
//! # Two panels, one read
//!
//! The details form and the logo are one `Resource` and two panels. They come
//! from the same row - `logo_file_id` is a column on `organization_profile` -
//! but they are written by different calls, for the reason set out in
//! [`crate::components::logo`]: an upload cannot happen inside a submit, and a
//! draft that carried the logo could silently revert somebody else's change.
//!
//! The signal is owned here rather than inside the logo panel so that the two
//! never disagree about what is currently set.

use leptos::prelude::*;
use phonix_core::audit::kinds;
use phonix_core::organization::OrganizationProfile;

use crate::components::history::RecordHistory;
use crate::components::logo::OrganizationLogo;
use crate::components::page::{Notice, Panel, Tone};
use crate::l;
use crate::server_fns::admin_fns::organization_profile;
use crate::ui::form::EntityForm;
use crate::ui::form::config::organization::organization_form;

#[component]
pub fn organization_tab() -> impl IntoView {
    let profile = Resource::new(|| (), |()| organization_profile());

    view! {
        <Transition fallback=|| {
            view! { <p class="text-sm text-content-subtle">"Loading..."</p> }
        }>
            {move || Suspend::new(async move {
                match profile.await {
                    Ok(profile) => view! { <Editor profile=profile /> }.into_any(),
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
        </Transition>
    }
}

#[component]
fn editor(profile: OrganizationProfile) -> impl IntoView {
    let logo = RwSignal::new(profile.logo_file_id);
    // Captured before the profile is moved into the form: the nudge is about
    // the state the screen opened in, and re-evaluating it as somebody types
    // would make it flicker away mid-sentence.
    let incomplete = !profile.is_complete();

    view! {
        // Capped to the form it contains. Seventeen fields is not a reason to
        // draw a card the width of the monitor.
        <div class="max-w-5xl space-y-3">
            {incomplete
                .then(|| {
                    view! {
                        <div class="flex items-start gap-2 rounded-control border border-edge bg-surface-sunken px-3 py-2 text-xs text-content-muted">
                            <span>
                                "This organization has no complete address yet. A registered name, \
                                 street and country are what a document needs to carry."
                            </span>
                        </div>
                    }
                })}

            <Panel
                title=l!("organization.title")
                description=l!("organization.description")
            >
                <EntityForm config=organization_form() value=profile />
            </Panel>

            <OrganizationLogo current=logo />

            // The logo lives in another table and is saved by another call,
            // but it is the same record to whoever is reading this - so its
            // changes are on this history too. See `files::set_logo`.
            <RecordHistory
                kind=kinds::ORGANIZATION
                id=Some(kinds::ORGANIZATION.singleton_id().to_owned())
            />
        </div>
    }
}
