//! Who is looking at the page, as the kit needs to know it.
//!
//! # The dependency points the right way
//!
//! Permission-gated controls need the signed-in account. The kit could reach
//! into the application shell for it - and would then depend on the shell,
//! which is exactly the direction that stops a component being reusable.
//!
//! So the kit declares what it needs and the host supplies it:
//! [`AppShell`](crate::components::shell::AppShell) calls [`Viewer::provide`]
//! once, and everything below reads it. The kit knows there is *a* viewer; it
//! does not know there is a sidebar, a workspace, or a session cookie.
//!
//! # Nobody, until proven otherwise
//!
//! [`Viewer::get`] answers `None` in two situations that are worth keeping
//! apart in your head and identical in effect: the session has not resolved
//! yet, and there is no host providing one. Both mean *every gated control
//! stays hidden*. The alternative - showing controls and hiding them when the
//! answer arrives - flickers buttons at people that they may not be allowed to
//! press, and is the wrong way round to be wrong.

use leptos::prelude::*;
use phonix_core::identity::AuthUser;

/// The signed-in account, for anything in the kit that gates on permissions.
#[derive(Clone, Copy)]
pub struct Viewer(pub Signal<Option<AuthUser>>);

impl Viewer {
    /// Make the viewer available to everything rendered below. The host calls
    /// this once.
    pub fn provide(user: Signal<Option<AuthUser>>) {
        provide_context(Self(user));
    }

    /// The viewer, or a signal that is permanently nobody when no host has
    /// provided one.
    pub fn get() -> Signal<Option<AuthUser>> {
        use_context::<Self>().map_or_else(|| Signal::derive(|| None), |viewer| viewer.0)
    }
}
