//! Tenant dashboard - proves the host -> tenant -> database -> cache path.

use leptos::prelude::*;
use leptos_meta::Title;

use crate::l;
use crate::server_fns::tenant_fns::tenant_user_count;

#[component]
pub fn dashboard_page() -> impl IntoView {
    let count = OnceResource::new(tenant_user_count());

    view! {
        <Title text=format!("{} | Phonix", l!("dashboard.title")) />

        <section class="space-y-6">
            <h1 class="text-2xl font-semibold tracking-tight text-content">
                {l!("dashboard.title")}
            </h1>

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
