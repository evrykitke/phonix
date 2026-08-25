//! The grid button in the top bar: what this workspace has, and what it could.
//!
//! # Why it is here and not in the sidebar
//!
//! The sidebar answers "where do I go inside this app". This answers "which
//! app am I in", which is a different question and one people expect at the top
//! of the window because that is where every other suite puts it.
//!
//! It shows the not-installed ones too, under their own heading. Hiding them
//! would mean the only way to find out that Books exists is to already know,
//! and a subscription business that hides its products from the people already
//! paying for one is leaving the obvious on the table. Somebody without the
//! permission to install sees the heading and no buttons - which is honest:
//! their administrator can, and now they know what to ask for.
//!
//! # Why only the ids travel
//!
//! The shell already knows which apps are on - it resolves the list once for
//! the whole page, see [`crate::apps::InstalledApps`]. Everything else - the
//! name, the summary, the icon, the route - is
//! [`phonix_core::apps::CATALOG`], compiled into this bundle. So this menu
//! costs no round trip of its own and is drawn from a constant.

use leptos::prelude::*;
use leptos::wasm_bindgen::JsCast;
use leptos::web_sys::Element;
use leptos_router::components::A;
use phonix_core::apps::{self, AppDescriptor};
use phonix_core::authorization::names;

use super::Shell;
use crate::apps::{InstalledApps, icon_of};
use crate::i18n::t;
use crate::icons::{Icon, IconSize};
use crate::l;

/// The store, and the one route that has to agree with `nav::tree`.
pub const APPS_HREF: &str = "/admin/apps";

#[component]
pub fn app_launcher() -> impl IntoView {
    let shell = Shell::get();
    let open = RwSignal::new(false);

    let enabled = InstalledApps::get();

    // The same outside-click rule the avatar menu uses, registered
    // unconditionally for the same reason: a listener that adds itself while
    // the menu opens races with the click that opened it.
    Effect::new(move |_| {
        let handle = window_event_listener(leptos::ev::click, move |event| {
            if !open.get_untracked() {
                return;
            }

            let inside = event
                .target()
                .and_then(|target| target.dyn_ref::<Element>().cloned())
                .is_some_and(|element| {
                    element
                        .closest("[data-app-launcher]")
                        .ok()
                        .flatten()
                        .is_some()
                });

            if !inside {
                open.set(false);
            }
        });

        on_cleanup(move || handle.remove());
    });

    let may_install = move || {
        shell
            .viewer()
            .get()
            .is_some_and(|user| user.can(names::APPS_INSTALL))
    };

    view! {
        <div class="relative" data-app-launcher>
            <button
                type="button"
                class="grid size-7 place-items-center rounded-control text-content-muted hover:bg-surface-hover hover:text-content"
                on:click=move |_| open.update(|value| *value = !*value)
                aria-haspopup="menu"
                aria-expanded=move || if open.get() { "true" } else { "false" }
                aria-label=l!("apps.launcher")
                title=l!("apps.launcher")
            >
                <Icon icon=Icon::LayoutGrid size=IconSize::Sm />
            </button>

            <Show when=move || open.get() fallback=|| ()>
                <div
                    class="absolute right-0 z-40 mt-1 w-72 overflow-hidden rounded-pop border border-edge bg-surface-raised shadow-pop"
                    role="menu"
                >
                    {move || {
                            let installed = enabled.get().unwrap_or_default();

                            // Everything with somewhere to go. Core is skipped
                            // because it has no home - it *is* the shell, and
                            // listing it would be listing the window the
                            // launcher is drawn in. Master data is listed even
                            // though nobody can switch it off: it has real
                            // screens, and "cannot be removed" is no reason to
                            // hide the way to them.
                            let on: Vec<&'static AppDescriptor> = apps::enabled_in(&installed)
                                .filter(|app| app.is_a_place())
                                .collect();
                            let off: Vec<&'static AppDescriptor> = apps::optional()
                                .filter(|app| !installed.iter().any(|id| id == app.id))
                                .collect();

                            // Plain `if`, not `<Show/>`: this is inside a
                            // resolved `Suspend`, so the lists are values and
                            // not signals. A `<Show/>` here would move them
                            // into a closure that reruns for nothing.
                            let installed_section = (!on.is_empty()).then(|| {
                                view! {
                                    <div class="p-1">
                                        {on.iter()
                                            .map(|app| {
                                                view! {
                                                    <LauncherEntry
                                                        app=app
                                                        on:click=move |_| open.set(false)
                                                    />
                                                }
                                            })
                                            .collect::<Vec<_>>()}
                                    </div>
                                }
                                .into_any()
                            });

                            let available_section = (!off.is_empty()).then(|| {
                                view! {
                                    <div class="border-t border-edge px-3 pb-1 pt-2">
                                        <div class="text-xs font-medium uppercase tracking-wide text-content-subtle">
                                            {l!("apps.launcher.not_installed")}
                                        </div>
                                    </div>
                                    <div class="p-1 pt-0">
                                        {off.iter()
                                            .map(|app| view! { <DimmedEntry app=app /> })
                                            .collect::<Vec<_>>()}
                                    </div>
                                }
                                .into_any()
                            });

                            view! {
                                {installed_section}
                                {available_section}

                                <Show when=may_install fallback=|| ()>
                                    <div class="border-t border-edge p-1">
                                        <A
                                            href=APPS_HREF
                                            attr:class="flex h-row w-full items-center gap-2 rounded-control px-2 text-sm text-content-muted hover:bg-surface-hover hover:text-content"
                                            attr:role="menuitem"
                                            on:click=move |_| open.set(false)
                                        >
                                            <Icon icon=Icon::Blocks size=IconSize::Sm />
                                            {l!("apps.launcher.browse")}
                                        </A>
                                    </div>
                                </Show>
                            }
                    }}
                </div>
            </Show>
        </div>
    }
}

/// An app this workspace has: a link straight into it.
#[component]
fn launcher_entry(app: &'static AppDescriptor) -> impl IntoView {
    view! {
        <A
            href=app.home.unwrap_or("/")
            attr:class="flex w-full items-start gap-2.5 rounded-control px-2 py-1.5 text-left hover:bg-surface-hover"
            attr:role="menuitem"
        >
            <span class="mt-0.5 text-content-muted">
                <Icon icon=icon_of(app) size=IconSize::Sm />
            </span>
            <span class="min-w-0">
                <span class="block truncate-fade text-sm font-medium text-content">
                    {t(&phonix_core::i18n::Message::new(app.name))}
                </span>
                <span class="block text-xs leading-snug text-content-subtle">
                    {t(&phonix_core::i18n::Message::new(app.summary))}
                </span>
            </span>
        </A>
    }
}

/// One this workspace has not switched on. Not a link: there is nowhere to go
/// yet, and a link that lands on a page saying "you cannot see this" is worse
/// than no link.
#[component]
fn dimmed_entry(app: &'static AppDescriptor) -> impl IntoView {
    view! {
        <div class="flex w-full items-center gap-2.5 rounded-control px-2 py-1.5 opacity-60">
            <span class="text-content-subtle">
                <Icon icon=icon_of(app) size=IconSize::Sm />
            </span>
            <span class="min-w-0 truncate-fade text-sm text-content-muted">
                {t(&phonix_core::i18n::Message::new(app.name))}
            </span>
        </div>
    }
}
