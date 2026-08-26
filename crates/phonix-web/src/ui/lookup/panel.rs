//! What every panel in this module has in common: how it goes away.
//!
//! A panel here is `position: fixed`, measured from its field at the instant it
//! opens - see [`place`](super::place). Fixed means it does not travel with the
//! page, so the arrangement only stays true for as long as nothing moves. The
//! answer is not to re-measure on every scroll frame; it is to close.
//!
//! Four ways out, and each of them is somebody's habit rather than an edge
//! case: a pointer anywhere else, Escape, the wheel, and a window that changed
//! size. They are gathered here because a dropdown that dismisses on three of
//! the four is worse than one that dismisses on none - it teaches a rule and
//! then breaks it.

use leptos::prelude::*;
use leptos::wasm_bindgen::JsCast;

/// Close `open` on anything that would leave the panel stranded.
///
/// `panel` and `anchor` are what a pointer is measured against: a click inside
/// either is a click on this control, and only a click outside both is a
/// dismissal. Both are read with `try_get_untracked`, because these listeners
/// outlive their owner by a tick when the field is inside a grid row that is
/// being replaced - see the note on zombie rows in `ui::table::menu`.
pub(super) fn dismiss_when_moved(
    open: RwSignal<bool>,
    panel: NodeRef<leptos::html::Div>,
    anchor: NodeRef<leptos::html::Div>,
) {
    Effect::new(move |_| {
        // Nothing is listening while nothing is open. The listeners are on the
        // window, and a page of forty closed dropdowns would otherwise be a
        // hundred and sixty handlers running on every wheel event.
        if !open.get() {
            return;
        }

        let outside = window_event_listener(leptos::ev::pointerdown, move |event| {
            let Some(target) = event.target() else {
                return;
            };
            let node = target.dyn_ref::<leptos::web_sys::Node>();

            let within = panel
                .try_get_untracked()
                .flatten()
                .is_some_and(|panel| panel.contains(node))
                || anchor
                    .try_get_untracked()
                    .flatten()
                    .is_some_and(|anchor| anchor.contains(node));

            if !within {
                let _ = open.try_set(false);
            }
        });

        let escape = window_event_listener(leptos::ev::keydown, move |event| {
            if event.key() == "Escape" {
                let _ = open.try_set(false);
            }
        });

        let scrolled = window_event_listener(leptos::ev::wheel, move |_| {
            let _ = open.try_set(false);
        });
        let resized = window_event_listener(leptos::ev::resize, move |_| {
            let _ = open.try_set(false);
        });

        on_cleanup(move || {
            outside.remove();
            escape.remove();
            scrolled.remove();
            resized.remove();
        });
    });
}
