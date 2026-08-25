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

use phonix_core::apps::AppDescriptor;

use crate::icons::Icon;

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
