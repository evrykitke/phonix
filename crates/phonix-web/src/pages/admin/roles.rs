//! The roles this workspace has defined, and what each grants.
//!
//! A role is a named bundle of permissions that reaches everybody holding it,
//! immediately - permissions are resolved per request, not frozen into a
//! session. That is what makes a role the right tool for "everybody in
//! accounts" and the individual editor the right tool for "everybody in
//! accounts except Sam".
//!
//! # Three screens, and why the middle one has tabs
//!
//! ```text
//! /admin/roles            the list        a grid
//! /admin/roles/new        define one      a form
//! /admin/roles/:id        one role        Details | Permissions
//! ```
//!
//! A role is two different things being edited by two different people at two
//! different moments: what it is *called*, which is settled once, and what it
//! *grants*, which is revisited. Putting both on one page meant one save button
//! for two decisions - and a permission tree with a name box balanced on top of
//! it, which is what made the screen feel like a form that had grown a tree
//! rather than a screen about a role.
//!
//! Tabs also give each half its own save. The details form is the kit's
//! [`EntityForm`]; the tree keeps its own, because what it submits is a set
//! rather than a draft, and its "unsaved, and this reaches N people" line is
//! the whole point of the button.
//!
//! # What is deliberately not offered
//!
//! Editing `Admin`. It holds the whole tree by definition and is rewritten from
//! the compiled definitions on every deploy, so a change made here would revert
//! at the next release - worse than being refused.

use leptos::prelude::*;
use leptos_meta::Title;
use phonix_core::audit::kinds;
use phonix_core::authorization::{PermissionSet, RoleDetail, RoleInput, roles as static_roles};
use phonix_core::permissions;
use uuid::Uuid;

use crate::components::history::RecordHistory;
use crate::components::page::{
    Badge, FormActions, GhostButton, Notice, PageHeader, Panel, PrimaryButton, Tone,
};
use crate::components::permission_tree::PermissionTree;
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::server_fns::admin_fns::{role_detail, role_permissions, save_role_permissions};
use crate::ui::alert::{Alert, Alerts};
use crate::ui::form::config::roles::{edit_role_form, new_role_form};
use crate::ui::form::{EntityForm, FormHost};
use crate::ui::table::DataGrid;
use crate::ui::table::config::roles::roles_grid;
use crate::ui::tabs::{Tab, TabbedPanel};

#[component]
pub fn roles_page() -> impl IntoView {
    view! {
        // "Phonix" is the product's name, not a word.
        <Title text=format!("{} | Phonix", l!("roles.title")) />

        <PageHeader
            title=l!("roles.title")
            subtitle=l!("roles.subtitle")
            icon=Icon::ShieldCheck
        />

        <DataGrid config=roles_grid() />
    }
}

/// Defining a role.
///
/// A page rather than a dialog over the list, for the same reason the invite
/// screen is: what happens next is a second step - choosing what the role
/// grants - and a dialog that closes onto a list has nowhere to send somebody.
#[component]
pub fn role_new_page() -> impl IntoView {
    view! {
        <Title text=format!("{} | Phonix", l!("roles.new.title")) />

        <PageHeader
            title=l!("roles.new.title")
            subtitle=l!("roles.new.subtitle")
            icon=Icon::ShieldCheck
            back=("/admin/roles", l!("roles.title"))
        />

        // A single-column form, so the card ends where it ends rather than
        // stretching a name and a description across a wide monitor.
        <div class="max-w-3xl">
            <Panel>
                <EntityForm config=new_role_form() value=RoleInput::blank() />
            </Panel>
        </div>
    }
}

/// One role: what it is called, and what it grants.
#[component]
pub fn role_page() -> impl IntoView {
    let params = leptos_router::hooks::use_params_map();
    let role_id = move || params.with(|params| params.get("id").unwrap_or_default());

    // Two resources rather than one call that fetches both: a resource starts
    // as soon as it is created, so these are already in flight together and
    // awaiting them in the same block is the join. They are also two different
    // questions - what the role is, and what it grants - saved by two different
    // buttons, so keeping them apart is what lets one tab re-read without the
    // other losing an unsaved tick.
    let details = Resource::new(role_id, |raw| async move {
        match raw.parse::<Uuid>() {
            Ok(role_id) => role_detail(role_id).await,
            Err(_) => Err(ServerFnError::new("That is not a role id.")),
        }
    });

    let grants = Resource::new(role_id, |raw| async move {
        match raw.parse::<Uuid>() {
            Ok(role_id) => role_permissions(role_id).await,
            Err(_) => Err(ServerFnError::new("That is not a role id.")),
        }
    });

    view! {
        <Title text=format!("{} | Phonix", l!("entity.role.singular")) />

        // Transition, not Suspense: navigating from one role to another
        // re-suspends, and a fallback here would blank a screen somebody is
        // looking at rather than replacing it when the next one arrives.
        <Transition fallback=|| {
            view! { <p class="text-sm text-content-subtle">{l!("common.loading")}</p> }
        }>
            {move || Suspend::new(async move {
                match (details.await, grants.await) {
                    (Ok(details), Ok(grants)) => {
                        view! { <RoleEditor details=details grants=grants /> }.into_any()
                    }
                    // Either failure is the same failure to this screen, and
                    // the server's own words say more than a house phrase.
                    (Err(err), _) | (_, Err(err)) => {
                        view! {
                            <>
                                <PageHeader
                                    title=l!("entity.role.singular")
                                    icon=Icon::ShieldCheck
                                    back=("/admin/roles", l!("roles.title"))
                                />
                                <Notice
                                    message=Signal::derive(move || Some(err.to_string()))
                                    tone=Tone::Danger
                                />
                            </>
                        }
                            .into_any()
                    }
                }
            })}
        </Transition>
    }
}

#[component]
fn role_editor(details: RoleInput, grants: RoleDetail) -> impl IntoView {
    let role_id = grants.summary.id;
    let display_name = grants.summary.display_name.clone();
    let user_count = grants.summary.user_count;
    let is_static = grants.summary.is_static;
    let is_default = grants.summary.is_default;

    // `Admin` holds the whole tree by definition and is rewritten from the
    // compiled definitions on every deploy. Editing it here would produce a
    // change that silently reverts at the next release.
    let is_admin = grants
        .summary
        .name
        .eq_ignore_ascii_case(static_roles::ADMIN);

    // Hoisted above the tab strip rather than created inside the tab that uses
    // them. A render closure runs again every time its tab comes back on
    // screen, so state declared inside one is state that is thrown away by
    // looking at the other tab - which here would be an unsaved selection.
    //
    // The details are hoisted for the mirror reason: the form owns its draft
    // while it is on screen, but the value it *re-opens* on comes from here,
    // and a saved change that was not written back would reappear as the old
    // one the moment somebody looked at the permissions tab and came back.
    let details = RwSignal::new(details);

    let stored = RwSignal::new(grants.permissions.clone());
    let selection = RwSignal::new(grants.permissions.clone());
    let pending = RwSignal::new(false);

    // Both tabs report the same way, which is the point of the channel being a
    // property of the screen rather than of each button: the details form is
    // configured `Channel::MessageBox`, and the tree - which is not an
    // `EntityForm` and so has no configuration to read - posts the same shape
    // of alert by hand.
    let alerts = Alerts::get();

    let dirty = move || selection.get() != stored.get();

    let save = Action::new(move |permissions: &PermissionSet| {
        let permissions = permissions.clone();
        async move { save_role_permissions(role_id, permissions).await }
    });

    Effect::new(move |_| {
        let Some(result) = save.value().get() else {
            return;
        };
        pending.set(false);

        match result {
            Ok(updated) => {
                // From what was stored, not from what was submitted: the server
                // pulls ancestors in and declines redundant grants, and showing
                // the draft would leave ticks on screen that are not in the
                // database.
                let count = updated.permissions.len();

                selection.set(updated.permissions.clone());
                stored.set(updated.permissions);

                // What was stored, said back. "Saved" alone leaves the reader
                // to work out whether the ancestors the server pulled in
                // actually landed.
                let permissions = if count == 1 {
                    "permission"
                } else {
                    "permissions"
                };
                let holders = if user_count == 1 { "person" } else { "people" };

                alerts.post(
                    Alert::success(format!(
                        "This role now grants {count} {permissions}, and the change has \
                             reached the {user_count} {holders} holding it.",
                    ))
                    .titled("Permissions saved")
                    .message_box(),
                );
            }
            Err(err) => {
                alerts.post(Alert::failure(err.to_string()).message_box());
            }
        }
    });

    // What `Then::Refresh` on the details form runs. Re-read rather than the
    // draft echoed back: the database declines a rename on a built-in role, so
    // the only honest answer to "what is stored now" is to ask.
    let reread = FormHost {
        refresh: Some(Callback::new(move |()| {
            leptos::task::spawn_local(async move {
                if let Ok(fresh) = role_detail(role_id).await {
                    details.set(fresh);
                }
            });
        })),
        close: None,
    };

    let details_tab = Tab::new("details", "Details", move || {
        view! {
            <div class="space-y-3">
                <Show when=move || is_static fallback=|| ()>
                    <BuiltInNote />
                </Show>

                <div class="max-w-3xl">
                    <Panel>
                        <EntityForm
                            config=edit_role_form(is_static)
                            value=details.get_untracked()
                            host=reread
                        />
                    </Panel>
                </div>
            </div>
        }
        .into_any()
    })
    .icon(Icon::SlidersHorizontal);

    let permissions_tab = Tab::new("permissions", "Permissions", move || {
        view! {
            <div class="space-y-3">
                <Show when=move || is_admin fallback=|| ()>
                    <Panel>
                        <div class="flex items-start gap-2 text-sm text-content-muted">
                            <span class="mt-0.5 shrink-0 text-warning">
                                <Icon icon=Icon::Lock size=IconSize::Xs />
                            </span>
                            <span>
                                "The Admin role always holds every permission, including ones \
                                 added by a future release. It is rewritten from the compiled \
                                 definitions on every deploy, so an edit here would not survive \
                                 one."
                            </span>
                        </div>
                    </Panel>
                </Show>

                <PermissionTree
                    selection=selection
                    disabled=Signal::derive(move || is_admin)
                />

                <Show when=move || !is_admin fallback=|| ()>
                    <div class="rounded-card border border-edge bg-surface-raised">
                        <FormActions>
                            <span class="mr-auto text-xs text-content-subtle">
                                {move || {
                                    if dirty() {
                                        crate::lp!("roles.unsaved_people", user_count)
                                    } else {
                                        l!("permissions.saved")
                                    }
                                }}
                            </span>
                            <GhostButton
                                label=l!("permissions.revert")
                                icon=Icon::RefreshCw
                                disabled=Signal::derive(move || !dirty())
                                on_click=Callback::new(move |()| {
                                    selection.set(stored.get_untracked());
                                })
                            />
                            <PrimaryButton
                                label=l!("permissions.save")
                                icon=Icon::Save
                                pending=Signal::derive(move || pending.get())
                                disabled=Signal::derive(move || !dirty())
                                on_click=Callback::new(move |()| {
                                    pending.set(true);
                                    save.dispatch(selection.get_untracked());
                                })
                            />
                        </FormActions>
                    </div>
                </Show>
            </div>
        }
        .into_any()
    })
    .icon(Icon::KeySquare);

    // A tab rather than a third panel under the details form. The page is
    // already two screens of permission tree; a history stacked below it would
    // be a section nobody scrolls to, and one that reloads every time the tree
    // is saved.
    let history_tab = Tab::new("history", "History", move || {
        view! { <RecordHistory kind=kinds::ROLE id=Some(role_id.to_string()) /> }.into_any()
    })
    .icon(Icon::Clock)
    .require(permissions::AUDIT_LOGS);

    view! {
        <PageHeader
            title=display_name
            icon=Icon::ShieldCheck
            back=("/admin/roles", l!("roles.title"))
        >
            <div class="flex flex-wrap items-center gap-1.5">
                {is_static
                    .then(|| view! { <Badge label=l!("roles.built_in") tone=Tone::Brand /> })}
                {is_default.then(|| view! { <Badge label=l!("roles.default") /> })}
                <Badge label=crate::lp!("roles.people", user_count) />
            </div>
        </PageHeader>

        <TabbedPanel id="role" tabs=vec![details_tab, permissions_tab, history_tab] />
    }
}

/// What can and cannot be changed about a role that ships with the product.
#[component]
fn built_in_note() -> impl IntoView {
    view! {
        <Panel>
            <div class="flex items-start gap-2 text-sm text-content-muted">
                <span class="mt-0.5 shrink-0 text-warning">
                    <Icon icon=Icon::Lock size=IconSize::Xs />
                </span>
                <span>
                    "This role ships with the product. Its key is what code assigns it by and \
                     cannot change, and it cannot be deleted. Its label, description and \
                     default flag are yours to edit."
                </span>
            </div>
        </Panel>
    }
}
