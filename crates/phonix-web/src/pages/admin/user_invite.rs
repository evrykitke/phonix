//! Adding somebody to this workspace.
//!
//! # Why this is a page and not a modal over the grid
//!
//! The design note called for a quick-add modal on the users list, and for most
//! entities that is right. An invitation is the case where it is not: what comes
//! back is a **single-use link that is shown once and never again**, and that
//! output needs somewhere it can be read, selected and copied without a table
//! scrolling underneath it.
//!
//! A modal would also have to decide what closing means while a link is on
//! screen - and every answer is wrong, because closing is how the link is lost.
//!
//! # The link is shown when there is something to say about it
//!
//! When the relay delivered the message, the link is secondary: the person will
//! get it. When there is no relay, or the relay refused, the link is the only
//! way the invitation reaches anybody, and it is presented as the thing to act
//! on. `InvitationIssued::was_emailed` is what decides which.

use leptos::prelude::*;
use leptos_meta::Title;
use phonix_core::identity::InvitationIssued;

use crate::components::page::{Notice, PageHeader, Panel, Tone};
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::lp;
use crate::server_fns::admin_fns::assignable_roles;
use crate::ui::clipboard;
use crate::ui::form::EntityForm;
use crate::ui::form::config::invitations::invite_form;

#[component]
pub fn user_invite_page() -> impl IntoView {
    let roles = Resource::new(|| (), |()| assignable_roles());

    // What the last send produced. Kept here rather than in the form, because
    // the form edits a `UserInvite` and has nowhere to put a link.
    let issued = RwSignal::new(None::<InvitationIssued>);
    let on_issued = Callback::new(move |result: InvitationIssued| issued.set(Some(result)));

    view! {
        <Title text=format!("{} | Phonix", l!("invite.title")) />

        <PageHeader
            title=l!("invite.title")
            subtitle=l!("invite.subtitle")
            icon=Icon::UserPlus
            back=("/admin/users", l!("users.title"))
        />

        <div class="grid gap-3 lg:grid-cols-2">
            <Transition fallback=|| {
                view! { <p class="text-sm text-content-subtle">"Loading..."</p> }
            }>
                {move || Suspend::new(async move {
                    match roles.await {
                        Ok(roles) => {
                            view! {
                                <Panel title=l!("invite.panel")>
                                    <EntityForm
                                        config=invite_form(roles, on_issued)
                                        value=phonix_core::identity::UserInvite::blank()
                                    />
                                </Panel>
                            }
                                .into_any()
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
            </Transition>

            {move || issued.get().map(|issued| view! { <Issued issued=issued /> })}
        </div>
    }
}

/// What the server minted, shown once.
#[component]
fn issued(issued: InvitationIssued) -> impl IntoView {
    let emailed = issued.was_emailed();
    let link = issued.link.clone();
    let copied = RwSignal::new(false);

    let note = issued.delivery_note.clone();
    let display_name = issued.display_name.clone();
    let email = issued.email.clone();
    let hours = issued.expires_in_hours;

    let to_copy = link.clone();

    view! {
        <Panel title=l!("invite.sent")>
            <div class="space-y-3">
                // One sentence, not three fragments around two styled
                // spans. Where a name and an address sit relative to each
                // other is grammar, and splitting the sentence to bold one of
                // them takes that decision away from the translator.
                <p class="text-sm text-content">
                    {l!("invite.added_as", name = display_name, email = email)}
                </p>

                // Only when there is something to say. A screen that reports a
                // successful send after every action is one nobody reads.
                {note
                    .map(|note| {
                        view! {
                            <Notice
                                message=Signal::derive(move || Some(note.clone()))
                                tone=Tone::Warning
                            />
                        }
                    })}

                <div class="space-y-1.5">
                    <p class="text-xs font-medium text-content">
                        {if emailed {
                            l!("invite.link_also")
                        } else {
                            l!("invite.link_only")
                        }}
                    </p>

                    // Selectable, wrapping, and in a monospace face: this gets
                    // copied by hand more often than by button.
                    <code class="block break-all rounded-control border border-edge bg-surface-sunken px-3 py-2 text-xs text-content">
                        {link}
                    </code>

                    <button
                        type="button"
                        class="inline-flex h-7 items-center gap-1.5 rounded-control border border-edge px-2.5 text-xs text-content-muted hover:bg-surface-hover hover:text-content"
                        on:click=move |_| {
                            clipboard::copy(&to_copy);
                            copied.set(true);
                        }
                    >
                        <Icon icon=Icon::Copy size=IconSize::Xs />
                        {move || if copied.get() { l!("invite.copied") } else { l!("invite.copy_link") }}
                    </button>
                </div>

                <p class="text-xs text-content-subtle">
                    {lp!("invite.link_expiry", hours)} " "
                    {l!("invite.link_once")}
                </p>
            </div>
        </Panel>
    }
}
