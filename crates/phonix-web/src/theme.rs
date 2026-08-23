//! Appearance: light/dark and the accent colour.
//!
//! # Where the preference lives
//!
//! In a cookie, not in `localStorage`. The difference matters at exactly one
//! moment - the first paint. The server has to know the choice *while* it
//! renders the document, or the page arrives in the default theme and flips
//! once the bundle boots, which is the white flash every dark-mode site with
//! client-only persistence has. A cookie is on the request; `localStorage` is
//! not.
//!
//! It is deliberately not `HttpOnly`: this is a display preference, not a
//! credential, and the browser half has to read it back after a client-side
//! navigation. It is host-only and `SameSite=Lax`, like the session cookie, so
//! one workspace cannot set another's.
//!
//! Per-device rather than per-account, for now. Storing it on the user row
//! would follow someone between machines, which is a better answer and a
//! bigger one - it needs a write on every toggle and a merge rule when the two
//! disagree. [`ThemePreference`] is the shape either would produce.
//!
//! # How it reaches the CSS
//!
//! Two attributes on `<html>`, and nothing else:
//!
//! ```html
//! <html data-theme="dark" data-accent="violet">
//! ```
//!
//! `style/main.css` keys its semantic tokens off those. No component knows
//! which theme is active - they use `bg-surface` and `text-brand`, and the
//! attributes decide what those mean.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

/// The cookie both halves read and write.
pub const COOKIE_NAME: &str = "phonix_appearance";

/// A year. The preference is not worth re-asking for.
pub const COOKIE_MAX_AGE_SECS: i64 = 60 * 60 * 24 * 365;

/// Light, dark, or whatever the operating system says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeMode {
    /// Follow `prefers-color-scheme`. The default, and the only one that keeps
    /// following the OS when it changes at sunset.
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub const ALL: &'static [ThemeMode] = &[Self::System, Self::Light, Self::Dark];

    /// The stored form.
    pub const fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|mode| mode.key() == key)
    }

    /// The `data-theme` value, or `None` when the OS decides.
    ///
    /// `System` deliberately writes no attribute rather than writing
    /// `"system"`: the CSS then falls through to `prefers-color-scheme`, and a
    /// media query cannot be spelled as an attribute value.
    pub const fn attribute(self) -> Option<&'static str> {
        match self {
            Self::System => None,
            Self::Light => Some("light"),
            Self::Dark => Some("dark"),
        }
    }
}

/// The accent hue - what `--brand` resolves to.
///
/// A closed set, because each one is a hand-tuned ramp in `style/main.css` with
/// a light and a dark variant. A free colour picker would have to derive those
/// at runtime and would produce unreadable pairings on the way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Accent {
    /// The default. Close to the violet ClickUp leads with.
    #[default]
    Violet,
    Blue,
    Teal,
    Green,
    Amber,
    Rose,
}

impl Accent {
    pub const ALL: &'static [Accent] = &[
        Self::Violet,
        Self::Blue,
        Self::Teal,
        Self::Green,
        Self::Amber,
        Self::Rose,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Violet => "violet",
            Self::Blue => "blue",
            Self::Teal => "teal",
            Self::Green => "green",
            Self::Amber => "amber",
            Self::Rose => "rose",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Violet => "Violet",
            Self::Blue => "Blue",
            Self::Teal => "Teal",
            Self::Green => "Green",
            Self::Amber => "Amber",
            Self::Rose => "Rose",
        }
    }

    /// A swatch colour for the picker.
    ///
    /// Fixed rather than `var(--brand)`: the swatches are shown side by side and
    /// have to look like themselves, not like the accent currently in force.
    pub const fn swatch(self) -> &'static str {
        match self {
            Self::Violet => "oklch(0.58 0.22 292)",
            Self::Blue => "oklch(0.58 0.19 255)",
            Self::Teal => "oklch(0.62 0.13 195)",
            Self::Green => "oklch(0.62 0.16 149)",
            Self::Amber => "oklch(0.72 0.16 70)",
            Self::Rose => "oklch(0.62 0.2 12)",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|accent| accent.key() == key)
    }
}

/// Whether the navigation panel is showing labels or just its icon rail.
///
/// Stored beside the colour choice because it is the same kind of thing - a
/// per-device display preference the server has to know *before* it renders, or
/// the panel arrives wide and snaps narrow once the bundle boots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SidebarState {
    #[default]
    Expanded,
    Collapsed,
}

impl SidebarState {
    pub const ALL: &'static [SidebarState] = &[Self::Expanded, Self::Collapsed];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Expanded => "wide",
            Self::Collapsed => "rail",
        }
    }

    pub const fn is_collapsed(self) -> bool {
        matches!(self, Self::Collapsed)
    }

    pub const fn toggled(self) -> Self {
        match self {
            Self::Expanded => Self::Collapsed,
            Self::Collapsed => Self::Expanded,
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|state| state.key() == key)
    }
}

/// What the viewer has chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ThemePreference {
    pub mode: ThemeMode,
    pub accent: Accent,
    pub sidebar: SidebarState,
}

impl ThemePreference {
    /// Encode for the cookie: `"dark:violet:rail"`.
    ///
    /// One cookie rather than three, so a toggle is one write and the parts can
    /// never be read out of step. Positional, and read back leniently, so a
    /// cookie written by an older build - which had no third field - still
    /// decodes rather than resetting someone's theme on deploy day.
    pub fn encode(self) -> String {
        format!(
            "{}:{}:{}",
            self.mode.key(),
            self.accent.key(),
            self.sidebar.key()
        )
    }

    /// Decode, falling back per field.
    ///
    /// A malformed or half-recognised value yields the default for that field
    /// rather than an error: this is a cookie the user's own browser hands back,
    /// and an old or truncated one should render the app, not break it.
    pub fn decode(raw: &str) -> Self {
        let mut fields = raw.split(':').map(str::trim);

        Self {
            mode: fields.next().and_then(ThemeMode::parse).unwrap_or_default(),
            accent: fields.next().and_then(Accent::parse).unwrap_or_default(),
            sidebar: fields
                .next()
                .and_then(SidebarState::parse)
                .unwrap_or_default(),
        }
    }

    /// The preference in force for this render.
    ///
    /// Both halves read the same cookie, and that is the point: the server
    /// reads it off the request so the very first HTML already carries the
    /// right attributes, and the browser reads it back off `document.cookie`
    /// during hydration so the two agree. Seeding the client with the default
    /// instead would leave the appearance menu ticking "System / Violet" over a
    /// page that is plainly dark and teal.
    pub fn from_request() -> Self {
        #[cfg(feature = "ssr")]
        {
            let Some(parts) = use_context::<http::request::Parts>() else {
                return Self::default();
            };

            let Some(raw) = parts
                .headers
                .get(http::header::COOKIE)
                .and_then(|value| value.to_str().ok())
            else {
                return Self::default();
            };

            read_cookie(raw, COOKIE_NAME)
                .map(|value| Self::decode(&value))
                .unwrap_or_default()
        }

        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::JsCast;

            document()
                .dyn_ref::<web_sys::HtmlDocument>()
                .and_then(|document| document.cookie().ok())
                .and_then(|cookies| read_cookie(&cookies, COOKIE_NAME))
                .map(|value| Self::decode(&value))
                .unwrap_or_default()
        }

        #[cfg(not(any(feature = "ssr", feature = "hydrate")))]
        Self::default()
    }
}

/// Find one cookie in a `Cookie:` header, or in `document.cookie`.
///
/// The two have the same `name=value; name=value` shape, which is why one
/// function serves both. Split on `;` and then on the *first* `=`, because a
/// cookie value may contain one itself - base64 padding is the usual case - and
/// splitting on every one would truncate it.
#[cfg(any(feature = "ssr", feature = "hydrate"))]
pub(crate) fn read_cookie(header: &str, name: &str) -> Option<String> {
    header.split(';').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        if key.trim() == name {
            Some(value.trim().to_owned())
        } else {
            None
        }
    })
}

/// The live preference, and the setters that change it.
///
/// Provided once by the shell and read by the appearance menu. A signal rather
/// than a plain value because the menu shows which option is selected, and that
/// has to update the instant it is clicked.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    preference: RwSignal<ThemePreference>,
}

impl Theme {
    /// Seed the context from what the document was rendered with.
    pub fn provide(initial: ThemePreference) -> Self {
        let theme = Self {
            preference: RwSignal::new(initial),
        };
        provide_context(theme);
        theme
    }

    /// The theme for this tree.
    ///
    /// Falls back to the default rather than panicking when no shell provided
    /// one: a component rendered on its own in a test should not explode over
    /// its colour scheme.
    pub fn get() -> Self {
        use_context::<Self>().unwrap_or_else(|| Self {
            preference: RwSignal::new(ThemePreference::default()),
        })
    }

    pub fn preference(self) -> ThemePreference {
        self.preference.get()
    }

    pub fn mode(self) -> ThemeMode {
        self.preference.get().mode
    }

    pub fn accent(self) -> Accent {
        self.preference.get().accent
    }

    pub fn sidebar(self) -> SidebarState {
        self.preference.get().sidebar
    }

    pub fn set_mode(self, mode: ThemeMode) {
        self.preference.update(|preference| preference.mode = mode);
        self.apply();
    }

    pub fn set_accent(self, accent: Accent) {
        self.preference
            .update(|preference| preference.accent = accent);
        self.apply();
    }

    /// Collapse the navigation panel to its icon rail, or open it again.
    pub fn toggle_sidebar(self) {
        self.preference
            .update(|preference| preference.sidebar = preference.sidebar.toggled());
        self.apply();
    }

    /// Write the choice to `<html>` and to the cookie.
    ///
    /// Both, and in that order: the attribute is what the eye sees this instant,
    /// the cookie is what the *next* server render will see. Skipping the cookie
    /// would make the theme revert on the next full page load.
    fn apply(self) {
        #[cfg(feature = "hydrate")]
        {
            let preference = self.preference.get_untracked();

            if let Some(root) = document().document_element() {
                match preference.mode.attribute() {
                    Some(value) => {
                        let _ = root.set_attribute("data-theme", value);
                    }
                    // Removed, not set to "system": the CSS falls through to
                    // `prefers-color-scheme` only in the attribute's absence.
                    None => {
                        let _ = root.remove_attribute("data-theme");
                    }
                }

                let _ = root.set_attribute("data-accent", preference.accent.key());
            }

            write_cookie(&preference.encode());
        }
    }
}

/// Persist the preference for the next request.
///
/// `Secure` only over HTTPS: a development server on plain `http://` would
/// otherwise have the cookie silently dropped, and the theme would appear not
/// to stick for reasons nothing reports.
#[cfg(feature = "hydrate")]
fn write_cookie(value: &str) {
    use wasm_bindgen::JsCast;

    let Some(html_document) = document().dyn_ref::<web_sys::HtmlDocument>().cloned() else {
        return;
    };

    let secure = window()
        .location()
        .protocol()
        .map(|protocol| protocol == "https:")
        .unwrap_or(false);

    let cookie = format!(
        "{COOKIE_NAME}={value}; Path=/; Max-Age={COOKIE_MAX_AGE_SECS}; SameSite=Lax{}",
        if secure { "; Secure" } else { "" }
    );

    let _ = html_document.set_cookie(&cookie);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_round_trip_through_the_cookie() {
        for mode in ThemeMode::ALL {
            for accent in Accent::ALL {
                for sidebar in SidebarState::ALL {
                    let preference = ThemePreference {
                        mode: *mode,
                        accent: *accent,
                        sidebar: *sidebar,
                    };
                    assert_eq!(ThemePreference::decode(&preference.encode()), preference);
                }
            }
        }
    }

    #[test]
    fn a_damaged_cookie_still_renders_the_app() {
        // Every one of these is something a real browser can hand back: an old
        // format, a truncated write, a value from a build that had a colour we
        // have since removed.
        for raw in [
            "",
            "dark",
            ":",
            "dark:",
            "chartreuse",
            "dark:chartreuse",
            "dark:teal:sideways",
            "dark:teal:rail:extra",
        ] {
            let decoded = ThemePreference::decode(raw);
            assert!(ThemeMode::ALL.contains(&decoded.mode), "{raw:?}");
            assert!(Accent::ALL.contains(&decoded.accent), "{raw:?}");
            assert!(SidebarState::ALL.contains(&decoded.sidebar), "{raw:?}");
        }

        // A cookie written before the sidebar field existed still decodes, and
        // keeps the two fields it does have.
        let older = ThemePreference::decode("dark:teal");
        assert_eq!(older.mode, ThemeMode::Dark);
        assert_eq!(older.accent, Accent::Teal);
        assert_eq!(older.sidebar, SidebarState::Expanded);

        // The recognisable half survives even when the rest does not.
        assert_eq!(
            ThemePreference::decode("dark:chartreuse").mode,
            ThemeMode::Dark
        );
        assert_eq!(ThemePreference::decode("wat:teal").accent, Accent::Teal);
    }

    #[test]
    fn system_writes_no_attribute() {
        // The whole light/dark fallback depends on this: an attribute of any
        // value wins over the media query, so "follow the OS" has to be absent.
        assert_eq!(ThemeMode::System.attribute(), None);
        assert_eq!(ThemeMode::Dark.attribute(), Some("dark"));
        assert_eq!(ThemeMode::Light.attribute(), Some("light"));
    }

    #[test]
    fn keys_are_unique_and_parse_back() {
        for (index, mode) in ThemeMode::ALL.iter().enumerate() {
            assert_eq!(ThemeMode::parse(mode.key()), Some(*mode));
            assert!(!ThemeMode::ALL[..index].contains(mode));
        }
        for (index, accent) in Accent::ALL.iter().enumerate() {
            assert_eq!(Accent::parse(accent.key()), Some(*accent));
            assert!(!Accent::ALL[..index].contains(accent));
        }
        for (index, sidebar) in SidebarState::ALL.iter().enumerate() {
            assert_eq!(SidebarState::parse(sidebar.key()), Some(*sidebar));
            assert!(!SidebarState::ALL[..index].contains(sidebar));
            assert_eq!(sidebar.toggled().toggled(), *sidebar);
        }
    }

    #[cfg(feature = "ssr")]
    #[test]
    fn cookies_are_found_among_others() {
        let header = "session_acme=abc; phonix_appearance=dark:teal:rail; other=1";

        assert_eq!(
            read_cookie(header, COOKIE_NAME).as_deref(),
            Some("dark:teal:rail")
        );
        assert_eq!(read_cookie(header, "missing"), None);

        // A value containing '=' must survive: base64 padding is the common case.
        assert_eq!(read_cookie("t=YWJj==", "t").as_deref(), Some("YWJj=="));
    }
}
