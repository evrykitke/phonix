//! The collapsible navigation panel.
//!
//! Renders [`crate::navigation::MENU`] to whatever depth it is declared at. A
//! group is a button that opens its children; a leaf is a link. Which one is
//! highlighted, and which groups are open, both come from the [`Trail`] rather
//! than from anything a screen sets - see [`super`] for why.
//!
//! Collapsed, the panel becomes a 48px rail of icons. Nested levels are hidden
//! there rather than squeezed: there is no honest way to show a third-level
//! item in 48 pixels, so clicking a group on the rail opens the panel back up
//! instead of pretending.
//!
//! Below `md` it is not a column at all but a drawer over the content, opened
//! from the top bar and closed by navigating, by the backdrop, or by Escape.
//! The rail is not offered there - see [`super`].
//!
//! [`Trail`]: crate::navigation::Trail

use leptos::prelude::*;
use leptos_router::components::A;

use phonix_core::identity::AuthUser;

use super::Shell;
use crate::components::tenant_badge::TenantBadge;
use crate::i18n::t;
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::navigation::{MENU, NavNode};

/// What hides a label when the panel is narrowed to its rail.
///
/// `md:hidden` rather than `hidden`, and written out rather than expressed with
/// `class:`: the rail is a desktop state, and below `md` this same panel is a
/// full-width drawer whose labels must stay. Tailwind only sees classes it can
/// read as whole strings, and the `class:` directive takes an identifier, which
/// a variant like `md:hidden` is not.
const RAIL_HIDES: &str = "md:hidden";

/// `RAIL_HIDES` when the panel is a rail, nothing when it is not.
fn rail_hidden(shell: Shell) -> &'static str {
    if shell.collapsed() { RAIL_HIDES } else { "" }
}

#[component]
pub fn sidebar() -> impl IntoView {
    let shell = Shell::get();

    view! {
        // The backdrop is the drawer's other half: it dims the page and takes
        // the tap that closes it. `md:hidden` because at that width the panel
        // is part of the layout and dimming the content would be nonsense.
        <div
            class="fixed inset-0 z-40 bg-black/40 md:hidden"
            class:hidden=move || !shell.drawer_open.get()
            on:click=move |_| shell.close_drawer()
            aria-hidden="true"
        />

        <aside
            class=move || {
                // Width applies from `md` up; the drawer is always full width
                // because it was opened on purpose.
                let width = if shell.collapsed() { "md:w-rail" } else { "md:w-sidebar" };
                let slide = if shell.drawer_open.get() {
                    "translate-x-0"
                } else {
                    "-translate-x-full md:translate-x-0"
                };
                format!(
                    "fixed inset-y-0 left-0 z-50 flex w-sidebar shrink-0 flex-col border-r \
                     border-edge bg-surface-shell transition-transform duration-200 ease-out \
                     md:static md:h-full md:transition-[width] md:duration-150 {width} {slide}",
                )
            }
            aria-label=l!("nav.main")
        >
            <Brand />

            <nav class="min-h-0 flex-1 overflow-y-auto overflow-x-hidden px-2 py-2">
                // The menu is permission-gated, so it cannot be built until the
                // session has resolved. No fallback: a skeleton of menu entries
                // that then change shape is worse than a panel that arrives
                // complete a few milliseconds later.
                <Suspense fallback=|| ()>
                    {move || Suspend::new(async move {
                        // Stored rather than borrowed: `nav_node` hands it to a
                        // `Show` closure that outlives this scope, and a group
                        // three levels down still has to gate its children.
                        let user = StoredValue::new(shell.user().await);
                        view! {
                            <ul class="space-y-0.5">{nav_nodes(shell, user, MENU, 0)}</ul>
                        }
                    })}
                </Suspense>
            </nav>

            <Workspace />
            <Footer />
        </aside>
    }
}

/// The workspace mark, and the control that collapses the panel.
#[component]
fn brand() -> impl IntoView {
    let shell = Shell::get();

    view! {
        <div class="flex h-topbar shrink-0 items-center gap-2 border-b border-edge px-2">
            <A
                href="/dashboard"
                attr:class="flex min-w-0 items-center gap-2 rounded-control px-1 py-1 hover:bg-surface-hover"
            >
                <span
                    class="grid size-6 shrink-0 place-items-center rounded-control bg-brand text-2xs font-bold text-on-brand"
                    aria-hidden="true"
                >
                    "P"
                </span>
                <span class=move || {
                    format!("truncate-fade text-sm font-semibold tracking-tight {}", rail_hidden(shell))
                }>
                    // The product's name, not a word.
                    "Phonix"
                </span>
            </A>
        </div>
    }
}

/// Which workspace this is, at the foot of the drawer.
///
/// `md:hidden` because above that width the same fact is in the top bar, where
/// there is room for it. Below it there is not: the badge was taking a third of
/// a 390px bar to repeat something that does not change while you use the app.
/// The drawer is the honest home for it - it is already the answer to "where am
/// I", and it is somewhere you go deliberately.
#[component]
fn workspace() -> impl IntoView {
    view! {
        <div class="shrink-0 border-t border-edge p-2 md:hidden">
            <TenantBadge />
        </div>
    }
}

/// Sits at the bottom of the panel, out of the way of the menu.
#[component]
fn footer() -> impl IntoView {
    let shell = Shell::get();

    view! {
        <div class="hidden shrink-0 border-t border-edge p-2 md:block">
            <button
                type="button"
                class="flex h-row w-full items-center gap-2 rounded-control px-2 text-content-muted hover:bg-surface-hover hover:text-content"
                on:click=move |_| shell.theme.toggle_sidebar()
                aria-label=move || {
                    if shell.collapsed() { l!("nav.expand") } else { l!("nav.collapse") }
                }
                title=move || {
                    if shell.collapsed() { l!("nav.expand") } else { l!("nav.collapse") }
                }
            >
                {move || {
                    let icon = if shell.collapsed() {
                        Icon::PanelLeftOpen
                    } else {
                        Icon::PanelLeftClose
                    };
                    view! { <Icon icon=icon size=IconSize::Sm class="shrink-0" /> }
                }}
                <span class="truncate-fade text-xs" class:hidden=move || shell.collapsed()>
                    {l!("nav.collapse_short")}
                </span>
            </button>
        </div>
    }
}

/// Render a level of the menu, and every level below it.
///
/// A plain function rather than a component because it calls itself: the return
/// type is erased to [`AnyView`], which is what stops a self-referential view
/// type the compiler cannot name.
fn nav_nodes(
    shell: Shell,
    user: StoredValue<Option<AuthUser>>,
    nodes: &'static [NavNode],
    depth: usize,
) -> AnyView {
    nodes
        .iter()
        .filter(|node| {
            // Permission gating. Presentation only: the server function behind
            // each screen states its own requirement, and that is the check
            // that stops anyone typing the URL.
            !node.hidden && user.with_value(|user| node.visible_to(user.as_ref()))
        })
        .map(|node| nav_node(shell, user, node, depth))
        .collect::<Vec<_>>()
        .into_any()
}

fn nav_node(
    shell: Shell,
    user: StoredValue<Option<AuthUser>>,
    node: &'static NavNode,
    depth: usize,
) -> AnyView {
    let key = node.key;

    // Nested levels are indented rather than boxed, and the icon column stays
    // aligned so the eye can still run straight down it.
    let indent = move || {
        if shell.collapsed() {
            String::new()
        } else {
            format!("padding-left:{}rem", 0.5 + depth as f32 * 0.75)
        }
    };

    let row_class = move |is_current: bool, on_trail: bool| {
        let state = if is_current {
            "bg-brand-subtle text-brand font-medium"
        } else if on_trail {
            "text-content hover:bg-surface-hover"
        } else {
            "text-content-muted hover:bg-surface-hover hover:text-content"
        };

        format!(
            "group flex h-row w-full items-center gap-2 rounded-control pr-2 text-sm \
             transition-colors {state}"
        )
    };

    match node.href {
        Some(href) if !node.is_group() => view! {
            <li>
                <A
                    href=href
                    attr:class=move || {
                        shell
                            .trail
                            .with(|trail| row_class(trail.is_current(key), trail.contains(key)))
                    }
                    attr:style=indent
                    // `aria-current` comes from <A> itself, which sets it from
                    // the router's own idea of an active link. Setting it here
                    // as well emitted the attribute twice.
                    attr:title=t(&node.label())
                >
                    <NodeIcon node=node />
                    <span class=move || format!("truncate-fade {}", rail_hidden(shell))>
                        {t(&node.label())}
                    </span>
                </A>
            </li>
        }
        .into_any(),

        // A group, with or without a landing route of its own.
        _ => view! {
            <li>
                <button
                    type="button"
                    class=move || {
                        shell
                            .trail
                            .with(|trail| row_class(trail.is_current(key), trail.contains(key)))
                    }
                    style=indent
                    aria-expanded=move || if shell.is_open(key) { "true" } else { "false" }
                    title=t(&node.label())
                    on:click=move |_| {
                        // On the rail there is nowhere to put the children, so
                        // the click opens the panel instead of a submenu that
                        // would have to render into 48 pixels.
                        if shell.collapsed() {
                            shell.theme.toggle_sidebar();
                        }
                        shell.toggle(key);
                    }
                >
                    <NodeIcon node=node />
                    <span class=move || format!("truncate-fade {}", rail_hidden(shell))>
                        {t(&node.label())}
                    </span>
                    <span
                        class=move || {
                            format!("ml-auto shrink-0 transition-transform duration-150 {}", rail_hidden(shell))
                        }
                        class:rotate-90=move || shell.is_open(key)
                        aria-hidden="true"
                    >
                        <Icon icon=Icon::ChevronRight size=IconSize::Xs />
                    </span>
                </button>

                <Show when=move || shell.is_open(key) && !shell.collapsed() fallback=|| ()>
                    <ul class="mt-0.5 space-y-0.5">{nav_nodes(shell, user, node.children, depth + 1)}</ul>
                </Show>
            </li>
        }
        .into_any(),
    }
}

/// A node's icon, or an aligned placeholder where it has none.
#[component]
fn node_icon(node: &'static NavNode) -> impl IntoView {
    match node.icon {
        Some(icon) => view! {
            <span class="grid size-6 shrink-0 place-items-center">
                <Icon icon=icon size=IconSize::Sm />
            </span>
        }
        .into_any(),
        None => view! {
            <span class="grid size-6 shrink-0 place-items-center" aria-hidden="true">
                <span class="size-1.5 rounded-full bg-current opacity-40"></span>
            </span>
        }
        .into_any(),
    }
}
