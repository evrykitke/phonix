//! Landing page.

use crate::l;
use leptos::prelude::*;
use leptos_meta::Title;

#[component]
pub fn home_page() -> impl IntoView {
    view! {
        <Title text="Phonix" />

        <section class="space-y-6">
            <h1 class="text-3xl font-semibold tracking-tight text-content">"Phonix"</h1>
            <p class="max-w-2xl text-content-muted">
                {l!("home.blurb")}
            </p>

            <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
                <StackCard name="Leptos" detail=l!("home.stack.leptos") />
                <StackCard name="PostgreSQL" detail=l!("home.stack.postgres") />
                <StackCard name="Redis" detail=l!("home.stack.redis") />
                <StackCard name="RabbitMQ" detail=l!("home.stack.rabbitmq") />
            </div>
        </section>
    }
}

#[component]
fn stack_card(name: &'static str, #[prop(into)] detail: String) -> impl IntoView {
    view! {
        <div class="rounded-lg border border-edge bg-surface-raised p-4">
            <div class="font-medium text-content">{name}</div>
            <div class="text-sm text-content-subtle">{detail}</div>
        </div>
    }
}
