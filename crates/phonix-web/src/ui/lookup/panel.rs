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
//!
//! # The wheel is about the page, not about scrolling
//!
//! The panel closes on the wheel because the *page* moving leaves it stranded
//! beside a field that is no longer there. A wheel inside the panel moves no
//! page - it scrolls the list, which is the one thing a long dropdown exists
//! to allow - so that one is not a dismissal, and treating it as one made
//! every list of more than a windowful impossible to reach the bottom of.
//! The list also sets `overscroll-behavior: contain`, so reaching its end does
//! not hand the wheel on to the page and close the panel that way instead.

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

        // Whether an event landed inside one of this control's own two nodes.
        let inside = move |node: Option<&leptos::web_sys::Node>,
                           within: NodeRef<leptos::html::Div>| {
            within
                .try_get_untracked()
                .flatten()
                .is_some_and(|element| element.contains(node))
        };

        let outside = window_event_listener(leptos::ev::pointerdown, move |event| {
            let Some(target) = event.target() else {
                return;
            };
            let node = target.dyn_ref::<leptos::web_sys::Node>();

            if !inside(node, panel) && !inside(node, anchor) {
                let _ = open.try_set(false);
            }
        });

        let escape = window_event_listener(leptos::ev::keydown, move |event| {
            if event.key() == "Escape" {
                let _ = open.try_set(false);
            }
        });

        let scrolled = window_event_listener(leptos::ev::wheel, move |event| {
            let node = event
                .target()
                .and_then(|target| target.dyn_into::<leptos::web_sys::Node>().ok());

            // Inside the panel this is somebody reading the list. Anywhere
            // else it is the page about to move out from under it. The anchor
            // is deliberately not exempt: it does not scroll, so a wheel over
            // the field itself is the page moving.
            if !inside(node.as_ref(), panel) {
                let _ = open.try_set(false);
            }
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
