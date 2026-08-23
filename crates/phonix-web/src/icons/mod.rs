//! The icon factory.
//!
//! # Why Lucide, and why generated
//!
//! Lucide is ISC-licensed, drawn on a 24x24 grid with a 2px `currentColor`
//! stroke, and - the part that matters here - each icon is plain geometry with
//! no wrapper worth keeping. That lets an icon be a `&'static str` of markup
//! instead of a component, so the whole library costs one enum, one `<svg>`,
//! and nothing at runtime.
//!
//! The alternative was `leptos_icons` + `icondata`. It is the idiomatic Leptos
//! choice and it was rejected on size: `icondata` carries roughly twenty
//! thousand icons across two dozen sets, and paying for that in compile time
//! and bundle weight to draw the fifty we actually use is a bad trade. The
//! curated list in `tools/icons.txt` is the price of admission instead - adding
//! an icon is a deliberate edit, which is what stops the bundle drifting.
//!
//! # Using one
//!
//! ```ignore
//! view! {
//!     <Icon icon=Icon::Settings />                       // 16px, inherits colour
//!     <Icon icon=Icon::Users size=IconSize::Md />
//!     <Icon icon=Icon::TriangleAlert class="text-danger" />
//! }
//! ```
//!
//! The component and the enum share a name deliberately: `Icon::Settings` names
//! the drawing, `<Icon/>` draws it. Rust keeps the two apart because one lives
//! in the type namespace and the other in the value namespace.
//!
//! Colour is never set here. Every icon strokes `currentColor`, so it takes the
//! colour of whatever it sits inside - which is what makes one icon work in a
//! muted sidebar row and in a brand-coloured button without a variant for each.

mod generated;

pub use generated::Icon;

use leptos::prelude::*;

/// The sizes the interface actually uses.
///
/// A closed set rather than a free number: an icon that is 17px because someone
/// typed 17 is the kind of drift that makes a compact layout look untidy, and
/// these line up with the type scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IconSize {
    /// 14px - inline with `text-xs`, dropdown adornments, chevrons.
    Xs,
    /// 16px - the default. Sidebar rows, buttons, form adornments.
    #[default]
    Sm,
    /// 20px - top bar actions, empty-state headings.
    Md,
    /// 24px - Lucide's native size. Feature illustrations only.
    Lg,
}

impl IconSize {
    /// Edge length in CSS pixels.
    pub const fn px(self) -> u16 {
        match self {
            Self::Xs => 14,
            Self::Sm => 16,
            Self::Md => 20,
            Self::Lg => 24,
        }
    }

    /// Stroke width that keeps the drawing's weight even as it shrinks.
    ///
    /// Lucide draws at 2px on a 24px grid. Scaled down without compensation a
    /// 14px icon reads noticeably heavier than the 13px text beside it, which
    /// is the single most common way a Lucide interface ends up looking blunt.
    pub const fn stroke(self) -> &'static str {
        match self {
            Self::Xs | Self::Sm => "1.75",
            Self::Md => "1.75",
            Self::Lg => "2",
        }
    }
}

/// Draw an icon.
///
/// Hidden from assistive technology by default: an icon beside a label is
/// decoration, and a screen reader announcing "settings settings" is worse than
/// silence. Pass `label` for the case where the icon *is* the control - an
/// icon-only button - and it becomes an `img` with an accessible name.
#[component]
pub fn icon(
    icon: Icon,
    #[prop(optional)] size: IconSize,
    /// Extra classes. Sizing comes from `size`, so this is for colour and
    /// layout: `"text-danger"`, `"shrink-0"`, `"rotate-90"`.
    #[prop(optional, into)]
    class: &'static str,
    /// Accessible name. Omit for decoration.
    #[prop(optional, into)]
    label: Option<&'static str>,
) -> impl IntoView {
    let px = size.px();

    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            width=px
            height=px
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width=size.stroke()
            stroke-linecap="round"
            stroke-linejoin="round"
            class=move || format!("inline-block shrink-0 {class}")
            aria-hidden=move || label.is_none().then_some("true")
            role=move || label.map(|_| "img")
            aria-label=label
            // Compile-time constant from `generated.rs`, which is produced by
            // `tools/generate-icons.mjs` from vendored SVG files. No user input
            // reaches this.
            inner_html=icon.body()
        ></svg>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::str::FromStr;

    #[test]
    fn every_icon_has_geometry_and_a_name() {
        for icon in Icon::ALL {
            assert!(!icon.key().is_empty(), "{icon:?} has no key");
            assert!(!icon.body().is_empty(), "{icon:?} has no body");
            // The generator strips the wrapper; if one survived, the component's
            // size and stroke would be silently overridden by the inner tag.
            assert!(
                !icon.body().contains("<svg"),
                "{icon:?} kept its <svg> wrapper"
            );
        }
    }

    #[test]
    fn keys_round_trip() {
        for icon in Icon::ALL {
            assert_eq!(Icon::from_str(icon.key()), Ok(*icon));
        }
        assert!(Icon::from_str("no-such-icon").is_err());
    }

    #[test]
    fn keys_are_unique() {
        let mut keys: Vec<&str> = Icon::ALL.iter().map(|icon| icon.key()).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "two variants share a lucide name");
    }

    #[test]
    fn icons_shrink_without_getting_heavier() {
        // Guards the intent of `stroke()`: nothing below 24px may draw at the
        // full 2px, or small icons out-weigh the text beside them.
        for size in [IconSize::Xs, IconSize::Sm, IconSize::Md] {
            assert_ne!(size.stroke(), "2", "{size:?} draws at full weight");
        }
        assert_eq!(IconSize::Lg.stroke(), "2");
    }
}
