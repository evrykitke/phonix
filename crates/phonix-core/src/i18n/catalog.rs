//! Where a key turns into a sentence.
//!
//! # Two layers, and why not three
//!
//! ```text
//! overlay   locales/<code>.json, read once at boot   may be missing, may be partial
//! built-in  i18n/en.json, compiled in                cannot be missing
//! ```
//!
//! A lookup tries the overlay, then the built-in, then gives up and returns the
//! key. Falling through per *key* rather than per *file* is what makes a
//! half-finished translation usable: a French catalog with sixty of a thousand
//! keys renders sixty sentences in French and the rest in English, which is a
//! product you can ship on the day the first sixty come back from the
//! translator.
//!
//! A third layer - per-tenant overrides in the database - slots in above the
//! overlay without changing anything here. It is deliberately not built yet.
//!
//! # This never fails
//!
//! There is no `Result` in this module. A missing key returns the key, a
//! malformed placeholder is left alone, an unknown argument is ignored. That is
//! not laziness about errors: this code runs inside the wasm bundle, where a
//! panic aborts and freezes the entire tab, and no sentence is worth that. The
//! worst outcome available is a screen with a dotted key on it, which is ugly,
//! searchable, and still a working application.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use super::language::Language;
use super::message::Message;

// The sorted `&[(&str, &str)]` generated from `i18n/en.json`.
include!(concat!(env!("OUT_DIR"), "/builtin.rs"));

/// Does the built-in catalog define this key?
///
/// A `const fn` so that [`msg!`](crate::msg) can ask at compile time. Binary
/// search rather than a scan because it is evaluated once per call site and
/// there will eventually be thousands of those.
#[must_use]
pub const fn builtin_contains(key: &str) -> bool {
    let mut low = 0;
    let mut high = BUILTIN.len();

    // Indexing rather than `get`: neither `Option::map` nor slice `get` is
    // available in a const context, and `mid` is strictly below `high`, which
    // never exceeds the length.
    #[allow(clippy::indexing_slicing)]
    while low < high {
        let mid = low + (high - low) / 2;

        match str_cmp(BUILTIN[mid].0, key) {
            -1 => low = mid + 1,
            1 => high = mid,
            _ => return true,
        }
    }

    false
}

/// The built-in English for a key, if there is one.
#[must_use]
pub fn builtin_lookup(key: &str) -> Option<&'static str> {
    BUILTIN
        .binary_search_by(|(candidate, _)| (*candidate).cmp(key))
        .ok()
        .and_then(|index| BUILTIN.get(index))
        .map(|(_, text)| *text)
}

/// Every key the application defines, sorted.
///
/// The list a translator is handed, and what a completeness check counts
/// against.
pub fn builtin_keys() -> impl Iterator<Item = &'static str> {
    BUILTIN.iter().map(|(key, _)| *key)
}

/// One language's worth of sentences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    language: Language,
    overlay: BTreeMap<String, String>,
}

impl Catalog {
    /// A catalog with nothing but the built-in English behind it.
    #[must_use]
    pub fn builtin(language: Language) -> Self {
        Self {
            language,
            overlay: BTreeMap::new(),
        }
    }

    /// A catalog with a deployment file layered over the built-in English.
    ///
    /// Keys the overlay does not define fall through, so a partial file is a
    /// partial translation rather than a broken one.
    #[must_use]
    pub fn with_overlay(language: Language, overlay: BTreeMap<String, String>) -> Self {
        Self { language, overlay }
    }

    #[must_use]
    pub const fn language(&self) -> Language {
        self.language
    }

    /// What the overlay actually carries.
    ///
    /// This is what the server inlines into the HTML for the browser, and it is
    /// deliberately only the overlay: the built-in English is already inside
    /// the wasm bundle, so sending it again would be sending the same kilobytes
    /// twice to a browser that has them.
    #[must_use]
    pub const fn overlay(&self) -> &BTreeMap<String, String> {
        &self.overlay
    }

    /// How much of the application this catalog actually covers, 0 to 100.
    ///
    /// Shown beside a language in the switcher when it is not finished, because
    /// "why is half of this in English?" is otherwise a support ticket.
    #[must_use]
    pub fn coverage(&self) -> u8 {
        let total = BUILTIN.len();

        if total == 0 || self.language == Language::ENGLISH {
            return 100;
        }

        let translated = builtin_keys()
            .filter(|key| self.overlay.contains_key(*key))
            .count();

        // Saturating rather than exact: a catalog carrying keys this build no
        // longer defines must not report more than complete.
        u8::try_from(translated.saturating_mul(100) / total).unwrap_or(100)
    }

    /// The sentence for a key, in this language, or `None` if nothing defines
    /// it anywhere.
    #[must_use]
    pub fn lookup(&self, key: &str) -> Option<&str> {
        self.overlay
            .get(key)
            .map(String::as_str)
            .or_else(|| builtin_lookup(key))
    }

    /// Resolve a message: pick the plural form, find the sentence, fill the
    /// blanks.
    #[must_use]
    pub fn render(&self, message: &Message) -> String {
        // Text that was never keyed. Nothing to look up and nothing to fill.
        if message.literal {
            return message.key.clone();
        }

        let key = match message.count {
            // A translation may define the plural forms while the built-in
            // English does not, or the other way round, so the *resolved* key
            // is chosen first and looked up as a whole.
            Some(count) => {
                let form = if count == 1 { "one" } else { "other" };
                let plural_key = format!("{}.{form}", message.key);

                if self.lookup(&plural_key).is_some() {
                    plural_key
                } else {
                    message.key.clone()
                }
            }
            None => message.key.clone(),
        };

        // The key itself when nothing defines it: searchable, and unmistakably
        // a bug, which a blank is not.
        let Some(template) = self.lookup(&key) else {
            return key;
        };

        fill(template, message)
    }
}

/// English with no overlay - the last line of defence.
///
/// A `OnceLock` because it is immutable and shared: every `Display` on a
/// `Message` reaches for it, including from log lines on hot paths.
#[must_use]
pub fn builtin_only() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| Catalog::builtin(Language::ENGLISH))
}

/// Substitute `{name}` for each argument's value.
///
/// Written by hand rather than with a format crate because the rules have to be
/// forgiving in specific ways: an unmatched `{` is text, `{}` is text, and a
/// placeholder nobody supplied a value for is left standing rather than
/// silently emptied. A translator who mistypes a blank should see the blank on
/// screen, not a sentence with a hole in it.
fn fill(template: &str, message: &Message) -> String {
    // The overwhelmingly common case: no blanks to fill, so no allocation
    // beyond the copy the caller needs anyway.
    if message.args.is_empty() || !template.contains('{') {
        return template.to_owned();
    }

    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;

    while let Some(open) = rest.find('{') {
        let (before, from_brace) = rest.split_at(open);
        out.push_str(before);

        // Everything after the brace, or the brace itself if it is the last
        // character in the template.
        let Some(after_brace) = from_brace.get(1..) else {
            out.push('{');
            return out;
        };

        let Some(close) = after_brace.find('}') else {
            // No closing brace anywhere: the rest is literal text.
            out.push_str(from_brace);
            return out;
        };

        let (name, tail) = after_brace.split_at(close);

        match message.args.iter().find(|arg| arg.name == name) {
            Some(arg) => out.push_str(&arg.value),
            // Nobody supplied it. Put the placeholder back verbatim so the
            // sentence still reads as a sentence with an obvious gap.
            None => {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }

        // Past the closing brace.
        rest = tail.get(1..).unwrap_or("");
    }

    out.push_str(rest);
    out
}

/// Three-way `str` comparison in a const context, as -1, 0 or 1.
///
/// `Ord` is not const, and `Ordering` cannot be matched through a const trait
/// call, so the comparison is spelled out. Byte-wise, which matches the
/// `BTreeMap` ordering `build.rs` sorted the catalog with.
const fn str_cmp(a: &str, b: &str) -> i8 {
    let (a, b) = (a.as_bytes(), b.as_bytes());

    let shorter = if a.len() < b.len() { a.len() } else { b.len() };
    let mut index = 0;

    // Bounded by the shorter of the two lengths on every read.
    #[allow(clippy::indexing_slicing)]
    while index < shorter {
        if a[index] < b[index] {
            return -1;
        }
        if a[index] > b[index] {
            return 1;
        }
        index += 1;
    }

    if a.len() < b.len() {
        -1
    } else if a.len() > b.len() {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msg;

    #[test]
    fn the_built_in_catalog_is_sorted_and_unique() {
        // Both the const check and the runtime lookup binary-search it, and a
        // binary search over an unsorted list does not fail loudly - it just
        // fails to find things.
        for pair in BUILTIN.windows(2) {
            let [(before, _), (after, _)] = pair else {
                continue;
            };
            assert!(before < after, "{before} and {after} are out of order");
        }
    }

    #[test]
    fn no_sentence_is_empty() {
        for (key, text) in BUILTIN {
            assert!(!text.trim().is_empty(), "{key} has no sentence");
        }
    }

    #[test]
    fn the_const_check_and_the_runtime_lookup_agree() {
        for key in builtin_keys() {
            assert!(builtin_contains(key), "{key}");
            assert!(builtin_lookup(key).is_some(), "{key}");
        }

        assert!(!builtin_contains("nothing.defines.this"));
        assert_eq!(builtin_lookup("nothing.defines.this"), None);

        // Boundaries: the first and last keys, and things either side of them.
        assert!(!builtin_contains(""));
        assert!(!builtin_contains("zzz"));
    }

    #[test]
    fn an_overlay_wins_and_the_rest_falls_through() {
        let mut overlay = BTreeMap::new();
        overlay.insert("common.save".to_owned(), "Hifadhi".to_owned());

        let catalog = Catalog::with_overlay(Language::ENGLISH, overlay);

        assert_eq!(catalog.lookup("common.save"), Some("Hifadhi"));
        assert_eq!(catalog.lookup("common.cancel"), Some("Cancel"));
        assert_eq!(catalog.lookup("nothing.defines.this"), None);
    }

    #[test]
    fn a_missing_key_renders_as_itself() {
        let catalog = Catalog::builtin(Language::ENGLISH);
        assert_eq!(
            catalog.render(&Message::new("nothing.defines.this")),
            "nothing.defines.this"
        );
    }

    #[test]
    fn a_translation_may_define_plurals_the_english_does_not() {
        let mut overlay = BTreeMap::new();
        overlay.insert(
            "auth.locked.one".to_owned(),
            "Subiri dakika {count}.".to_owned(),
        );
        overlay.insert(
            "auth.locked.other".to_owned(),
            "Subiri dakika {count}.".to_owned(),
        );

        let catalog = Catalog::with_overlay(Language::ENGLISH, overlay);
        let message = Message::new("auth.locked").count(3);

        assert_eq!(catalog.render(&message), "Subiri dakika 3.");
    }

    #[test]
    fn a_damaged_template_never_panics_and_never_swallows_text() {
        let message = msg!("common.save", name = "Ada");

        for (template, expected) in [
            ("plain", "plain"),
            ("hello {name}", "hello Ada"),
            ("{name}{name}", "AdaAda"),
            // An argument nobody asked for is ignored.
            ("no blanks here", "no blanks here"),
            // A blank nobody supplied stays visible.
            ("hello {missing}", "hello {missing}"),
            // Unbalanced braces are text.
            ("hello {", "hello {"),
            ("hello {name", "hello {name"),
            ("}{", "}{"),
            ("{}", "{}"),
            ("{ name }", "{ name }"),
        ] {
            assert_eq!(fill(template, &message), expected, "for {template:?}");
        }
    }

    #[test]
    fn coverage_counts_what_the_overlay_actually_carries() {
        // English is complete by definition - it is the list.
        assert_eq!(Catalog::builtin(Language::ENGLISH).coverage(), 100);

        let Some(other) = Language::ALL.iter().find(|l| **l != Language::ENGLISH) else {
            return;
        };

        assert_eq!(Catalog::builtin(*other).coverage(), 0);

        let mut overlay = BTreeMap::new();
        for key in builtin_keys() {
            overlay.insert(key.to_owned(), "translated".to_owned());
        }
        assert_eq!(Catalog::with_overlay(*other, overlay).coverage(), 100);

        // Keys this build no longer defines cannot push it over 100.
        let mut stale = BTreeMap::new();
        stale.insert("gone.in.a.later.build".to_owned(), "x".to_owned());
        assert_eq!(Catalog::with_overlay(*other, stale).coverage(), 0);
    }

    #[test]
    fn str_cmp_matches_the_standard_ordering() {
        for a in ["", "a", "ab", "b", "common.save", "common.saving"] {
            for b in ["", "a", "ab", "b", "common.save", "common.saving"] {
                let expected = match a.as_bytes().cmp(b.as_bytes()) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                };
                assert_eq!(str_cmp(a, b), expected, "{a:?} vs {b:?}");
            }
        }
    }
}
