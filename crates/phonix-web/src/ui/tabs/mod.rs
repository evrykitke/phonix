//! Tabs: one screen's worth of content, grouped, with one group on show.
//!
//! # What it is for
//!
//! The same argument as the [data grid](crate::ui::table) and the
//! [entity form](crate::ui::form). A settings screen and an account screen both
//! ended up as a column of stacked panels, which reads as one long scroll where
//! nothing is findable and half the viewport is whitespace beside a narrow
//! form. Grouping them is not decoration - it is what lets a screen add a
//! seventh panel without becoming unusable.
//!
//! ```ignore
//! <TabbedPanel id="account" tabs=vec![
//!     Tab::new("profile", "Profile", || view! { <Profile /> }.into_any()),
//!     Tab::new("security", "Security", || view! { <Security /> }.into_any()),
//! ] />
//! ```
//!
//! # The active tab lives in the URL
//!
//! `?tab=security`, not a local signal. Three things follow from that, and all
//! three are the reason:
//!
//! * a reload keeps you where you were, instead of throwing you back to the
//!   first tab;
//! * a tab can be linked to - "your recovery codes are under
//!   `/account?tab=security`" is a URL somebody can send;
//! * **the server and the browser agree on what to render.** The active tab is
//!   derived from the request either way, so the markup the server streams is
//!   the markup hydration expects. A local signal defaulting to the first tab
//!   would render tab one on the server and tab three in the browser, and a
//!   hydration mismatch does not degrade - it panics and kills every handler on
//!   the page.
//!
//! Switching tabs *replaces* the history entry rather than pushing one. A tab
//! is a view of one page, not a new place; pushing would make Back walk
//! backwards through tabs instead of leaving the screen.
//!
//! # Only the active tab is rendered
//!
//! Not rendered-and-hidden. A hidden panel that still exists is a second copy
//! of every input on the screen, with the same ids and the same labels, which
//! is what makes a screen reader read a form twice and a `getElementById` find
//! the wrong one.
//!
//! The consequence is that a tab's *local* state resets when you leave it. That
//! is usually right - a half-typed password should not survive a trip to
//! another tab - but a screen whose tabs share an edit buffer has to declare
//! that buffer above the tabs, where it outlives them. The settings screen does
//! exactly that: one form, one save, panels grouped across tabs.

use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::{use_location, use_navigate, use_query_map};
use phonix_core::identity::AuthUser;

use crate::icons::{Icon, IconSize};
use crate::ui::viewer::Viewer;

/// The query parameter the active tab is read from and written to.
const TAB_PARAM: &str = "tab";

/// What renders when a tab is the one on show.
type Render = Arc<dyn Fn() -> AnyView + Send + Sync>;

/// One tab.
pub struct Tab {
    /// The value that appears in `?tab=`. Stable: it is in URLs people keep.
    pub key: &'static str,
    /// What the tab strip reads. A `String` and not a `&'static str`, unlike
    /// `key`: the key is machinery that belongs in a URL, the label is a
    /// sentence and comes out of the catalog.
    pub label: String,
    pub icon: Option<Icon>,
    /// The permission needed to see it at all.
    ///
    /// A tab **hides**, unlike a [field](crate::ui::form::Field). The rule is
    /// the same one the rest of the kit follows: hiding is right when the thing
    /// hidden is a way in, and wrong when it is a value that would be submitted
    /// as something else. A tab is a way in. And as everywhere else, this is
    /// cosmetic - whatever the tab contains states its own permission where it
    /// reads or writes.
    pub permission: Option<&'static str>,
    render: Render,
}

impl Clone for Tab {
    fn clone(&self) -> Self {
        Self {
            key: self.key,
            label: self.label.clone(),
            icon: self.icon,
            permission: self.permission,
            render: Arc::clone(&self.render),
        }
    }
}

impl Tab {
    pub fn new(
        key: &'static str,
        label: impl Into<String>,
        render: impl Fn() -> AnyView + Send + Sync + 'static,
    ) -> Self {
        Self {
            key,
            label: label.into(),
            icon: None,
            permission: None,
            render: Arc::new(render),
        }
    }

    #[must_use]
    pub const fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    #[must_use]
    pub const fn require(mut self, permission: &'static str) -> Self {
        self.permission = Some(permission);
        self
    }

    /// Whether this viewer may see the tab.
    ///
    /// Nobody is nobody: while the session is still resolving, `user` is `None`
    /// and a gated tab stays hidden. Showing it for the moment before the
    /// answer arrives would flash content at somebody who may not have it.
    pub fn visible_to(&self, user: Option<&AuthUser>) -> bool {
        match self.permission {
            None => true,
            Some(permission) => user.is_some_and(|user| user.can(permission)),
        }
    }

    pub fn view(&self) -> AnyView {
        (self.render)()
    }
}

/// Which tab is on show, given what the URL asks for and what exists.
///
/// Falls back to the first visible tab rather than rendering nothing: a URL
/// naming a tab that has been renamed, or one the viewer may not see, is a
/// stale bookmark - and a blank screen is a worse answer to it than the front
/// tab.
fn active<'a>(tabs: &'a [Tab], requested: Option<&str>) -> Option<&'a Tab> {
    requested
        .and_then(|key| tabs.iter().find(|tab| tab.key == key))
        .or_else(|| tabs.first())
}

/// A tab strip and the content of whichever tab is on show.
#[component]
pub fn tabbed_panel(
    /// Distinguishes this strip's ids from any other on the page.
    id: &'static str,
    tabs: Vec<Tab>,
) -> impl IntoView {
    let viewer = Viewer::get();
    let query = use_query_map();
    let navigate = use_navigate();
    let location = use_location();

    let tabs = StoredValue::new(tabs);

    // A `Callback` rather than a bare closure because it is handed to one click
    // handler per tab, and a closure would be moved into the first of them.
    //
    // Rebuilt from the live path and query each time it runs: switching tabs
    // must not discard the rest of the query string, and must not carry a stale
    // path if the route has changed under us.
    let go = Callback::new(move |key: &'static str| {
        let mut params = query.get_untracked();
        params.replace(TAB_PARAM, key.to_owned());

        let path = location.pathname.get_untracked();
        let search = params.to_query_string();

        navigate(
            &format!("{path}{search}"),
            // Replace, not push: a tab is a view of this page, not a new
            // place. Pushing would make Back walk backwards through tabs.
            NavigateOptions {
                replace: true,
                ..Default::default()
            },
        );
    });

    view! {
        <div class="space-y-3">
            <div
                role="tablist"
                aria-label=crate::l!("common.sections")
                // Scrolls rather than wrapping: a wrapped strip changes height
                // as tabs are added, which moves the content under the pointer.
                class="flex gap-1 overflow-x-auto border-b border-edge"
            >
                {move || {
                    let user = viewer.get();
                    let requested = query.with(|params| params.get(TAB_PARAM));

                    let visible: Vec<Tab> = tabs
                        .with_value(|tabs| {
                            tabs.iter().filter(|tab| tab.visible_to(user.as_ref())).cloned().collect()
                        });

                    let current = active(&visible, requested.as_deref()).map(|tab| tab.key);

                    visible
                        .into_iter()
                        .map(|tab| {
                            let selected = current == Some(tab.key);
                            let key = tab.key;
                            let icon = tab.icon;
                            let label = tab.label.clone();

                            let class = if selected {
                                "-mb-px inline-flex items-center gap-1.5 whitespace-nowrap border-b-2 \
                                 border-brand px-3 py-2 text-sm font-medium text-content"
                            } else {
                                "-mb-px inline-flex items-center gap-1.5 whitespace-nowrap border-b-2 \
                                 border-transparent px-3 py-2 text-sm text-content-muted \
                                 hover:border-edge hover:text-content"
                            };

                            view! {
                                <button
                                    type="button"
                                    role="tab"
                                    id=format!("{id}-tab-{key}")
                                    aria-controls=format!("{id}-panel-{key}")
                                    aria-selected=if selected { "true" } else { "false" }
                                    class=class
                                    on:click=move |_| go.run(key)
                                >
                                    {icon.map(|icon| view! { <Icon icon=icon size=IconSize::Xs /> })}
                                    {label}
                                </button>
                            }
                        })
                        .collect::<Vec<_>>()
                }}
            </div>

            {move || {
                let user = viewer.get();
                let requested = query.with(|params| params.get(TAB_PARAM));

                let visible: Vec<Tab> = tabs
                    .with_value(|tabs| {
                        tabs.iter().filter(|tab| tab.visible_to(user.as_ref())).cloned().collect()
                    });

                active(&visible, requested.as_deref())
                    .map(|tab| {
                        view! {
                            <div
                                role="tabpanel"
                                id=format!("{id}-panel-{}", tab.key)
                                aria-labelledby=format!("{id}-tab-{}", tab.key)
                            >
                                {tab.view()}
                            </div>
                        }
                    })
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use phonix_core::authorization::PermissionSet;
    use phonix_core::identity::{UserId, UserStatus};

    use super::*;

    fn tab(key: &'static str) -> Tab {
        Tab::new(key, key, || ().into_any())
    }

    fn viewer(permissions: PermissionSet) -> AuthUser {
        AuthUser {
            id: UserId::nil(),
            email: "viewer@example.test".to_owned(),
            first_name: "V".to_owned(),
            last_name: "Iewer".to_owned(),
            display_name: "V Iewer".to_owned(),
            roles: Vec::new(),
            permissions,
            is_owner: false,
            status: UserStatus::Active,
            mfa_satisfied: true,
            mfa_enabled: false,
            email_verified: true,
        }
    }

    #[test]
    fn the_url_decides_which_tab_is_on_show() {
        let tabs = [tab("profile"), tab("password")];

        assert_eq!(
            active(&tabs, Some("password")).map(|t| t.key),
            Some("password")
        );
    }

    #[test]
    fn no_tab_asked_for_is_the_first_one() {
        let tabs = [tab("profile"), tab("password")];

        assert_eq!(active(&tabs, None).map(|t| t.key), Some("profile"));
    }

    #[test]
    fn a_stale_bookmark_lands_on_the_front_tab_rather_than_on_nothing() {
        // The tab was renamed, or the viewer may not see it. A blank screen is
        // a worse answer to a stale URL than the front tab.
        let tabs = [tab("profile"), tab("password")];

        assert_eq!(
            active(&tabs, Some("billing")).map(|t| t.key),
            Some("profile")
        );
    }

    #[test]
    fn a_screen_with_no_visible_tabs_renders_nothing_rather_than_panicking() {
        assert!(active(&[], Some("profile")).is_none());
    }

    #[test]
    fn a_gated_tab_is_hidden_from_a_viewer_without_the_permission() {
        let gated = tab("audit").require(phonix_core::permissions::AUDIT_LOGS);

        assert!(!gated.visible_to(Some(&viewer(PermissionSet::new()))));
        assert!(gated.visible_to(Some(&viewer(PermissionSet::all()))));
    }

    #[test]
    fn a_gated_tab_stays_hidden_while_nobody_is_known_yet() {
        let gated = tab("audit").require(phonix_core::permissions::AUDIT_LOGS);

        assert!(!gated.visible_to(None));
    }

    #[test]
    fn an_ungated_tab_is_shown_to_anybody_who_can_open_the_screen() {
        assert!(tab("profile").visible_to(None));
    }
}
