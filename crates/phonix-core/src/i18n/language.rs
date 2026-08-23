//! The languages the application offers, and how one gets chosen.
//!
//! A closed list, in the same spirit as [`Country`](crate::locale::Country):
//! a language stored as free text is a language nothing can group by, and a
//! `<html lang>` nothing can validate. The code is what is stored, and it is
//! what names the deployment file - `fr` is `locales/fr.json`.
//!
//! Adding a language is one line here and one file beside it. Nothing else in
//! the application enumerates languages.

use core::fmt;

use serde::{Deserialize, Serialize};

/// Which way the script runs.
///
/// Carried from the start even though every language listed today is
/// left-to-right. `dir` has to be on `<html>` in the bytes the server sends -
/// the same argument the theme makes for `data-theme` - and retrofitting it
/// once components have hard-coded `ml-`/`mr-` everywhere is a far larger job
/// than declaring it now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Direction {
    Ltr,
    Rtl,
}

impl Direction {
    /// The `dir` attribute value.
    pub const fn attribute(self) -> &'static str {
        match self {
            Self::Ltr => "ltr",
            Self::Rtl => "rtl",
        }
    }
}

/// One language the switcher can offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Language {
    code: &'static str,
    english_name: &'static str,
    native_name: &'static str,
    direction: Direction,
}

impl Language {
    /// The language everything falls back to, and the one compiled into the
    /// binary. It cannot be missing, so it is the only one that is a `const`
    /// rather than a lookup.
    pub const ENGLISH: Self = Self {
        code: "en",
        english_name: "English",
        native_name: "English",
        direction: Direction::Ltr,
    };

    /// Every language on offer.
    ///
    /// English first because it is the default; the rest alphabetically by
    /// their own name, which is the order a person scanning the switcher for
    /// their language expects - somebody looking for German is looking for
    /// "Deutsch", and will find it under D.
    ///
    /// Every entry here is a *finished* language: `locales/<code>.json` carries
    /// every key `i18n/en.json` does, and a test refuses the build if it does
    /// not. Adding a language therefore means translating the catalog, not
    /// starting one - the per-key fallback still exists, but it is a runtime
    /// safety net for a deployment's own overrides rather than a licence to
    /// ship half a language.
    pub const ALL: &'static [Self] = &[
        Self::ENGLISH,
        Self {
            code: "de",
            english_name: "German",
            native_name: "Deutsch",
            direction: Direction::Ltr,
        },
        Self {
            code: "fr",
            english_name: "French",
            native_name: "Français",
            direction: Direction::Ltr,
        },
    ];

    /// The BCP-47 code: what is stored, what names the file, what goes in
    /// `<html lang>`.
    pub const fn code(self) -> &'static str {
        self.code
    }

    /// The name in English, for an administrator's list.
    pub const fn english_name(self) -> &'static str {
        self.english_name
    }

    /// The name in the language itself, for the switcher.
    ///
    /// A switcher labelled in the language you already read is no help to the
    /// person who needs it: somebody stuck in a language they do not speak is
    /// looking for the word they *do* recognise.
    pub const fn native_name(self) -> &'static str {
        self.native_name
    }

    pub const fn direction(self) -> Direction {
        self.direction
    }

    pub const fn is_default(self) -> bool {
        matches!(self.direction, Direction::Ltr) && str_eq(self.code, "en")
    }

    /// Look up an exact code, case-insensitively.
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        Self::ALL
            .iter()
            .copied()
            .find(|language| language.code.eq_ignore_ascii_case(raw))
    }

    /// Resolve a tag that may carry a region or a script: `en-GB`, `fr-CA`,
    /// `zh-Hant-TW`.
    ///
    /// Falls back to the primary subtag, so a browser asking for `en-GB` gets
    /// English rather than nothing. Regional variants are deliberately not
    /// separate entries - the difference between `en-GB` and `en-US` in an
    /// admin application is a handful of words, and carrying two near-identical
    /// catalogs to capture them costs more than it returns.
    pub fn negotiate_tag(tag: &str) -> Option<Self> {
        let tag = tag.trim();

        Self::parse(tag).or_else(|| {
            let (primary, _) = tag.split_once('-')?;
            Self::parse(primary)
        })
    }

    /// Pick a language from an `Accept-Language` header.
    ///
    /// Honours the quality values, because they are how a browser expresses
    /// "French, but English will do" and ignoring them picks the wrong one for
    /// exactly the people who set the header deliberately.
    ///
    /// `None` when nothing on offer matches, which is an ordinary answer: the
    /// caller then uses the deployment default rather than the least-bad guess.
    pub fn negotiate(header: &str) -> Option<Self> {
        let mut best: Option<(f32, Self)> = None;

        for entry in header.split(',') {
            let mut parts = entry.split(';');

            let Some(tag) = parts.next().map(str::trim).filter(|tag| !tag.is_empty()) else {
                continue;
            };

            // `*` means "anything", which is a preference for nothing in
            // particular and must not beat a named language further down.
            if tag == "*" {
                continue;
            }

            // Absent `q` means 1.0 per RFC 9110; an unparseable one is treated
            // as absent rather than as zero, because a malformed header should
            // still express the language it names.
            let quality = parts
                .find_map(|part| part.trim().strip_prefix("q=")?.trim().parse::<f32>().ok())
                .unwrap_or(1.0);

            let Some(language) = Self::negotiate_tag(tag) else {
                continue;
            };

            // Strictly greater, so that ties keep the earlier entry - which is
            // the order the browser listed them in, and therefore the order the
            // person actually prefers.
            if best.is_none_or(|(best_quality, _)| quality > best_quality) {
                best = Some((quality, language));
            }
        }

        best.map(|(_, language)| language)
    }
}

impl Default for Language {
    fn default() -> Self {
        Self::ENGLISH
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}

impl AsRef<str> for Language {
    fn as_ref(&self) -> &str {
        self.code
    }
}

/// Serialised as the bare code, so the column, the cookie, the JSON on the wire
/// and the value in the browser are the same two characters.
impl Serialize for Language {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.code)
    }
}

/// Deserialises leniently: an unknown code becomes English rather than an
/// error.
///
/// This value arrives from a cookie the browser hands back and from rows
/// written by older builds. A language that has since been withdrawn must
/// render the application in English, not fail to render it.
impl<'de> Deserialize<'de> for Language {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::negotiate_tag(&raw).unwrap_or_default())
    }
}

/// `str` equality in a const context, which `==` is not.
const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());

    if a.len() != b.len() {
        return false;
    }

    let mut index = 0;
    // Indexing rather than iterating: `Iterator` is not available in a const
    // context, and both reads are bounded by the length checked above.
    #[allow(clippy::indexing_slicing)]
    while index < a.len() {
        if a[index] != b[index] {
            return false;
        }
        index += 1;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_parse_back() {
        for (index, language) in Language::ALL.iter().enumerate() {
            assert_eq!(Language::parse(language.code()), Some(*language));
            assert!(
                !Language::ALL[..index].contains(language),
                "{} is listed twice",
                language.code()
            );
        }
    }

    #[test]
    fn english_is_the_default_and_is_on_the_list() {
        assert_eq!(Language::default(), Language::ENGLISH);
        assert!(Language::ALL.contains(&Language::ENGLISH));
        assert!(Language::ENGLISH.is_default());
    }

    #[test]
    fn regional_tags_fall_back_to_their_language() {
        for tag in ["en-GB", "en-US", "en-us", "EN-Latn-GB"] {
            assert_eq!(
                Language::negotiate_tag(tag),
                Some(Language::ENGLISH),
                "{tag}"
            );
        }

        assert_eq!(
            Language::negotiate_tag("fr-CA").map(Language::code),
            Some("fr")
        );
        assert_eq!(
            Language::negotiate_tag("de-AT").map(Language::code),
            Some("de")
        );
        assert_eq!(Language::negotiate_tag("ja-JP"), None);
    }

    #[test]
    fn quality_values_decide_which_language_wins() {
        // The browser prefers French even though English is listed first.
        assert_eq!(
            Language::negotiate("en;q=0.5,fr;q=0.9").map(Language::code),
            Some("fr")
        );

        // An absent q is 1.0, so the first entry wins over an explicit 0.9.
        assert_eq!(
            Language::negotiate("en,fr;q=0.9").map(Language::code),
            Some("en")
        );

        // Languages nobody offers are skipped, not guessed at.
        assert_eq!(
            Language::negotiate("ja,pt;q=0.9,de;q=0.1").map(Language::code),
            Some("de")
        );
        assert_eq!(Language::negotiate("ja,pt"), None);
    }

    #[test]
    fn a_damaged_header_still_yields_a_language() {
        // Every one of these is something a real client can send.
        for header in [
            "",
            ",",
            ";q=0.9",
            "*",
            "en;q=",
            "en;q=banana",
            "  en-GB  ,  fr ; q = 0.8 ",
        ] {
            // The only requirement is that it does not panic and does not
            // invent a language that is not on offer.
            if let Some(language) = Language::negotiate(header) {
                assert!(Language::ALL.contains(&language), "{header:?}");
            }
        }

        // A malformed q is treated as absent rather than as zero, so the
        // language it names still counts.
        assert_eq!(
            Language::negotiate("en;q=banana").map(Language::code),
            Some("en")
        );

        // `*` alone expresses no preference at all.
        assert_eq!(Language::negotiate("*"), None);
    }

    #[test]
    fn unknown_codes_deserialise_to_english() {
        // A cookie written by a build that offered a language this one does not.
        let language: Language = serde_json::from_str("\"ja\"").unwrap();
        assert_eq!(language, Language::ENGLISH);

        let language: Language = serde_json::from_str("\"fr\"").unwrap();
        assert_eq!(language.code(), "fr");
    }

    #[test]
    fn languages_round_trip_through_serde() {
        for language in Language::ALL {
            let json = serde_json::to_string(language).unwrap();
            assert_eq!(json, format!("\"{}\"", language.code()));
            assert_eq!(serde_json::from_str::<Language>(&json).unwrap(), *language);
        }
    }

    #[test]
    fn every_language_names_itself() {
        for language in Language::ALL {
            assert!(!language.native_name().is_empty(), "{}", language.code());
            assert!(!language.english_name().is_empty(), "{}", language.code());
            // Two letters, or two plus a region. Anything else is not a tag
            // `<html lang>` will accept.
            assert!(
                language.code().len() >= 2 && language.code().is_ascii(),
                "{} is not a usable BCP-47 tag",
                language.code()
            );
        }
    }
}
