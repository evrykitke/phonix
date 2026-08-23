//! The avatar dropdown, and the appearance controls inside it.
//!
//! Theme lives here rather than in a settings screen because it is a per-device
//! display choice, not workspace configuration: it belongs one click from every
//! screen, next to the account it applies to.
//!
//! Both controls write through [`Theme`], which sets the attributes on `<html>`
//! and the cookie in one step - so the change is instant and the next full page
//! load already has it. See [`crate::theme`].

use leptos::prelude::*;
use leptos::wasm_bindgen::JsCast;
use leptos::web_sys::Element;
use leptos_router::components::A;
use phonix_core::authorization::names;
use phonix_core::identity::AuthUser;

use super::Shell;
use crate::components::language::LanguageSection;
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::server_fns::auth_fns::sign_out;
use crate::theme::{Accent, ThemeMode};

#[component]
pub fn user_menu() -> impl IntoView {
    let shell = Shell::get();

    view! {
        // Nothing until the session resolves: an avatar with placeholder
        // initials in it is a worse answer than no avatar for a moment.
        <Suspense fallback=|| ()>
            {move || Suspend::new(async move {
                shell.user().await.map(|user| view! { <UserMenuBody user=user /> })
            })}
        </Suspense>
    }
}

/// The menu itself, once there is somebody to name in it.
#[component]
fn user_menu_body(user: AuthUser) -> impl IntoView {
    let open = RwSignal::new(false);

    let initials = user.initials();
    let display_name = user.display_name.clone();
    let email = user.email.clone();
    let may_configure = user.can(names::SETTINGS);

    let sign_out_action = Action::new(|(): &()| async move {
        let _ = sign_out().await;
        // A full load, not a router navigation: the cookie has just been
        // cleared and every resource on the page was resolved while it existed.
        let _ = window().location().set_href("/");
    });

    // Any click that is not inside this menu closes it, which is what makes the
    // dropdown behave like every other one on the platform. Registered
    // unconditionally rather than only while open: a listener that adds and
    // removes itself races with the click that opened the menu and closes it
    // again in the same tick.
    Effect::new(move |_| {
        let handle = window_event_listener(leptos::ev::click, move |event| {
            if !open.get_untracked() {
                return;
            }

            // `closest` walks up from whatever was clicked, so a click on the
            // avatar's own <span> still counts as inside the menu.
            let inside = event
                .target()
                .and_then(|target| target.dyn_ref::<Element>().cloned())
                .is_some_and(|element| {
                    element.closest("[data-user-menu]").ok().flatten().is_some()
                });

            if !inside {
                open.set(false);
            }
        });

        on_cleanup(move || handle.remove());
    });

    view! {
        <div class="relative" data-user-menu>
            <button
                type="button"
                class="flex h-7 items-center gap-1.5 rounded-control pl-0.5 pr-1 hover:bg-surface-hover"
                on:click=move |_| open.update(|value| *value = !*value)
                aria-haspopup="menu"
                aria-expanded=move || if open.get() { "true" } else { "false" }
                aria-label=l!("menu.account_and_appearance")
            >
                <span
                    class="grid size-6 shrink-0 place-items-center rounded-full bg-brand text-2xs font-semibold text-on-brand"
                    aria-hidden="true"
                >
                    {initials}
                </span>
                <span class="hidden max-w-[10rem] truncate-fade text-sm md:inline">
                    {display_name.clone()}
                </span>
                <Icon icon=Icon::ChevronDown size=IconSize::Xs class="text-content-subtle" />
            </button>

            <Show when=move || open.get() fallback=|| ()>
                <div
                    class="absolute right-0 z-40 mt-1 w-64 overflow-hidden rounded-pop border border-edge bg-surface-raised shadow-pop"
                    role="menu"
                >
                    <div class="border-b border-edge px-3 py-2">
                        <div class="truncate-fade text-sm font-medium text-content">
                            {display_name.clone()}
                        </div>
                        <div class="truncate-fade text-xs text-content-subtle">{email.clone()}</div>
                    </div>

                    <Appearance />
                    <LanguageSection />

                    <div class="border-t border-edge p-1">
                        // Ungated: everybody has an account, so this is the one
                        // entry that needs no permission behind it.
                        <A
                            href="/account"
                            attr:class="flex h-row w-full items-center gap-2 rounded-control px-2 text-sm text-content-muted hover:bg-surface-hover hover:text-content"
                            attr:role="menuitem"
                            on:click=move |_| open.set(false)
                        >
                            <Icon icon=Icon::CircleUser size=IconSize::Sm />
                            {l!("menu.my_account")}
                        </A>

                        <Show when=move || may_configure fallback=|| ()>
                            <A
                                href="/admin/settings"
                                attr:class="flex h-row w-full items-center gap-2 rounded-control px-2 text-sm text-content-muted hover:bg-surface-hover hover:text-content"
                                attr:role="menuitem"
                                on:click=move |_| open.set(false)
                            >
                                <Icon icon=Icon::Settings size=IconSize::Sm />
                                {l!("menu.workspace_settings")}
                            </A>
                        </Show>

                        <button
                            type="button"
                            class="flex h-row w-full items-center gap-2 rounded-control px-2 text-sm text-content-muted hover:bg-surface-hover hover:text-danger"
                            role="menuitem"
                            on:click=move |_| {
                                open.set(false);
                                sign_out_action.dispatch(());
                            }
                        >
                            <Icon icon=Icon::LogOut size=IconSize::Sm />
                            {l!("menu.sign_out")}
                        </button>
                    </div>
                </div>
            </Show>
        </div>
    }
}

/// Light/dark, and the accent.
#[component]
fn appearance() -> impl IntoView {
    let shell = Shell::get();
    let theme = shell.theme;

    view! {
        <div class="border-b border-edge px-3 py-2">
            <div class="flex items-center gap-1.5 text-xs font-medium uppercase tracking-wide text-content-subtle">
                <Icon icon=Icon::Palette size=IconSize::Xs />
                {l!("menu.appearance")}
            </div>

            <div class="mt-2 grid grid-cols-3 gap-1 rounded-control bg-surface-sunken p-0.5">
                {ThemeMode::ALL
                    .iter()
                    .copied()
                    .map(|mode| {
                        view! {
                            <button
                                type="button"
                                class=move || {
                                    let selected = theme.mode() == mode;
                                    let state = if selected {
                                        "bg-surface-raised text-content shadow-panel"
                                    } else {
                                        "text-content-muted hover:text-content"
                                    };
                                    format!(
                                        "flex h-7 items-center justify-center gap-1 rounded-[0.3rem] \
                                         text-xs transition-colors {state}",
                                    )
                                }
                                aria-pressed=move || if theme.mode() == mode { "true" } else { "false" }
                                on:click=move |_| theme.set_mode(mode)
                            >
                                <Icon icon=mode_icon(mode) size=IconSize::Xs />
                                {mode.label()}
                            </button>
                        }
                    })
                    .collect::<Vec<_>>()}
            </div>

            <div class="mt-2 flex items-center gap-1.5">
                {Accent::ALL
                    .iter()
                    .copied()
                    .map(|accent| {
                        view! {
                            <button
                                type="button"
                                class=move || {
                                    let selected = theme.accent() == accent;
                                    let ring = if selected {
                                        "ring-2 ring-offset-2 ring-offset-surface-raised ring-content-subtle"
                                    } else {
                                        "hover:scale-110"
                                    };
                                    format!(
                                        "grid size-5 place-items-center rounded-full transition-transform {ring}",
                                    )
                                }
                                // A fixed colour, not var(--brand): the swatches
                                // sit side by side and each has to look like
                                // itself, not like the accent in force.
                                style=format!("background-color:{}", accent.swatch())
                                aria-label=accent.label()
                                title=accent.label()
                                aria-pressed=move || if theme.accent() == accent { "true" } else { "false" }
                                on:click=move |_| theme.set_accent(accent)
                            >
                                <Show when=move || theme.accent() == accent fallback=|| ()>
                                    <Icon
                                        icon=Icon::Check
                                        size=IconSize::Xs
                                        class="text-white drop-shadow"
                                    />
                                </Show>
                            </button>
                        }
                    })
                    .collect::<Vec<_>>()}
            </div>
        </div>
    }
}

const fn mode_icon(mode: ThemeMode) -> Icon {
    match mode {
        ThemeMode::System => Icon::Monitor,
        ThemeMode::Light => Icon::Sun,
        ThemeMode::Dark => Icon::Moon,
    }
}
