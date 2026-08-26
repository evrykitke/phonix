//! Where a lookup's panel sits.
//!
//! "Wherever the field is" is the whole requirement, and it is less trivial
//! than it sounds because a field near the bottom of a long form has no room
//! below it and a field near the right edge has no room beside it. So the
//! panel is measured from the trigger when it opens and clamped into the
//! viewport, exactly as the grid's row menu is - and for the same underlying
//! reason.
//!
//! # Fixed, not absolute
//!
//! A lookup can be inside a form inside a panel inside the shell's scrolling
//! content column, and it can be inside a table cell inside `overflow-x-auto`.
//! An absolutely positioned panel is clipped by the first of those ancestors
//! that scrolls, which shows up as a dropdown that opens and is cut in half.
//! `position: fixed` takes it out of all of them, at the price of not
//! travelling with the page - so anything that would move it closes it
//! instead. See the listeners in [`super`].

/// Where a panel sits, in viewport coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct At {
    pub left: f64,
    pub width: f64,
    /// Distance from the top, when the panel hangs below the field.
    pub top: Option<f64>,
    /// Distance from the bottom, when it opens upwards instead.
    pub bottom: Option<f64>,
    /// How tall it may get before it scrolls inside itself.
    pub max_height: f64,
}

impl Default for At {
    fn default() -> Self {
        Self {
            left: 0.0,
            width: 0.0,
            top: Some(0.0),
            bottom: None,
            max_height: 0.0,
        }
    }
}

impl At {
    pub fn style(self) -> String {
        let vertical = match (self.top, self.bottom) {
            (Some(top), _) => format!("top:{top}px"),
            (_, Some(bottom)) => format!("bottom:{bottom}px"),
            _ => "top:0".to_owned(),
        };

        format!(
            "position:fixed;left:{}px;width:{}px;{vertical};max-height:{}px",
            self.left, self.width, self.max_height,
        )
    }
}

/// The gap between the field and its panel, and between the panel and the edge
/// of the screen.
const GAP: f64 = 4.0;

/// The height a panel would like to have.
///
/// Not a maximum - a panel is only ever as tall as its content - but the
/// figure the two sides are judged against. Below it, a panel is being made to
/// fit rather than being given room, and if the other side has more it goes
/// there instead.
///
/// It was 120px, which is two rows and a scrollbar. That is the point at which
/// a dropdown is *unusable*, and judging against it meant a field with 130px
/// underneath it opened downwards into a box showing two of its twelve
/// options - not clipped, but indistinguishable from clipped.
const ROOMY: f64 = 240.0;

/// Work out where a panel of `wanted` width belongs, given the field's
/// rectangle and the size of the window.
///
/// Split out from the measuring so it can be tested without a browser: every
/// decision worth getting right is in here, and the caller only reads numbers
/// off the DOM.
pub(super) fn fit(field: Rect, viewport: Size, wanted: f64) -> At {
    // Never narrower than the field it belongs to - a panel that is is a
    // dropdown that does not look attached to anything - and never wider than
    // the window it opens in.
    let width = wanted
        .max(field.width)
        .min((viewport.width - GAP * 2.0).max(0.0));

    // A gap at the field, and a gap at the window edge. Without the second one
    // a panel that uses all the room it is given sits flush against the bottom
    // of the screen, which reads as the browser having cut it off.
    let below = (viewport.height - field.bottom - GAP * 2.0).max(0.0);
    let above = (field.top - GAP * 2.0).max(0.0);

    // Left-aligned with the field until that would push it off the right edge,
    // at which point it slides back rather than growing off-screen.
    let left = field
        .left
        .min(viewport.width - width - GAP)
        .max(GAP);

    // Downwards is the default because that is where a reader's eye already
    // is. Upwards when below cannot give a panel the room it wants *and* above
    // has more, so a field on a short window does not flip to the side with
    // even less room than it started with.
    let upwards = below < ROOMY && above > below;

    At {
        left,
        width,
        top: (!upwards).then_some(field.bottom + GAP),
        bottom: upwards.then_some(viewport.height - field.top + GAP),
        max_height: if upwards { above } else { below },
    }
}

/// Measure the field and work out where its panel goes.
///
/// Browser only. The interfaces are asked for in the hydrate build alone, and
/// the server never opens a panel. The arithmetic is all in [`fit`], which is
/// why this function has nothing in it worth testing.
#[cfg(feature = "hydrate")]
pub(super) fn of(anchor: leptos::prelude::NodeRef<leptos::html::Div>, wanted: f64) -> At {
    use leptos::prelude::*;

    // Nothing to measure means nothing on screen to measure it against, which
    // is the same answer the server gives.
    let Some(element) = anchor.try_get_untracked().flatten() else {
        return fit(Rect::NOWHERE, Size::NOTHING, wanted);
    };

    let rect = element.get_bounding_client_rect();
    let viewport = Size {
        width: window()
            .inner_width()
            .ok()
            .and_then(|width| width.as_f64())
            .unwrap_or(0.0),
        height: window()
            .inner_height()
            .ok()
            .and_then(|height| height.as_f64())
            .unwrap_or(0.0),
    };

    fit(
        Rect {
            left: rect.left(),
            top: rect.top(),
            bottom: rect.bottom(),
            width: rect.width(),
        },
        viewport,
        wanted,
    )
}

/// The same answer for a build that cannot measure anything.
///
/// The server never opens a panel, so this is only ever the value the signal
/// starts at. It goes through [`fit`] all the same, which is what keeps the
/// arithmetic compiled - and therefore tested - in the build the suite runs in.
#[cfg(not(feature = "hydrate"))]
pub(super) fn of(_anchor: leptos::prelude::NodeRef<leptos::html::Div>, wanted: f64) -> At {
    fit(Rect::NOWHERE, Size::NOTHING, wanted)
}

/// The part of a `DomRect` this needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Rect {
    pub left: f64,
    pub top: f64,
    pub bottom: f64,
    pub width: f64,
}

impl Rect {
    /// A field that has not been measured.
    pub const NOWHERE: Self = Self {
        left: 0.0,
        top: 0.0,
        bottom: 0.0,
        width: 0.0,
    };
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    /// A window nobody has looked at.
    pub const NOTHING: Self = Self {
        width: 0.0,
        height: 0.0,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Size = Size {
        width: 1000.0,
        height: 800.0,
    };

    fn field(left: f64, top: f64) -> Rect {
        Rect {
            left,
            top,
            bottom: top + 32.0,
            width: 200.0,
        }
    }

    #[test]
    fn a_panel_hangs_below_the_field_when_there_is_room() {
        let at = fit(field(100.0, 100.0), WINDOW, 200.0);

        assert_eq!(at.top, Some(136.0));
        assert_eq!(at.bottom, None);
        assert_eq!(at.left, 100.0);
    }

    #[test]
    fn a_field_near_the_bottom_opens_upwards() {
        let at = fit(field(100.0, 740.0), WINDOW, 200.0);

        assert_eq!(at.top, None);
        assert_eq!(at.bottom, Some(64.0));
        // And it may only grow into the room it actually has above.
        assert_eq!(at.max_height, 740.0 - GAP * 2.0);
    }

    #[test]
    fn a_field_with_too_little_under_it_flips_rather_than_being_squeezed() {
        // 130px below is enough to draw a panel and not enough to read one.
        // Opening downwards there is what looked like a clipped dropdown.
        let at = fit(field(100.0, 630.0), WINDOW, 200.0);

        assert_eq!(at.top, None);
        assert!(at.bottom.is_some());
        assert!(at.max_height > ROOMY);
    }

    #[test]
    fn a_panel_never_touches_the_bottom_of_the_window() {
        // Flush against the edge reads as the browser having cut it off.
        let at = fit(field(100.0, 100.0), WINDOW, 200.0);
        let top = at.top.unwrap_or_default();

        assert!(top + at.max_height <= WINDOW.height - GAP);
    }

    #[test]
    fn a_short_window_does_not_flip_to_the_side_with_even_less_room() {
        // Neither side is roomy. Staying put beats moving to somewhere worse.
        let squat = Size {
            width: 1000.0,
            height: 300.0,
        };
        let at = fit(field(100.0, 40.0), squat, 200.0);

        assert!(at.top.is_some());
    }

    #[test]
    fn a_panel_never_leaves_the_right_edge() {
        // A wide picker on a field close to the edge slides left rather than
        // hanging off the screen, which is the failure this exists to stop.
        let at = fit(field(900.0, 100.0), WINDOW, 640.0);

        assert!(at.left + at.width <= WINDOW.width - GAP);
        assert_eq!(at.left, 1000.0 - 640.0 - GAP);
    }

    #[test]
    fn a_panel_is_at_least_as_wide_as_its_field() {
        // The list presentation asks for nothing and gets the field's width,
        // which is what makes it read as part of the control.
        let at = fit(field(100.0, 100.0), WINDOW, 0.0);

        assert_eq!(at.width, 200.0);
    }

    #[test]
    fn a_panel_wider_than_the_window_is_cut_down_to_it() {
        let narrow = Size {
            width: 380.0,
            height: 800.0,
        };
        let at = fit(field(8.0, 100.0), narrow, 640.0);

        assert_eq!(at.width, 380.0 - GAP * 2.0);
        assert_eq!(at.left, GAP);
    }
}
