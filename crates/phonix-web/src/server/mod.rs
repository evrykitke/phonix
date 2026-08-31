//! Server-side plumbing for the presentation layer.
//!
//! Compiled only with `--features ssr`. Nothing here reaches the WebAssembly
//! bundle: it is the code that turns a use case's result into an HTTP response,
//! and the browser has no use for it.
//!
//! | Module     | Responsibility                                        |
//! | ---------- | ----------------------------------------------------- |
//! | [`client`] | Reading the address and user-agent a request carries  |
//! | [`cookie`] | Building and parsing the session cookie               |
//!
//! It lives in `phonix-web` rather than `phonix-server` because both need it:
//! the Leptos server functions set headers through `ResponseOptions`, the axum
//! middleware sets them on a `Response`, and `phonix-server` depends on this
//! crate rather than the other way round.

pub mod client;
pub mod cookie;
