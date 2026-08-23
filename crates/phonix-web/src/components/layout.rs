//! Chooses the chrome a route gets.
//!
//! There are two:
//!
//! * **The app shell** - navigation panel, top bar, command palette. See
//!   [`crate::components::shell`].
//! * **Bare** - the page on its own, centred by the page itself: the sign-in
//!   and sign-up screens, and the screens that finish a sign-in.
//!
//! # Chosen by path, guaranteed by `landing`
//!
//! Reading the path looks like the fragile option - a list of prefixes that
//! drifts out of step with the router - and it would be, on its own. What makes
//! it exact is [`landing`]: a session may only ever be *on* a path that suits
//! it, because anything else is redirected before the page renders. Bare
//! chrome on `/auth/*` is therefore the same statement as "this session has not
//! finished signing in", arrived at without waiting for a round trip.
//!
//! The alternative - awaiting the session here and choosing from it - puts
//! `<Outlet/>` inside a `Suspend`, and a route outlet inside an async boundary
//! stops swapping on navigation. See [`crate::components::shell`] for that.
//!
//! # The redirect is a real 302
//!
//! [`landing`] is applied from the outermost blocking resource in the tree, so
//! its answer is known before a byte of the page has been written. A page that
//! decides for itself can only redirect after its own HTML has been flushed,
//! which the visitor watches happen.

use leptos::prelude::*;
use leptos_router::components::Outlet;
use leptos_router::hooks::use_location;
use phonix_core::identity::{
    HALF_AUTHENTICATED_PREFIX, Landing, SIGN_IN_PATH, SIGN_UP_PATH, landing,
};

use crate::components::shell::AppShell;
use crate::server_fns::auth_fns::current_user;

#[component]
pub fn layout() -> impl IntoView {
    let location = use_location();

    // Whether this path is one a finished session belongs on. Reactive, so a
    // client-side navigation from the challenge to the dashboard swaps the
    // chrome without a reload.
    let bare = move || {
        let path = location.pathname.get();
        path == SIGN_IN_PATH || path == SIGN_UP_PATH || path.starts_with(HALF_AUTHENTICATED_PREFIX)
    };

    view! {
        <Gatekeeper />

        <Show
            when=bare
            fallback=|| {
                view! {
                    <AppShell>
                        <Outlet />
                    </AppShell>
                }
            }
        >
            <div class="min-h-full bg-surface text-content">
                <Outlet />
            </div>
        </Show>
    }
}

/// Turns a session around when it is somewhere it does not belong.
///
/// Renders nothing. It exists as its own component so the resource it blocks on
/// sits in its own `Suspense` boundary, well away from the outlet.
#[component]
fn gatekeeper() -> impl IntoView {
    // Blocking, so the server holds the first chunk of HTML until this has been
    // decided. A streaming resource would flush the page and then redirect,
    // which is the reload this exists to avoid.
    let session = OnceResource::new_blocking(current_user());
    let location = use_location();

    view! {
        <Suspense fallback=|| ()>
            {move || Suspend::new(async move {
                let user = session.await.ok().flatten();
                let path = location.pathname.get_untracked();

                if let Landing::Redirect(target) = landing(&path, user.as_ref()) {
                    leave_for(target);
                }
            })}
        </Suspense>
    }
}

/// Send the browser to `path`.
///
/// A real 302 during SSR; a full load in the browser, because everything the
/// page resolved was resolved for the session it is being sent away from.
#[cfg(feature = "ssr")]
fn leave_for(path: &str) {
    leptos_axum::redirect(path);
}

#[cfg(not(feature = "ssr"))]
fn leave_for(path: &str) {
    let _ = window().location().set_href(path);
}
