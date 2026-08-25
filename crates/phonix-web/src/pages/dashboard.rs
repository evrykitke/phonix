//! Tenant dashboard - proves the host -> tenant -> database -> cache path.
//!
//! # Why the app store shows up here
//!
//! A workspace switched on for the first time has core and nothing else: no
//! Sales in the menu, no master data, one number on a card. Somebody landing on
//! that has no way of knowing there is anything more, and the grid in the top
//! bar is a control you have to already be looking for.
//!
//! So the first screen after signing in says what to do next, and stops saying
//! it the moment there is anything switched on. It is not a permanent panel and
//! must not become one - see `[[the-real-estate-rule]]`: a card that is right
//! on day one and noise on day two is noise.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::A;
use phonix_core::apps;
use phonix_core::authorization::names;

use crate::components::shell::app_launcher::APPS_HREF;
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::server_fns::app_fns::enabled_apps;
use crate::server_fns::tenant_fns::tenant_user_count;
use crate::ui::viewer::Viewer;

#[component]
pub fn dashboard_page() -> impl IntoView {
    let count = OnceResource::new(tenant_user_count());

    view! {
        <Title text=format!("{} | Phonix", l!("dashboard.title")) />

        <section class="space-y-6">
            <h1 class="text-2xl font-semibold tracking-tight text-content">
                {l!("dashboard.title")}
            </h1>

            <GetStarted />

            // ErrorBoundary catches the Err arm of the server function so a
            // database outage renders a message instead of a blank page.
            <ErrorBoundary fallback=|errors| {
                view! {
                    <div class="rounded-lg border border-danger bg-danger-subtle p-4 text-sm text-danger">
                        <p class="font-medium">{l!("dashboard.load_failed")}</p>
                        <ul class="mt-2 list-disc pl-5">
                            {move || {
                                errors
                                    .get()
                                    .into_iter()
                                    .map(|(_, err)| view! { <li>{err.to_string()}</li> })
                                    .collect::<Vec<_>>()
                            }}
                        </ul>
                    </div>
                }
            }>
                <Suspense fallback=|| {
                    view! { <p class="text-content-subtle">{l!("common.loading")}</p> }
                }>
                    {move || Suspend::new(async move {
                        count
                            .await
                            .map(|count| {
                                view! {
                                    <div class="max-w-measure rounded-lg border border-edge bg-surface-raised p-6">
                                        <div class="text-sm text-content-subtle">
                                            {l!("dashboard.user_count")}
                                        </div>
                                        <div class="mt-1 text-3xl font-semibold text-content">
                                            {count}
                                        </div>
                                    </div>
                                }
                            })
                    })}
                </Suspense>
            </ErrorBoundary>
        </section>
    }
}

/// The one-time nudge towards the store.
///
/// Renders nothing at all once anything optional is on, and nothing for
/// somebody who could not act on it either - a person without the permission to
/// install is being told to go and press a button that is not there.
#[component]
fn get_started() -> impl IntoView {
    let viewer = Viewer::get();
    let enabled = OnceResource::new(async move { enabled_apps().await.unwrap_or_default() });

    view! {
        <Suspense fallback=|| ()>
            {move || Suspend::new(async move {
                let installed = enabled.await;
                let anything_on = apps::optional().any(|app| {
                    installed.iter().any(|id| id == app.id)
                });
                let may_install = viewer
                    .get()
                    .is_some_and(|user| user.can(names::APPS_INSTALL));

                (!anything_on && may_install).then(|| {
                    view! {
                        <div class="max-w-measure rounded-panel border border-edge bg-surface-raised p-5">
                            <div class="flex items-start gap-3">
                                <span class="grid size-9 shrink-0 place-items-center rounded-control bg-brand-subtle text-brand">
                                    <Icon icon=Icon::Blocks size=IconSize::Md />
                                </span>
                                <div class="min-w-0">
                                    <h2 class="text-sm font-semibold text-content">
                                        {l!("apps.get_started.title")}
                                    </h2>
                                    <p class="mt-1 text-sm leading-relaxed text-content-muted">
                                        {l!("apps.get_started.detail")}
                                    </p>
                                    <A
                                        href=APPS_HREF
                                        attr:class="mt-3 inline-flex h-8 items-center gap-1.5 rounded-control bg-brand px-3 text-sm font-medium text-on-brand hover:bg-brand-hover"
                                    >
                                        {l!("apps.get_started.action")}
                                        <Icon icon=Icon::ArrowRight size=IconSize::Xs />
                                    </A>
                                </div>
                            </div>
                        </div>
                    }
                })
            })}
        </Suspense>
    }
}
