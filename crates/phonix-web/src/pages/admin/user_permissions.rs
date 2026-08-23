//! What one account may do, and how to change it.
//!
//! The screen shows the whole permission tree with the account's *effective*
//! set ticked - what its roles give, plus individual grants, minus individual
//! denials. Each row says where its state came from, because unticking a box
//! means different things depending on the answer:
//!
//! ```text
//! "from role"            untick => this account, alone, is denied it
//! "granted directly"     untick => the individual grant goes away
//! "denied for this user" tick   => the denial goes away
//! (nothing)              tick   => granted to this account alone
//! ```
//!
//! Saving submits the whole ticked set. The server works out which rows to
//! write from what is stored *now*, so two administrators on this screen at
//! once cannot silently undo each other - see
//! [`crate::server_fns::admin_fns`].

use leptos::prelude::*;
use leptos_meta::Title;
use phonix_core::authorization::{PermissionSet, UserPermissionView};
use phonix_core::identity::UserId;

use crate::components::page::{
    Badge, FormActions, GhostButton, Notice, PageHeader, Panel, PrimaryButton, Tone,
};
use crate::components::permission_tree::PermissionTree;
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::server_fns::admin_fns::{save_user_permissions, user_permissions};

#[component]
pub fn user_permissions_page() -> impl IntoView {
    let params = leptos_router::hooks::use_params_map();

    // Re-resolved on navigation rather than read once: the route can change
    // under this component when the palette sends you to another account.
    let view_data = Resource::new(
        move || params.with(|params| params.get("id").unwrap_or_default()),
        |raw| async move {
            let Ok(user_id) = raw.parse::<UserId>() else {
                return Err(ServerFnError::new("That is not a user id."));
            };
            user_permissions(user_id).await
        },
    );

    view! {
        <Title text=format!("{} | Phonix", l!("permissions.title")) />

        <Suspense fallback=|| {
            view! { <p class="text-sm text-content-subtle">"Loading..."</p> }
        }>
            {move || Suspend::new(async move {
                match view_data.await {
                    Ok(loaded) => view! { <Editor loaded=loaded /> }.into_any(),
                    Err(err) => {
                        view! {
                            <>
                                <PageHeader
                                    title=l!("permissions.title")
                                    icon=Icon::KeySquare
                                    back=("/admin/users", l!("users.title"))
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
        </Suspense>
    }
}

#[component]
fn editor(loaded: UserPermissionView) -> impl IntoView {
    let user_id = loaded.user_id;
    let editable = loaded.is_editable();
    let display_name = loaded.display_name.clone();
    let email = loaded.email.clone();
    let roles = loaded.roles.clone();

    // The server's answer, kept apart from the edit buffer so "Revert" has
    // something to go back to and so annotations describe what is *stored*
    // rather than what is currently ticked.
    let stored = RwSignal::new(loaded);
    let selection = RwSignal::new(stored.with_untracked(UserPermissionView::effective));

    let notice = RwSignal::new(None::<String>);
    let failed = RwSignal::new(None::<String>);
    let pending = RwSignal::new(false);

    let dirty = move || selection.get() != stored.with(UserPermissionView::effective);

    let save = Action::new(move |permissions: &PermissionSet| {
        let permissions = permissions.clone();
        async move { save_user_permissions(user_id, permissions).await }
    });

    Effect::new(move |_| {
        let Some(result) = save.value().get() else {
            return;
        };
        pending.set(false);

        match result {
            Ok(updated) => {
                // Re-seeded from the server's answer, not from what was sent:
                // the server pulls in ancestors and declines to store grants a
                // role already gives, and the screen should show that.
                selection.set(updated.effective());
                stored.set(updated);
                failed.set(None);
                notice.set(Some("Permissions saved.".to_owned()));
            }
            Err(err) => {
                notice.set(None);
                failed.set(Some(err.to_string()));
            }
        }
    });

    let annotate = Callback::new(move |name: &'static str| {
        stored.with(|stored| stored.source(name).label().map(str::to_owned))
    });

    view! {
        <PageHeader
            title=display_name.clone()
            subtitle=email
            icon=Icon::KeySquare
            back=("/admin/users", l!("users.title"))
        />

        <div class="space-y-3">
            <Notice message=Signal::derive(move || failed.get()) tone=Tone::Danger />
            <Notice message=Signal::derive(move || notice.get()) tone=Tone::Success />

            <Panel
                title=l!("field.roles")
                description=l!("permissions.inherited")
            >
                <div class="flex flex-wrap gap-1">
                    {if roles.is_empty() {
                        view! {
                            <span class="text-sm text-content-subtle">{l!("permissions.no_roles")}</span>
                        }
                            .into_any()
                    } else {
                        roles
                            .iter()
                            .map(|role| view! { <Badge label=role.clone() tone=Tone::Brand /> })
                            .collect::<Vec<_>>()
                            .into_any()
                    }}
                </div>
            </Panel>

            <Show when=move || !editable fallback=|| ()>
                <div class="flex items-start gap-2 rounded-control border border-edge bg-surface-sunken px-3 py-2 text-sm text-content-muted">
                    <span class="mt-0.5 shrink-0 text-warning">
                        <Icon icon=Icon::Lock size=IconSize::Xs />
                    </span>
                    <span>
                        {l!("permissions.owner_locked")}
                    </span>
                </div>
            </Show>

            <PermissionTree
                selection=selection
                disabled=Signal::derive(move || !editable)
                annotate=annotate
            />

            <Show when=move || editable fallback=|| ()>
                <div class="rounded-card border border-edge bg-surface-raised">
                    <FormActions>
                        <span class="mr-auto text-xs text-content-subtle">
                            {move || {
                                if dirty() {
                                    l!("permissions.unsaved")
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
                                selection.set(stored.with(UserPermissionView::effective));
                                notice.set(None);
                                failed.set(None);
                            })
                        />
                        <PrimaryButton
                            label=l!("permissions.save")
                            icon=Icon::Save
                            pending=Signal::derive(move || pending.get())
                            disabled=Signal::derive(move || !dirty())
                            on_click=Callback::new(move |()| {
                                pending.set(true);
                                notice.set(None);
                                save.dispatch(selection.get_untracked());
                            })
                        />
                    </FormActions>
                </div>
            </Show>

            <p class="text-xs text-content-subtle">
                {l!("permissions.denial_note")}
            </p>

            <Summary stored=stored />
        </div>
    }
}

/// What is stored against this account, apart from its roles.
///
/// Worth showing separately: the tree renders the *result*, and "why is this
/// person different from everybody else with the same role" is a question the
/// result cannot answer.
#[component]
fn summary(stored: RwSignal<UserPermissionView>) -> impl IntoView {
    view! {
        <Panel
            title=l!("permissions.overrides")
            description=l!("permissions.overrides_detail")
        >
            {move || {
                stored
                    .with(|stored| {
                        if stored.overrides.is_empty() {
                            return view! {
                                <p class="text-sm text-content-subtle">
                                    {l!("permissions.no_overrides")}
                                </p>
                            }
                                .into_any();
                        }

                        let granted: Vec<String> = stored
                            .overrides
                            .granted
                            .iter()
                            .map(str::to_owned)
                            .collect();
                        let denied: Vec<String> = stored
                            .overrides
                            .denied
                            .iter()
                            .map(str::to_owned)
                            .collect();

                        view! {
                            <div class="space-y-3">
                                <OverrideList
                                    title=l!("permissions.granted_directly")
                                    names=granted
                                    tone=Tone::Success
                                    icon=Icon::Plus
                                />
                                <OverrideList
                                    title=l!("permissions.denied")
                                    names=denied
                                    tone=Tone::Danger
                                    icon=Icon::Ban
                                />
                            </div>
                        }
                            .into_any()
                    })
            }}
        </Panel>
    }
}

#[component]
fn override_list(
    #[prop(into)] title: String,
    names: Vec<String>,
    tone: Tone,
    icon: Icon,
) -> impl IntoView {
    if names.is_empty() {
        return ().into_any();
    }

    view! {
        <div>
            <div class="mb-1 flex items-center gap-1.5 text-xs font-medium uppercase tracking-wide text-content-subtle">
                <Icon icon=icon size=IconSize::Xs />
                {title}
            </div>
            <div class="flex flex-wrap gap-1">
                {names
                    .into_iter()
                    .map(|name| view! { <Badge label=name tone=tone /> })
                    .collect::<Vec<_>>()}
            </div>
        </div>
    }
    .into_any()
}
