//! Drawing the app catalog.
//!
//! [`phonix_core::apps`] describes what a workspace can switch on. It names its
//! icon as a Lucide string rather than an [`Icon`], because `Icon` is three
//! hundred variants of SVG geometry that live in this crate, which sits above
//! `phonix-core`. This is the one place the string becomes a drawing.
//!
//! The table is small on purpose - one line per app - and the test below fails
//! the build if an app names an icon that has not been through
//! `tools/icons.txt`. So the declaration stays with the app and the resolution
//! stays with the drawings, and neither can drift without something going red.

use leptos::prelude::*;
use phonix_core::apps::AppDescriptor;

use crate::icons::Icon;

/// Which apps this workspace has switched on, for anything that has to draw
/// differently because of it.
///
/// The same shape as [`crate::ui::viewer::Viewer`] and for the same reason: the
/// launcher, the dashboard's opening card and the permission editor all need
/// this list, and three components each opening their own resource would be
/// three round trips for one constant-sized answer.
///
/// # `None` means "not yet", and it is not the same as "nothing"
///
/// An empty list is a real answer - a workspace with only core. Unresolved has
/// to be told apart from it, because the two want opposite defaults: a
/// permission editor that treated "not yet" as "nothing" would draw a third of
/// its tree, then grow. Where the difference matters, `None` should mean *show
/// everything* and let the answer narrow it, which is the way round that never
/// hides a control somebody was reaching for.
#[derive(Clone, Copy)]
pub struct InstalledApps(pub Signal<Option<Vec<String>>>);

impl InstalledApps {
    /// Make the list available to everything rendered below. The shell calls
    /// this once.
    pub fn provide(apps: Signal<Option<Vec<String>>>) {
        provide_context(Self(apps));
    }

    /// The list, or a permanently unresolved signal where no host has provided
    /// one - a test rendering a component on its own, most often.
    pub fn get() -> Signal<Option<Vec<String>>> {
        use_context::<Self>().map_or_else(|| Signal::derive(|| None), |apps| apps.0)
    }
}

/// The drawing for an app's declared icon.
///
/// Falls back to a generic package rather than refusing: an icon nobody
/// remembered to add is a blemish, and a blank tile in the store would be a
/// worse one. The test is what stops it happening.
pub fn icon_of(app: &AppDescriptor) -> Icon {
    match app.icon {
        "layout-dashboard" => Icon::LayoutDashboard,
        "boxes" => Icon::Boxes,
        "file-text" => Icon::FileText,
        _ => Icon::Package,
    }
}

#[cfg(test)]
mod tests {
    use phonix_core::apps::CATALOG;

    use super::*;

    #[test]
    fn every_app_in_the_catalog_has_a_drawing() {
        // The fallback exists so a missing entry is a blemish rather than a
        // blank tile. This is what stops it being reached.
        for app in CATALOG {
            assert_ne!(
                icon_of(app),
                Icon::Package,
                "{} declares icon '{}', which is not in the table here - add it, \
                 and add the name to tools/icons.txt if it is not there either",
                app.id,
                app.icon,
            );
        }
    }
}
