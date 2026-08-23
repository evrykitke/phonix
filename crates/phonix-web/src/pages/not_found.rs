//! 404 page.

use leptos::prelude::*;
use leptos_meta::Title;

use crate::l;

#[component]
pub fn not_found_page() -> impl IntoView {
    // During SSR the response status is set to 404 as well, so crawlers and
    // monitoring see a real not-found rather than a 200 with error text.
    #[cfg(feature = "ssr")]
    {
        let response = expect_context::<leptos_axum::ResponseOptions>();
        response.set_status(axum::http::StatusCode::NOT_FOUND);
    }

    view! {
        <Title text=format!("{} | Phonix", l!("not_found.title")) />

        <section class="space-y-3">
            <p class="text-sm font-medium text-content-subtle">"404"</p>
            <h1 class="text-2xl font-semibold text-content">{l!("not_found.heading")}</h1>
            <a href="/" class="inline-block text-brand hover:underline">
                {l!("not_found.home")}
            </a>
        </section>
    }
}
