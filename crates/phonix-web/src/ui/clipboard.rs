//! Putting something on the clipboard.
//!
//! The same shape as [`table::export::download`](crate::ui::table::export::download),
//! and for the same reason: this is a browser action, `web-sys` is a
//! hydrate-only dependency, and the server build needs a body that compiles and
//! does nothing rather than a `cfg` at every call site.
//!
//! # Failure is silent, deliberately
//!
//! Every caller shows the text on screen as well - an invitation link, an API
//! key - because a clipboard is a convenience and a selectable `<code>` block
//! is the actual delivery. A browser that refuses the write costs nothing, and
//! an error message about it would be noise sitting next to the thing it failed
//! to copy.

/// Put `text` on the clipboard, if the browser will have it.
#[cfg(feature = "hydrate")]
pub fn copy(text: &str) {
    let _ = leptos::prelude::window()
        .navigator()
        .clipboard()
        .write_text(text);
}

/// Unreachable on the server: nothing renders a click there.
#[cfg(not(feature = "hydrate"))]
pub fn copy(_text: &str) {}
