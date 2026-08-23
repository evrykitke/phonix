//! What the browser shows when the WebAssembly module traps.
//!
//! A panic in wasm cannot be caught. `catch_unwind` compiles for
//! `wasm32-unknown-unknown` and never catches anything, because the panic
//! strategy is abort - there is no unwinding to intercept - and `ErrorBoundary`
//! answers for `Result`, not for a trap. One panic anywhere therefore stops the
//! whole module: every event handler, every effect, every pending resource.
//! From the outside that is a frozen page with no explanation.
//!
//! What *can* still run is the panic hook. It is called before the trap, on a
//! module that is still alive, and it may reach the DOM through `web-sys`
//! without a reactive runtime. So this prevents nothing - nothing can - it
//! converts "the page is frozen and the viewer assumes the product is broken"
//! into a visible message and one button.
//!
//! # Nothing here may call back into wasm
//!
//! After the hook returns, the module traps. A `Closure` on the reload button
//! would be a call *into* a module whose memory is in whatever state the panic
//! left it, so the buttons carry inline handlers instead: plain DOM, no wasm,
//! no reactive system. That is also why the overlay is built out of elements
//! and inline styles rather than out of a component.
//!
//! # Reloading by itself, exactly once
//!
//! A reload is the right answer to a hydration mismatch - a stale bundle
//! meeting fresh server markup is the usual cause - and the wrong answer to a
//! panic raised by a click, which would silently throw away a half-filled form.
//! The question is therefore *had the viewer touched anything yet*: before the
//! first pointer or key event nothing of theirs exists to lose and the page
//! reloads on its own; afterwards it only offers to. A flag in `sessionStorage`
//! bounds that to one reload per tab, so a panic that reproduces becomes a
//! message rather than a loop.

use std::panic::PanicHookInfo;
use std::sync::atomic::{AtomicBool, Ordering};

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{Document, Element, Window};

/// The overlay's id: the handle its dismiss button reaches for, and the marker
/// that keeps a second panic from stacking a second copy.
const OVERLAY_ID: &str = "phonix-recovery";

/// Set once this tab has reloaded itself. Session storage, not local: a new tab
/// deserves its own attempt.
const RELOADED_KEY: &str = "phonix.recovery.reloaded";

/// True until the viewer's first pointer or key event.
static UNTOUCHED: AtomicBool = AtomicBool::new(true);

/// Install the panic hook. Call once, before hydration.
pub fn install() {
    watch_for_interaction();
    #[cfg(debug_assertions)]
    offer_a_test_panic();

    std::panic::set_hook(Box::new(|info| {
        // The readable message in the console, which is what a developer wants
        // and all the default hook does.
        console_error_panic_hook::hook(info);
        // The message on the screen, which is what everybody else wants.
        announce(info);
    }));
}

/// Hang `__phonixPanic()` off `window`, for development builds only.
///
/// Everything below can only be seen by causing the failure it answers to, and
/// a hydration mismatch is not something anybody can raise on demand. So this
/// raises one: call it from the browser console and the real hook runs down the
/// real path, including the trap that follows it.
///
/// * click the page first, then call it - the overlay
/// * reload and call it before touching anything - the one automatic reload,
///   and the overlay on the call after that, the flag having been spent
///
/// `debug_assertions` is off in `--release`, which is what `cargo leptos build`
/// and every deployed bundle use, so this reaches no one.
#[cfg(debug_assertions)]
fn offer_a_test_panic() {
    let Some(window) = web_sys::window() else {
        return;
    };

    #[allow(clippy::panic, reason = "the entire purpose of this function")]
    let trigger = Closure::<dyn Fn()>::new(|| panic!("__phonixPanic(): a deliberate test panic"));

    let _ = js_sys::Reflect::set(
        &window,
        &wasm_bindgen::JsValue::from_str("__phonixPanic"),
        trigger.as_ref(),
    );

    trigger.forget();
}

/// Note that the viewer has done something, so a later panic may have taken
/// their work with it.
///
/// Pointer and key only: scrolling across a page that has never been touched
/// still leaves nothing to lose. Registered in the capture phase, so nothing
/// downstream can stop it from being seen.
fn watch_for_interaction() {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };

    let mark = Closure::<dyn Fn()>::new(|| UNTOUCHED.store(false, Ordering::Relaxed));
    let callback = mark.as_ref().unchecked_ref();

    for event in ["pointerdown", "keydown"] {
        let _ = document.add_event_listener_with_callback_and_bool(event, callback, true);
    }

    // Deliberately leaked: it has to outlive this call, and the alternative is
    // holding a handle that only a page teardown would ever drop.
    mark.forget();
}

/// Put the panic on the screen.
///
/// Every step is fallible and every failure is silent, because a panic *inside*
/// the panic hook aborts with no message at all - the one outcome worse than
/// the frozen page this exists to explain.
fn announce(info: &PanicHookInfo<'_>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };

    // A hook can be entered more than once before the module actually traps.
    if document.get_element_by_id(OVERLAY_ID).is_some() {
        return;
    }

    if UNTOUCHED.load(Ordering::Relaxed) && reload_once(&window) {
        return;
    }

    let Some(body) = document.body() else {
        return;
    };
    let Some(overlay) = overlay(&document, &info.to_string()) else {
        return;
    };

    let _ = body.append_child(&overlay);
}

/// Reload the page, unless this tab has already tried that.
///
/// Returns whether a reload was started, in which case there is nothing worth
/// drawing.
fn reload_once(window: &Window) -> bool {
    let Ok(Some(storage)) = window.session_storage() else {
        // Storage is refused in some privacy modes. With nowhere to record the
        // attempt there is no way to bound it, so do not make it.
        return false;
    };

    if storage.get_item(RELOADED_KEY).ok().flatten().is_some() {
        return false;
    }

    if storage.set_item(RELOADED_KEY, "1").is_err() {
        return false;
    }

    window.location().reload().is_ok()
}

/// The scrim and the card.
///
/// Inline styles throughout, each resolved against the theme tokens in
/// `main.css` with a literal fallback: the tokens make this match the rest of
/// the application in either theme, and the fallbacks mean a page whose
/// stylesheet never arrived still shows something legible.
fn overlay(document: &Document, detail: &str) -> Option<Element> {
    let scrim = element(
        document,
        "div",
        "position:fixed; inset:0; z-index:2147483647; display:flex; \
         align-items:center; justify-content:center; padding:1rem; \
         background:var(--overlay, oklch(0.205 0.011 286 / 0.35)); \
         font-family:ui-sans-serif, system-ui, sans-serif;",
    )?;
    scrim.set_id(OVERLAY_ID);

    let card = element(
        document,
        "div",
        "box-sizing:border-box; width:100%; max-width:30rem; padding:1.25rem; \
         border:1px solid var(--edge, #d4d4d8); \
         border-radius:var(--radius-card, 0.5rem); \
         background:var(--surface-raised, #ffffff); \
         color:var(--content, #18181b); \
         box-shadow:var(--elevation-pop, 0 10px 30px -12px rgb(0 0 0 / 0.35)); \
         font-size:13px; line-height:1.5;",
    )?;

    let title = element(
        document,
        "h2",
        "margin:0 0 0.5rem; font-size:15px; font-weight:600; \
         color:var(--danger, #b91c1c);",
    )?;
    title.set_text_content(Some("Something went wrong"));

    let explanation = element(document, "p", "margin:0 0 0.75rem;")?;
    explanation.set_text_content(Some(
        "This page stopped responding and has to be reloaded. \
         Anything not yet saved will be lost.",
    ));

    // `set_text_content`, never `set_inner_html`: a panic message carries
    // whatever string the application was holding, including one that came
    // from a viewer.
    let message = element(
        document,
        "pre",
        "margin:0 0 1rem; padding:0.5rem 0.625rem; max-height:8rem; \
         overflow:auto; white-space:pre-wrap; overflow-wrap:anywhere; \
         border-radius:var(--radius-control, 0.375rem); \
         background:var(--surface-sunken, #f4f4f5); \
         color:var(--content-muted, #52525b); \
         font-family:ui-monospace, monospace; font-size:11px;",
    )?;
    message.set_text_content(Some(detail));

    let actions = element(
        document,
        "div",
        "display:flex; flex-wrap:wrap; gap:0.5rem; justify-content:flex-end;",
    )?;

    const BUTTON: &str = "padding:0.375rem 0.75rem; \
         border-radius:var(--radius-control, 0.375rem); font:inherit; \
         font-weight:500; cursor:pointer;";

    // Dismissable on purpose: the application behind this is dead, but its
    // markup is still readable, and someone mid-form may want to copy out what
    // they had typed before reloading over it.
    let dismiss = element(
        document,
        "button",
        &format!(
            "{BUTTON} border:1px solid var(--edge-strong, #a1a1aa); \
             background:transparent; color:inherit;"
        ),
    )?;
    dismiss.set_text_content(Some("Dismiss"));
    dismiss.set_attribute("type", "button").ok()?;
    dismiss
        .set_attribute(
            "onclick",
            &format!("document.getElementById('{OVERLAY_ID}').remove()"),
        )
        .ok()?;

    let reload = element(
        document,
        "button",
        &format!(
            "{BUTTON} border:1px solid transparent; \
             background:var(--brand, #4f46e5); color:var(--on-brand, #ffffff);"
        ),
    )?;
    reload.set_text_content(Some("Reload page"));
    reload.set_attribute("type", "button").ok()?;
    reload.set_attribute("onclick", "location.reload()").ok()?;

    actions.append_child(&dismiss).ok()?;
    actions.append_child(&reload).ok()?;

    card.append_child(&title).ok()?;
    card.append_child(&explanation).ok()?;
    card.append_child(&message).ok()?;
    card.append_child(&actions).ok()?;
    scrim.append_child(&card).ok()?;

    Some(scrim)
}

/// One element with one style attribute, or nothing.
fn element(document: &Document, tag: &str, style: &str) -> Option<Element> {
    let element = document.create_element(tag).ok()?;
    element.set_attribute("style", style).ok()?;
    Some(element)
}
