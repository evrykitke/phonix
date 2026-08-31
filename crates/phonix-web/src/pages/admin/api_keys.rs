//! The credentials other software reaches this workspace with.
//!
//! A heading, the switch that decides whether the API answers at all, and the
//! grid. Everything about the list itself is a configuration in
//! [`crate::ui::table::config::api_keys`].
//!
//! # Why the switch is on this screen and not in Settings
//!
//! It is not a security policy, which is what the settings screen holds; it is
//! whether this workspace has the API. Somebody who has come here to look at
//! keys is the person asking that question, and a screen full of working keys
//! that all return 403 - with the reason two pages away - is the confusing
//! state worth designing out.
//!
//! It is gated on `Settings` rather than on the key permissions, because it is
//! a licence rather than a grant. See `docs/adr/0002-public-api.md`.

use leptos::prelude::*;
use leptos_meta::Title;
use phonix_core::permissions;

use crate::components::page::{GhostButton, PageHeader, Panel, PrimaryButton, Tone};
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::server_fns::api_key_fns::{api_access, set_api_access};
use crate::ui::alert::{Alert, Alerts};
use crate::ui::table::DataGrid;
use crate::ui::table::config::api_keys::api_keys_grid;
use crate::ui::viewer::Viewer;

#[component]
pub fn api_keys_page() -> impl IntoView {
    view! {
        <Title text=format!("{} | Phonix", l!("api_keys.title")) />

        <PageHeader
            title=l!("api_keys.title")
            subtitle=l!("api_keys.subtitle")
            icon=Icon::KeySquare
        />

        <div class="space-y-3">
            <ApiAccess />
            <DataGrid config=api_keys_grid() />
        </div>
    }
}

/// Whether the API answers this workspace, and the button that changes it.
///
/// The state is read by everybody who can see this screen - a key that works
/// and a key that is being refused look identical on the list, so the answer
/// has to be visible - and changed only by somebody holding `Settings`.
#[component]
fn api_access() -> impl IntoView {
    let viewer = Viewer::get();
    let may_change =
        Signal::derive(move || viewer.get().is_some_and(|user| user.can(permissions::SETTINGS)));

    // The switch is a fact about the workspace, so it is fetched rather than
    // assumed. `Resource` and not a plain signal: it is read on the server
    // during the render and again by the browser, and both have to agree.
    let access = Resource::new(|| (), |()| api_access());
    let saving = RwSignal::new(false);

    let flip = move |enabled: bool| {
        saving.set(true);

        leptos::task::spawn_local(async move {
            match set_api_access(enabled).await {
                Ok(()) => {
                    Alerts::get().post(Alert::success(l!("api_keys.access.saved")));
                    access.refetch();
                }
                Err(err) => Alerts::get().post(Alert::failure(err.to_string())),
            }

            saving.set(false);
        });
    };

    view! {
        // Outside a boundary the two sides of hydration disagree about what
        // this panel says, which is an unrecoverable mismatch rather than a
        // cosmetic one.
        <Transition fallback=|| view! { <div class="h-20" /> }>
            {move || Suspend::new(async move {
                let enabled = access.await.unwrap_or(false);

                view! {
                    <Panel title=l!("api_keys.access.title")>
                        <div class="flex flex-wrap items-center justify-between gap-3">
                            <div class="min-w-0 space-y-1">
                                <p class="text-sm text-content">
                                    {if enabled {
                                        l!("api_keys.access.on")
                                    } else {
                                        l!("api_keys.access.off")
                                    }}
                                </p>

                                // Only worth offering when there is something
                                // at the other end of it.
                                {enabled
                                    .then(|| {
                                        view! {
                                            <a
                                                class="inline-flex items-center gap-1 text-xs text-brand hover:underline"
                                                href="/api/v1/docs"
                                                target="_blank"
                                                rel="noreferrer"
                                            >
                                                <Icon icon=Icon::ExternalLink size=IconSize::Xs />
                                                {l!("api_keys.access.docs")}
                                            </a>
                                        }
                                    })}
                            </div>

                            {move || {
                                may_change
                                    .get()
                                    .then(|| {
                                        if enabled {
                                            view! {
                                                <GhostButton
                                                    label=l!("api_keys.access.turn_off")
                                                    icon=Icon::ShieldOff
                                                    tone=Tone::Danger
                                                    disabled=saving
                                                    on_click=Callback::new(move |()| flip(false))
                                                />
                                            }
                                                .into_any()
                                        } else {
                                            view! {
                                                <PrimaryButton
                                                    label=l!("api_keys.access.toggle")
                                                    icon=Icon::ShieldCheck
                                                    pending=saving
                                                    on_click=Callback::new(move |()| flip(true))
                                                />
                                            }
                                                .into_any()
                                        }
                                    })
                            }}
                        </div>
                    </Panel>
                }
            })}
        </Transition>
    }
}
