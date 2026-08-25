//! The bar across the top: where you are, and the ways out of it.
//!
//! The breadcrumb is read straight off the [`Trail`](crate::navigation::Trail),
//! so a new screen gets one by being in the menu. Nothing here is per-page, and
//! nothing here has to be.

use leptos::prelude::*;

use super::Shell;
use super::app_launcher::AppLauncher;
use super::user_menu::UserMenu;
use crate::components::tenant_badge::TenantBadge;
use crate::i18n::t;
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::navigation::{MENU, find};

#[component]
pub fn top_bar() -> impl IntoView {
    let shell = Shell::get();

    view! {
        <header class="flex h-topbar shrink-0 items-center gap-2 border-b border-edge bg-surface-shell px-2 sm:px-3">
            // The only way to reach the menu below `md`, which is why it is
            // first: it is the control that stands in for the whole panel.
            <button
                type="button"
                class="grid size-7 shrink-0 place-items-center rounded-control text-content-muted hover:bg-surface-hover hover:text-content md:hidden"
                on:click=move |_| shell.toggle_drawer()
                aria-label=l!("nav.open")
                aria-expanded=move || if shell.drawer_open.get() { "true" } else { "false" }
            >
                <Icon icon=Icon::Menu size=IconSize::Sm />
            </button>

            <Breadcrumb />

            <div class="ml-auto flex items-center gap-1.5">
                <SearchTrigger />

                <span class="hidden md:inline-flex">
                    <TenantBadge />
                </span>

                // Before the bell and the avatar: this is a "where am I"
                // control, and those two are about what has happened and who
                // you are. Grouping it with the search keeps the two questions
                // about *place* next to each other.
                <AppLauncher />

                <button
                    type="button"
                    class="grid size-7 place-items-center rounded-control text-content-muted hover:bg-surface-hover hover:text-content"
                    aria-label=l!("nav.notifications")
                    title=l!("nav.notifications")
                >
                    <Icon icon=Icon::Bell size=IconSize::Sm />
                </button>

                <UserMenu />
            </div>

            // Announced to screen readers on navigation; the visual breadcrumb
            // is decorative next to this.
            <div class="sr-only" aria-live="polite">
                {move || shell.trail.with(|trail| trail.current().map(label_for).unwrap_or_default())}
            </div>
        </header>
    }
}

/// Where you are, root first.
#[component]
fn breadcrumb() -> impl IntoView {
    let shell = Shell::get();

    view! {
        <nav class="hidden min-w-0 items-center sm:flex" aria-label=l!("nav.breadcrumb")>
            <ol class="flex min-w-0 items-center gap-1 text-sm">
                {move || {
                    let keys = shell.trail.with(|trail| trail.keys().to_vec());

                    if keys.is_empty() {
                        return view! {
                            // The product's name, not a word.
                            <li class="truncate-fade text-content-muted">"Phonix"</li>
                        }
                            .into_any();
                    }

                    let last = keys.len() - 1;

                    keys.into_iter()
                        .enumerate()
                        .map(|(index, key)| {
                            let is_last = index == last;

                            view! {
                                <li class="flex min-w-0 items-center gap-1">
                                    <Show when=move || { index > 0 } fallback=|| ()>
                                        <span class="text-content-subtle" aria-hidden="true">
                                            <Icon icon=Icon::ChevronRight size=IconSize::Xs />
                                        </span>
                                    </Show>
                                    <span
                                        class=if is_last {
                                            "truncate-fade font-medium text-content"
                                        } else {
                                            "truncate-fade text-content-muted"
                                        }
                                        aria-current=is_last.then_some("page")
                                    >
                                        {label_for(key)}
                                    </span>
                                </li>
                            }
                        })
                        .collect::<Vec<_>>()
                        .into_any()
                }}
            </ol>
        </nav>
    }
}

/// The button that opens the command palette, and the shortcut that does the
/// same thing without it.
#[component]
fn search_trigger() -> impl IntoView {
    let shell = Shell::get();

    view! {
        <button
            type="button"
            class="grid size-7 place-items-center rounded-control text-content-muted hover:bg-surface-hover hover:text-content sm:flex sm:size-auto sm:h-7 sm:items-center sm:gap-2 sm:border sm:border-edge sm:bg-surface sm:px-2 sm:text-content-subtle sm:hover:border-edge-strong sm:hover:bg-transparent sm:hover:text-content-muted"
            on:click=move |_| shell.open_palette()
            aria-label=l!("palette.title")
            title=l!("palette.title")
        >
            // One icon, not one per breakpoint: `Icon` already carries
            // `inline-block`, and whether that or a `hidden` added here wins is
            // decided by the order Tailwind emits them, not by the order they
            // are written. Two icons was two icons.
            <Icon icon=Icon::Search size=IconSize::Sm />
            <span class="hidden text-xs sm:inline">{l!("palette.search")}</span>
            <kbd class="hidden rounded border border-edge bg-surface-sunken px-1 font-mono text-2xs text-content-subtle sm:inline">
                // Not a sentence: the two key caps a keyboard is engraved
                // with. They read the same in every language this ships in.
                "Ctrl K"
            </kbd>
        </button>
    }
}

/// The menu label for a node key, for the breadcrumb.
///
/// A key with no node is not an error worth showing anyone: the menu changed
/// under a URL, and an empty crumb is the honest render.
fn label_for(key: &'static str) -> String {
    find(MENU, key)
        .map(|node| t(&node.label()))
        .unwrap_or_default()
}
