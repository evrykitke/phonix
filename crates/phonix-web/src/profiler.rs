//! The one thing the application tells the development profiler.
//!
//! After hydration there is no server request to match, so the route the
//! browser is on is knowable only here. It is *pushed* to the toolbar - a call
//! out through a global the toolbar owns - and nothing is ever read back.
//!
//! That direction is not a style choice. The toolbar is vanilla JavaScript in
//! a shadow root precisely so that it survives a wasm panic, which freezes
//! every effect, handler and resource in this module at once and is the moment
//! the profiler is most wanted. If this crate depended on anything the toolbar
//! produced, the toolbar would die with the application. See
//! `docs/adr/0004-development-profiler.md`, sections 6 and 7.
//!
//! With no profiler on the page the global is absent and every function here
//! returns without doing anything - the equivalent of the ADR's
//! `window.__phonix_profiler?.route(path)`.

use leptos::prelude::*;

/// Push the current route to the toolbar, whenever it changes.
///
/// Renders nothing, on either side, so it cannot be a hydration mismatch.
/// Place it inside `<Router>`, which is where the location is readable.
#[component]
pub fn ProfilerBridge() -> impl IntoView {
    #[cfg(feature = "hydrate")]
    {
        let location = leptos_router::hooks::use_location();

        // This is also what tells the toolbar that hydration has finished and
        // it is safe to touch the DOM: an effect cannot run before then, and
        // appending to `<body>` any earlier would shift the nodes Leptos is
        // still walking.
        Effect::new(move |_| {
            push_route(&location.pathname.get());
        });
    }
}

/// Call `window.__phonix_profiler.route(path)`, if anything is listening.
///
/// Every step is fallible and every failure is a silent return. A development
/// tool that is not installed must cost the application nothing, and a
/// development tool that *is* installed must not be able to panic the page it
/// is watching.
#[cfg(feature = "hydrate")]
fn push_route(path: &str) {
    use wasm_bindgen::{JsCast, JsValue};

    let Some(window) = web_sys::window() else {
        return;
    };

    let Ok(profiler) = js_sys::Reflect::get(&window, &JsValue::from_str("__phonix_profiler"))
    else {
        return;
    };

    if profiler.is_undefined() || profiler.is_null() {
        return;
    }

    let Ok(route) = js_sys::Reflect::get(&profiler, &JsValue::from_str("route")) else {
        return;
    };

    let Ok(route) = route.dyn_into::<js_sys::Function>() else {
        return;
    };

    // The toolbar's own error, if it has one. Not this crate's problem, and
    // certainly not worth a panic.
    let _ = route.call1(&profiler, &JsValue::from_str(path));
}
