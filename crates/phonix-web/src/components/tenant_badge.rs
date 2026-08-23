//! Shows which tenant the current host resolved to.

use leptos::prelude::*;

use crate::l;
use crate::server_fns::tenant_fns::current_tenant;

#[component]
pub fn tenant_badge() -> impl IntoView {
    // Resolved during SSR, so the correct tenant name is in the HTML the
    // browser first receives rather than appearing after hydration.
    let tenant = OnceResource::new(current_tenant());

    view! {
        <Suspense fallback=|| {
            view! { <span class="text-sm text-content-subtle">"..."</span> }
        }>
            {move || Suspend::new(async move {
                match tenant.await {
                    Ok(tenant) => {
                        view! {
                            <span class="inline-flex items-center gap-2 rounded-full border border-edge bg-surface px-3 py-1 text-sm">
                                <span class="size-2 rounded-full bg-brand" aria-hidden="true"></span>
                                <span class="font-medium text-content">
                                    {tenant.display_name.clone()}
                                </span>
                                <span class="text-content-subtle">{tenant.slug.to_string()}</span>
                            </span>
                        }
                            .into_any()
                    }
                    Err(_) => {
                        view! {
                            <span class="text-sm text-content-subtle">{l!("tenant.absent")}</span>
                        }
                            .into_any()
                    }
                }
            })}
        </Suspense>
    }
}
