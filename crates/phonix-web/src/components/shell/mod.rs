//! The application shell: navigation panel, top bar, command palette.
//!
//! ```text
//! +--------------------------------------------------------------+
//! | brand | breadcrumb            search Ctrl+K   bell   avatar v |  <- TopBar
//! +---------+----------------------------------------------------+
//! | Dashbrd |                                                    |
//! | Admin v |                  <Outlet/>                         |  <- Sidebar
//! |  Users  |                                                    |     + page
//! |  Roles  |                                                    |
//! +---------+----------------------------------------------------+
//! ```
//!
//! # One piece of state, shared
//!
//! [`Shell`] is provided once here and read by every part. It is `Copy` - all
//! signals - so a child takes it from context and holds it without lifetimes or
//! clones. What it carries:
//!
//! * the signed-in user, for permission gating, as a resource
//! * the [`Trail`] for the current URL, recomputed on navigation
//! * which groups the *user* has explicitly opened or closed
//! * whether the command palette is up
//! * whether the navigation drawer is open, on a narrow screen
//!
//! # Two panels, one component
//!
//! Below `md` the navigation panel is a drawer: absent from the layout, slid in
//! over the content, with a backdrop. At `md` and up it is a column in the flex
//! row that the collapse control narrows to a rail. Both are the same markup
//! with different classes rather than two components, because a second copy of
//! a permission-gated recursive menu is a second copy to keep correct.
//!
//! The rail is a desktop idea and is not offered on a drawer - there is no
//! screen where a 48px strip of icons is the right answer *and* the panel had
//! to be summoned to see it at all.
//!
//! # Expansion: the tree decides, the user overrides
//!
//! A group is open when the current route is inside it. That is the behaviour
//! asked for - navigate to a child and its ancestors open, at any depth - and
//! it needs no per-screen wiring because [`Trail`] resolves it from the URL.
//!
//! But a user who collapses the section they are standing in must stay
//! collapsed, and an "open" flag alone would be re-opened by the very next
//! render. So the map here holds *overrides*, not state: a key present means
//! "the user said so", a key absent means "follow the trail". Navigating
//! somewhere else clears nothing and needs to clear nothing, because the trail
//! moves underneath the untouched keys.
//!
//! # Why the user is a resource here rather than a prop
//!
//! The obvious shape is for the layout to await the session and hand it in.
//! That puts `<Outlet/>` - the page itself - inside a `Suspend`, and a route
//! outlet inside an async boundary stops swapping on navigation: clicking a
//! menu entry changes the URL and leaves the previous screen on the display
//! until the page is reloaded. The chrome resolves its own user instead, so the
//! outlet hangs off nothing but the router.
//!
//! Each part that needs the user therefore reads it inside its own `Suspense` -
//! the sidebar for permission gating, the palette for the same, the avatar menu
//! for a name. Those are small and independent; the page is neither.

pub mod app_launcher;
pub mod command_palette;
pub mod sidebar;
pub mod topbar;
pub mod user_menu;

use std::collections::HashMap;

use leptos::prelude::*;
use leptos_router::hooks::use_location;
use phonix_core::identity::AuthUser;

use crate::navigation::{MENU, Trail};
use crate::server_fns::app_fns::enabled_apps;
use crate::server_fns::auth_fns::current_user;
use crate::theme::Theme;

use command_palette::CommandPalette;
use sidebar::Sidebar;
use topbar::TopBar;

/// Everything the chrome shares.
#[derive(Clone, Copy)]
pub struct Shell {
    /// Blocking, so the server holds the first chunk until the chrome can be
    /// rendered with the right menu rather than an empty one that fills in.
    session: OnceResource<Option<AuthUser>>,
    /// Which apps this workspace has switched on.
    ///
    /// Blocking too, and for a sharper reason than the menu: the permission
    /// editor draws a *different tree* depending on this, and one that arrived
    /// a moment late would grow rows under the pointer. One indexed query.
    apps: OnceResource<Vec<String>>,
    /// Groups the user has explicitly opened (`true`) or closed (`false`).
    /// Absent means "whatever the trail says".
    overrides: RwSignal<HashMap<&'static str, bool>>,
    /// Whether the command palette is up.
    pub palette_open: RwSignal<bool>,
    /// Whether the navigation drawer is showing. Only has an effect below
    /// `md`, where the panel is not part of the layout.
    pub drawer_open: RwSignal<bool>,
    /// The selected path through the menu, recomputed on every navigation.
    pub trail: Memo<Trail>,
    pub theme: Theme,
}

impl Shell {
    fn provide() -> Self {
        let location = use_location();

        let shell = Self {
            session: OnceResource::new_blocking(async move { current_user().await.ok().flatten() }),
            apps: OnceResource::new_blocking(
                async move { enabled_apps().await.unwrap_or_default() },
            ),
            overrides: RwSignal::new(HashMap::new()),
            palette_open: RwSignal::new(false),
            drawer_open: RwSignal::new(false),
            // A memo, so the whole sidebar does not re-run when the URL changes
            // to another route inside the same section.
            trail: Memo::new(move |_| Trail::resolve(MENU, &location.pathname.get())),
            theme: Theme::get(),
        };

        provide_context(shell);
        // The kit is handed what it needs rather than reaching in for it, so
        // `ui` depends on `phonix_core` and not on this shell.
        crate::ui::viewer::Viewer::provide(shell.viewer());
        crate::apps::InstalledApps::provide(shell.installed_apps());

        shell
    }

    /// The shell for this tree.
    ///
    /// The one panic the crate allows itself, and it cannot fire at runtime for
    /// a reason a viewer could reach: the context is provided by `<AppShell>`,
    /// every caller is inside one by construction, and a caller that is not is
    /// a wiring mistake visible the first time that screen is opened. Returning
    /// an `Option` here would push a `let else` with no sensible branch into
    /// several dozen components.
    #[allow(clippy::expect_used, reason = "provided by <AppShell>")]
    pub fn get() -> Self {
        use_context::<Self>().expect("Shell::get outside an <AppShell>")
    }

    /// The signed-in user, once the session has resolved.
    ///
    /// `None` is reachable in practice - a session can expire between the
    /// document being requested and this resolving - so the chrome renders
    /// without a menu rather than panicking. `landing` is what puts such a
    /// visitor back at the form.
    /// The signed-in account as a signal, for anything that needs it without
    /// being able to await - the permission-gated controls in
    /// [`crate::ui`], which know there is a viewer and nothing else about
    /// this shell.
    pub fn viewer(self) -> Signal<Option<AuthUser>> {
        let session = self.session;

        Signal::derive(move || session.get().flatten())
    }

    /// The apps this workspace has, as a signal. `None` until it resolves -
    /// see [`crate::apps::InstalledApps`] for why the two are worth telling
    /// apart.
    pub fn installed_apps(self) -> Signal<Option<Vec<String>>> {
        let apps = self.apps;

        Signal::derive(move || apps.get())
    }

    pub async fn user(self) -> Option<AuthUser> {
        self.session.await
    }

    /// Whether a group is showing its children.
    ///
    /// Reactive on both inputs: an explicit choice if one was made, the current
    /// trail otherwise.
    pub fn is_open(self, key: &'static str) -> bool {
        match self.overrides.with(|map| map.get(key).copied()) {
            Some(chosen) => chosen,
            None => self.trail.with(|trail| trail.contains(key)),
        }
    }

    /// Open a closed group or close an open one, and remember which.
    pub fn toggle(self, key: &'static str) {
        let now_open = !self.is_open(key);
        self.overrides.update(|map| {
            map.insert(key, now_open);
        });
    }

    pub fn open_palette(self) {
        self.palette_open.set(true);
    }

    pub fn toggle_drawer(self) {
        self.drawer_open.update(|open| *open = !*open);
    }

    pub fn close_drawer(self) {
        self.drawer_open.set(false);
    }

    pub fn close_palette(self) {
        self.palette_open.set(false);
    }

    /// Whether the navigation panel is showing labels or only its icon rail.
    ///
    /// A desktop question only: the drawer always shows labels, because it was
    /// opened deliberately and covers the content either way.
    pub fn collapsed(self) -> bool {
        self.theme.sidebar().is_collapsed()
    }
}

/// The chrome around every signed-in screen.
#[component]
pub fn app_shell(children: Children) -> impl IntoView {
    let shell = Shell::provide();

    // Ctrl+K anywhere, which is why it is on the window rather than on an
    // element: the palette has to open from a text field, a table row, or
    // nothing focused at all.
    //
    // `preventDefault` matters - Ctrl+K is the browser's own search-bar
    // shortcut in Firefox, and without it the address bar takes the keystroke
    // and the palette never sees it. Cmd+K for macOS, where Ctrl is not the
    // modifier anyone reaches for.
    Effect::new(move |_| {
        let handle = window_event_listener(leptos::ev::keydown, move |event| {
            let k = event.key().eq_ignore_ascii_case("k");

            if k && (event.ctrl_key() || event.meta_key()) {
                event.prevent_default();
                shell.palette_open.update(|open| *open = !*open);
            } else if event.key() == "Escape" {
                shell.close_palette();
                shell.close_drawer();
            }
        });

        // Removed when the shell is torn down, so a hot reload does not leave
        // two listeners fighting over the same keystroke.
        on_cleanup(move || handle.remove());
    });

    // A navigation closes the drawer. Without this, tapping an entry on a
    // phone leaves the panel covering the page it just opened.
    Effect::new(move |_| {
        shell.trail.track();
        shell.close_drawer();
    });

    view! {
        <div class="relative flex h-full w-full overflow-hidden bg-surface-shell text-content">
            <Sidebar />

            <div class="flex min-w-0 flex-1 flex-col">
                <TopBar />

                // The only scrolling region: the chrome stays put without
                // position: fixed, and a long table does not push the top bar
                // off the screen.
                //
                // `relative` is what makes that true, and it is load-bearing.
                // An absolutely positioned descendant with no positioned
                // ancestor is laid out against the document, not against this
                // box, and an ancestor's `overflow: hidden` does not clip it -
                // clipping only reaches descendants whose containing block is
                // inside the clipper. `sr-only` is `position: absolute`, so a
                // visually hidden 1px element two thousand pixels down a long
                // page adds two thousand pixels of scrollable height to the
                // document itself, and the whole shell - sidebar and top bar
                // with it - starts to scroll. Both this and the shell root are
                // positioned so that nothing inside either can reach the
                // document to do that.
                //
                // `children()` is the route outlet, and it is deliberately not
                // inside a Suspense: see the module docs.
                <main class="relative min-h-0 flex-1 overflow-y-auto bg-surface">
                    <div class="mx-auto w-full max-w-[1600px] px-3 py-3 sm:px-4 sm:py-4">
                        {children()}
                    </div>
                </main>
            </div>

            <CommandPalette />
        </div>
    }
}
