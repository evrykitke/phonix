//! The Phonix Leptos application.
//!
//! Compiled twice:
//!
//! * with `--features ssr`     for the server binary (`phonix-server`)
//! * with `--features hydrate` for the WebAssembly bundle
//!
//! Code that must not reach the browser - database access, secrets, broker
//! connections - lives behind `#[cfg(feature = "ssr")]` or inside a `#[server]`
//! function body, which the macro strips from the client build.

// Leptos view types are one deeply nested tuple per screen, and the compiler
// computes their layout recursively. The sign-up form alone exceeds the default
// 128; this is the type checker's stack, not ours, and costs nothing to raise.
#![recursion_limit = "512"]
// A panic here is not an error, it is the end of the session: in wasm the
// strategy is abort, so one panic stops every handler, effect and resource in
// the module at once and the page freezes. `recovery` makes that survivable;
// this makes it rare. The crate is already compliant - one documented
// `expect`, on `Shell::get` - and the point of the deny is that it stays that
// way rather than drifting back one convenient `unwrap` at a time.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
// Tests are the exception, and the whole crate is `cfg(test)` while `cargo
// test` builds it. A test that unwraps is a test that fails loudly on the
// developer's machine, which is exactly what it should do.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

pub mod app;
pub mod components;
pub mod i18n;
pub mod icons;
pub mod navigation;
pub mod pages;
pub mod server_fns;
pub mod theme;
pub mod ui;

#[cfg(feature = "hydrate")]
pub mod recovery;

#[cfg(feature = "ssr")]
pub mod server;
#[cfg(feature = "ssr")]
pub mod state;

pub use app::{App, shell};

/// Entry point for the WebAssembly bundle.
///
/// `cargo-leptos` emits a JS shim that calls this on page load; it takes over
/// the server-rendered DOM instead of replacing it.
#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    // Turns an opaque "unreachable executed" wasm trap into a readable Rust
    // panic message in the browser console *and* a message on the screen, since
    // the trap takes the whole module with it and the page freezes.
    recovery::install();
    leptos::mount::hydrate_body(App);
}
