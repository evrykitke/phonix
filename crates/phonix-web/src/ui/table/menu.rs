//! One button per row, holding every action that row offers.
//!
//! # Why a menu and not a strip of buttons
//!
//! The grid used to draw each action as its own button. That reads well with
//! two and stops working at four: the actions column grows every time a module
//! gains a verb, and it grows *sideways*, which on a phone is the one direction
//! that costs more than it looks - a table wider than the screen makes the
//! browser widen the layout viewport, and every fixed overlay on the page goes
//! with it.
//!
//! A single trigger is a fixed width whatever the row offers, which is also
//! what lets the row itself be short.
//!
//! # Always a menu, even for one action
//!
//! The obvious refinement - draw a lone action as a plain button and only fall
//! back to a menu at two or more - was tried on paper and rejected. Actions are
//! filtered per row and per permission: `RowAction::when` hides Delete on a
//! built-in role, a gated action is absent for a viewer without the permission.
//! So "how many actions" is a per-row answer, and a column that is a button on
//! one row and a menu on the next is a column whose control moves as your eye
//! goes down it. One trigger, always in the same place, is worth the extra
//! click on the rows that have only one entry.
//!
//! # The panel is always in the DOM
//!
//! It is shown and hidden by a class, never by `{open.get().then(..)}`. This
//! subtree renders inside the grid's `Suspend`, which hydrates asynchronously,
//! so a node that first appears on a click is a node leptos tries to *hydrate*
//! against the `<!---->` the server left where the closed menu was. With an
//! `<a>` inside - which every link action is - that is an unrecoverable
//! hydration error, and the page dies mid-click with every handler on it.
//!
//! Cost: one hidden panel per row. The strip of buttons this replaced put the
//! same number of nodes in the row anyway, so it is a wash.
//!
//! # Positioned fixed, on purpose
//!
//! The table scrolls inside `overflow-x-auto`, and `overflow-x` other than
//! `visible` makes the *other* axis a scroll container too. An absolutely
//! positioned menu is therefore clipped by the table's own box - it would open
//! and be cut off at the last row. So the menu is `position: fixed`, placed
//! from the trigger's rectangle when it opens, and closed by anything that
//! would move it: a scroll, a wheel, a resize.
//!
//! # Every handler here assumes its row may already be dead
//!
//! The rows are rendered inside the grid's `Transition`, and a transition holds
//! the *previous* markup on the screen until the next page of rows has arrived.
//! The reactive owner does not wait with it: re-running the closure that builds
//! the body disposes the old owner immediately, at the moment the refetch
//! starts. Between those two instants - a server round trip, and longer on a
//! slow link - every visible row is a zombie. It looks and hovers exactly as
//! before, and its signals and callbacks are already gone.
//!
//! `grid.refresh()` after a delete opens that window on purpose, so this is not
//! a rare interleaving: delete one row, click any other row's menu while the
//! table reloads, and the click lands on a disposed arena. Reading one is a
//! panic, and a panic in wasm takes the whole page with it.
//!
//! So nothing in a handler here may assume it is still alive. Each one asks
//! first - `try_get_untracked`, `try_run` - and does nothing if the answer is
//! no, which is the correct behaviour anyway: closing a menu that is about to
//! be replaced is not work worth doing. `<A>` still navigates, because that is
//! the browser following a link and owes nothing to the reactive system.

use leptos::prelude::*;
use leptos::wasm_bindgen::JsCast;
use leptos::web_sys;
use leptos_router::components::A;

use super::GridRow;
use super::action::{ActionKind, RowAction};
use super::handle::GridHandle;
use crate::components::page::Tone;
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::ui::alert::{Alerts, Confirm};

/// Where a menu sits, in viewport coordinates.
///
/// Right-anchored rather than left: the trigger is at the end of the row, and a
/// menu that grew rightwards from it would leave the screen.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct At {
    /// Distance from the right edge of the viewport.
    right: f64,
    /// Distance from the top, when the menu hangs below the trigger.
    top: Option<f64>,
    /// Distance from the bottom, when it opens upwards instead.
    bottom: Option<f64>,
}

impl At {
    fn style(self) -> String {
        let vertical = match (self.top, self.bottom) {
            (Some(top), _) => format!("top:{top}px"),
            (_, Some(bottom)) => format!("bottom:{bottom}px"),
            _ => "top:0".to_owned(),
        };

        format!("position:fixed;right:{}px;{vertical}", self.right)
    }
}

/// The actions for one row, behind one button.
#[component]
pub fn row_menu<T: GridRow>(
    actions: Vec<RowAction<T>>,
    row: T,
    handle: GridHandle,
) -> impl IntoView {
    let alerts = Alerts::get();
    let open = RwSignal::new(false);
    let at = RwSignal::new(At::default());

    let trigger = NodeRef::<leptos::html::Button>::new();
    let panel = NodeRef::<leptos::html::Div>::new();

    let count = actions.len();
    let actions = StoredValue::new(actions);

    // Everything that closes it. A pointer down anywhere that is neither the
    // trigger nor the menu itself - which is also what dismisses one row's menu
    // when another row's trigger is pressed, without either row knowing the
    // other exists. Scrolling and resizing close it rather than chasing it: a
    // fixed panel does not travel with the row it belongs to.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }

        let outside = window_event_listener(leptos::ev::pointerdown, move |event| {
            let Some(target) = event.target() else {
                return;
            };
            let node = target.dyn_ref::<web_sys::Node>();

            let within = panel
                .get_untracked()
                .is_some_and(|panel| panel.contains(node))
                || trigger
                    .get_untracked()
                    .is_some_and(|trigger| trigger.contains(node));

            if !within {
                open.set(false);
            }
        });

        let escape = window_event_listener(leptos::ev::keydown, move |event| {
            if event.key() == "Escape" {
                open.set(false);
            }
        });

        let scrolled = window_event_listener(leptos::ev::wheel, move |_| open.set(false));
        let resized = window_event_listener(leptos::ev::resize, move |_| open.set(false));

        on_cleanup(move || {
            outside.remove();
            escape.remove();
            scrolled.remove();
            resized.remove();
        });
    });

    view! {
        <div class="inline-flex">
            <button
                type="button"
                node_ref=trigger
                class="grid size-7 place-items-center rounded-control border border-edge text-content-subtle hover:bg-surface-hover hover:text-content"
                aria-haspopup="menu"
                aria-expanded=move || if open.get() { "true" } else { "false" }
                aria-label=l!("grid.row_actions")
                title=l!("grid.row_actions")
                on:click=move |event| {
                    event.stop_propagation();
                    // The row may have been disposed while its markup waits for
                    // the transition to swap it out. One question covers `at`
                    // too: both signals belong to this menu's owner, so either
                    // both are alive or neither is.
                    let Some(is_open) = open.try_get_untracked() else {
                        return;
                    };

                    if is_open {
                        open.set(false);
                    } else {
                        // Measured on the way open, not on every render: the
                        // trigger's rectangle only matters at the moment the
                        // panel is put on the screen, and anything that would
                        // move it afterwards closes it instead.
                        at.set(place(trigger, count));
                        open.set(true);
                    }
                }
            >
                <Icon icon=Icon::Ellipsis size=IconSize::Xs />
            </button>

            // Always in the DOM, shown by a class - never `{open.then(..)}`.
            // This subtree is rendered inside the grid's `Suspend`, which
            // hydrates asynchronously, so a node that first appears on a click
            // is a node leptos tries to *hydrate* against the `<!---->` the
            // server left for the closed menu. With an `<a>` inside, that is an
            // unrecoverable hydration error and a dead page. The row's mobile
            // detail row is built the same way and for the same reason.
            <div
                node_ref=panel
                role="menu"
                // `w-max` with a ceiling: the entries name themselves, and a
                // menu as wide as the longest verb reads better than one padded
                // to a guess - but never wider than the screen it opens on.
                class="alert-enter z-[55] w-max max-w-[min(16rem,calc(100vw-1rem))] overflow-hidden rounded-card border border-edge bg-surface-raised py-1 shadow-pop"
                class:hidden=move || !open.get()
                aria-hidden=move || if open.get() { "false" } else { "true" }
                style=move || at.get().style()
            >
                {actions
                    .with_value(|actions| {
                        actions
                            .iter()
                            .cloned()
                            .map(|action| {
                                view! {
                                    <MenuEntry
                                        action=action
                                        row=row.clone()
                                        handle=handle
                                        alerts=alerts
                                        close=Callback::new(move |()| open.set(false))
                                    />
                                }
                            })
                            .collect::<Vec<_>>()
                    })}
            </div>
        </div>
    }
}

/// Where the menu should sit, measured from its trigger.
///
/// Browser only. Measuring needs web-sys interfaces this crate asks for in the
/// hydrate build alone, and the server never opens a menu.
#[cfg(feature = "hydrate")]
fn place(trigger: NodeRef<leptos::html::Button>, count: usize) -> At {
    // Roughly how tall the menu will be. An estimate rather than a measurement:
    // measuring means rendering it off-screen first, and being a few pixels out
    // only changes which side of the trigger it opens on.
    const ITEM: f64 = 32.0;
    const PADDING: f64 = 8.0;
    const GAP: f64 = 4.0;

    let Some(button) = trigger.get_untracked() else {
        return At::default();
    };

    let rect = button.get_bounding_client_rect();
    let viewport = window()
        .inner_height()
        .ok()
        .and_then(|height| height.as_f64())
        .unwrap_or(0.0);
    let width = window()
        .inner_width()
        .ok()
        .and_then(|width| width.as_f64())
        .unwrap_or(0.0);

    let height = PADDING * 2.0 + ITEM * f64::from(u32::try_from(count).unwrap_or(u32::MAX));
    let below = viewport - rect.bottom();

    // Downwards unless it would not fit, which is what happens on the last rows
    // of a long table. If it fits neither way it hangs below anyway, where at
    // least the first entries are reachable.
    let upwards = below < height && rect.top() >= height;

    At {
        right: (width - rect.right()).max(GAP),
        top: (!upwards).then(|| rect.bottom() + GAP),
        bottom: upwards.then(|| viewport - rect.top() + GAP),
    }
}

#[cfg(not(feature = "hydrate"))]
fn place(_trigger: NodeRef<leptos::html::Button>, _count: usize) -> At {
    At::default()
}

/// One line in the menu.
#[component]
fn menu_entry<T: GridRow>(
    action: RowAction<T>,
    row: T,
    handle: GridHandle,
    alerts: Alerts,
    close: Callback<()>,
) -> impl IntoView {
    let class = format!(
        "flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm {}",
        match action.tone {
            Tone::Danger => "text-danger hover:bg-danger-subtle",
            _ => "text-content-muted hover:bg-surface-hover hover:text-content",
        },
    );

    let label = action.label.clone();
    let icon = action.icon;

    match action.kind {
        ActionKind::Link(href) => {
            let href = href(&row);

            view! {
                <A
                    href=href
                    attr:class=class
                    attr:role="menuitem"
                    // Not `run`: this entry outlives its owner for as long as
                    // the transition holds the old rows on screen, and the
                    // link navigates either way.
                    on:click=move |_| {
                        close.try_run(());
                    }
                >
                    <Icon icon=icon size=IconSize::Xs />
                    {label.clone()}
                </A>
            }
            .into_any()
        }
        ActionKind::Run(run) => {
            let confirm = action.confirm.clone();

            // The deed, as something that can be run later. A confirmation
            // dialog cannot be waited on the way `window.confirm` could, so
            // this goes into the question rather than after it.
            let perform = move || {
                // Last run's message would otherwise sit above the table while
                // this one is still in flight.
                handle.clear();
                run.run((row.clone(), handle));
            };

            view! {
                <button
                    type="button"
                    class=class
                    role="menuitem"
                    on:click=move |_| {
                        // The question is built *before* the menu closes. A
                        // `Confirm` carries a `Callback`, which is allocated in
                        // whichever reactive owner is current - so building one
                        // while this entry's own owner is being torn down is a
                        // callback in a disposed arena, and running it later is
                        // a panic rather than a delete.
                        match confirm.clone() {
                            None => perform(),
                            Some(question) => {
                                alerts
                                    .ask(
                                        Confirm::new(question, perform.clone())
                                            .titled(label.clone())
                                            .confirm_label(label.clone()),
                                    )
                            }
                        }

                        close.try_run(());
                    }
                >
                    <Icon icon=icon size=IconSize::Xs />
                    {label.clone()}
                </button>
            }
            .into_any()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_menu_hangs_below_its_trigger_when_there_is_room() {
        let at = At {
            right: 12.0,
            top: Some(200.0),
            bottom: None,
        };

        assert_eq!(at.style(), "position:fixed;right:12px;top:200px");
    }

    #[test]
    fn a_menu_with_no_room_below_opens_upwards() {
        // What the last row of a long table gets. Without it the menu would
        // hang off the bottom of the screen and the delete nobody could reach
        // would look like a delete that does not exist.
        let at = At {
            right: 12.0,
            top: None,
            bottom: Some(48.0),
        };

        assert_eq!(at.style(), "position:fixed;right:12px;bottom:48px");
    }

    #[test]
    fn a_menu_that_has_not_been_placed_still_produces_a_style() {
        // Belt and braces: an empty style would leave the panel at the top-left
        // of the viewport, which reads as a rendering fault rather than as a
        // menu.
        assert_eq!(At::default().style(), "position:fixed;right:0px;top:0");
    }
}
