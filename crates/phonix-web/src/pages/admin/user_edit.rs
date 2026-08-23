//! Editing one account.
//!
//! The screen is a heading and a form, the same way the users list is a heading
//! and a grid. Everything about *what* may be edited - the controls, which
//! permission each one needs, what is required, where the save goes and what
//! happens afterwards - is a configuration in
//! [`crate::ui::form::config::users`]. What is left here is the part that is
//! genuinely about this page: resolving the account and the roles before the
//! configuration can be built.
//!
//! # Why two things are resolved and not one
//!
//! A form configuration is built during a render and cannot await anything, so
//! the select's options have to exist before `user_form` is called. Fetching
//! them inside the field would draw an empty select and fill it in a moment
//! later, which quietly discards a choice made in that moment.
//!
//! Two resources rather than one that fetches both: a resource starts as soon
//! as it is created, so these two requests are already in flight together and
//! awaiting them in the same block is the join. Neither answer is needed to ask
//! for the other.

use leptos::prelude::*;
use leptos_meta::Title;
use phonix_core::audit::kinds;
use phonix_core::identity::UserId;

use crate::components::history::RecordHistory;
use crate::components::page::{Notice, PageHeader, Panel, Tone};
use crate::icons::Icon;
use crate::l;
use crate::server_fns::admin_fns::{assignable_roles, user_edit};
use crate::ui::form::EntityForm;
use crate::ui::form::config::users::user_form;

#[component]
pub fn user_edit_page() -> impl IntoView {
    let params = leptos_router::hooks::use_params_map();

    // Re-resolved on navigation rather than read once: the route can change
    // under this component when the command palette sends you to another
    // account.
    let account = Resource::new(
        move || params.with(|params| params.get("id").unwrap_or_default()),
        |raw| async move {
            let Ok(user_id) = raw.parse::<UserId>() else {
                return Err(ServerFnError::new("That is not a user id."));
            };

            user_edit(user_id).await
        },
    );

    // The workspace's roles do not depend on which account is open, so this one
    // is fetched once and not re-fetched on navigation.
    let roles = Resource::new(|| (), |()| assignable_roles());

    // The raw parameter rather than the loaded account: the history section is
    // outside the transition, so it must not wait for the form to arrive
    // before it knows which record it is on.
    let record = Signal::derive(move || {
        params
            .with(|params| params.get("id"))
            .filter(|id| !id.is_empty())
    });

    view! {
        <Title text=format!("{} | Phonix", l!("users.edit")) />

        <PageHeader
            title=l!("users.edit")
            icon=Icon::Pencil
            back=("/admin/users", l!("users.title"))
        />

        // Capped rather than full-bleed: the card should end where the form
        // ends, or a wide monitor draws a metre of empty panel beside eight
        // fields.
        <div class="max-w-5xl space-y-4">
        // Transition, not Suspense: re-navigating between two accounts
        // re-suspends, and a fallback here would blank a form somebody is
        // looking at rather than replacing it when the next one arrives.
        <Transition fallback=|| {
            view! { <p class="text-sm text-content-subtle">"Loading..."</p> }
        }>
            {move || Suspend::new(async move {
                match (account.await, roles.await) {
                    (Ok(account), Ok(roles)) => {
                        view! {
                            // `Panel` supplies the padding - the second `p-4`
                            // that used to be here doubled it.
                            <Panel>
                                <EntityForm config=user_form(roles) value=account />
                            </Panel>
                        }
                            .into_any()
                    }
                    // Either failure is the same failure to this screen: the
                    // form cannot be built, and the server's own words say more
                    // than a house phrase would.
                    (Err(err), _) | (_, Err(err)) => {
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

        // Outside the transition, so switching accounts does not blank it -
        // and below the form, because the form is what somebody opened this
        // page to use.
        <RecordHistory kind=kinds::USER id=record />
        </div>
    }
}
