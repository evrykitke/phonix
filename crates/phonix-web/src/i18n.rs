//! Which language this page is in, and how a view asks for words.
//!
//! # `l!` is `msg!` already resolved
//!
//! ```ignore
//! <h1>{l!("auth.signin.title")}</h1>
//! <p>{l!("validation.password.too_short", min = 12)}</p>
//! ```
//!
//! The key is checked at compile time by [`phonix_core::msg`], which this wraps.
//! A view wants words now, so `l!` returns a `String`; a validator wants to
//! defer, so it returns a [`Message`] and the view resolves it later with
//! [`t`].
//!
//! # The server decides, and the browser is told
//!
//! Language resolution happens once, on the server, before a byte of HTML is
//! written. The choice is stamped on `<html lang>` and the catalog that produced
//! it is inlined into the document. Hydration then reads **both back off the
//! DOM** rather than working them out again.
//!
//! That is not laziness - it is the only arrangement that is safe. Leptos
//! compares the DOM the wasm builds against the DOM the server sent, and a
//! single differing word is a hydration mismatch. A mismatch in this
//! application is not a warning: wasm aborts rather than unwinds, so the page
//! freezes. Two independent resolutions can differ; reading the server's answer
//! cannot.
//!
//! # Why the whole catalog is inlined, and why that is cheaper than it looks
//!
//! Only the *overlay* is inlined - the deployment file's contents. The built-in
//! English is already inside the wasm bundle, so sending it again would be
//! sending the same bytes to a browser that has them. An English page therefore
//! inlines `{}`.
//!
//! A translated page inlines its whole catalog, tens of kilobytes before
//! compression. The alternative - fetching `/i18n/fr.<hash>.json` and letting
//! the browser cache it forever - is genuinely cheaper per page and is the
//! right next step, but it cannot serve the first frame: hydration would have
//! to wait for a network round trip, and until it landed the wasm would have
//! only English to render, which is the mismatch again. Inline first, cache the
//! second visit.
//!
//! # Changing language reloads the page
//!
//! The switcher writes a cookie and reloads rather than swapping strings in
//! place. The server has to re-render anyway - `<html lang>`, the `dir`
//! attribute, and every string resolved during SSR are all the old language -
//! and a reload makes the new language true everywhere at once instead of
//! true in the parts that happened to be reactive.
//!
//! It also means [`Locale`] holds a plain catalog rather than a signal, and no
//! view has to be reactive over its own wording.

use std::sync::Arc;

use leptos::prelude::*;
use phonix_core::Message;
use phonix_core::i18n::{Catalog, Language};

/// The cookie the switcher writes and the server reads.
pub const COOKIE_NAME: &str = "phonix_lang";

/// A year, like the appearance cookie. A language preference does not go stale.
pub const COOKIE_MAX_AGE_SECS: i64 = 60 * 60 * 24 * 365;

/// The id of the inlined catalog, on both sides.
pub const CATALOG_ELEMENT_ID: &str = "phonix-i18n";

/// The words in force for this render.
///
/// Cloning is an `Arc` bump. Not `Copy`, which is the one ergonomic cost of
/// holding the catalog by hand rather than in a `StoredValue` - and worth it,
/// because `StoredValue::new` wants a reactive owner and this type is
/// constructed in places that have none, including the fallback below.
#[derive(Clone)]
pub struct Locale {
    catalog: Arc<Catalog>,
    coverage: u8,
}

/// Where the shell records how much of the application this render's language
/// covers, so the browser can be told rather than work it out again.
pub const COVERAGE_ATTRIBUTE: &str = "data-i18n-coverage";

impl Locale {
    /// Seed the context from what the document was rendered with.
    pub fn provide(catalog: Arc<Catalog>) -> Self {
        let coverage = coverage_for(&catalog);
        let locale = Self { catalog, coverage };
        provide_context(locale.clone());
        locale
    }

    /// How much of the application this language covers, 0 to 100.
    ///
    /// Read off `<html>` in the browser rather than recomputed, because the
    /// number is a fraction of the built-in catalog and the two halves of the
    /// application carry separately compiled copies of it. Anything that
    /// decides whether an element exists has to come from one place.
    pub const fn coverage(&self) -> u8 {
        self.coverage
    }

    /// The locale for this tree.
    ///
    /// Falls back to built-in English rather than panicking when no shell
    /// provided one: a component rendered on its own in a test should draw
    /// English, not take the page down.
    pub fn get() -> Self {
        use_context::<Self>().unwrap_or_else(|| Self {
            catalog: Arc::new(Catalog::builtin(Language::ENGLISH)),
            // English is by definition complete, so the note the coverage
            // gates never appears on this path.
            coverage: 100,
        })
    }

    pub fn language(&self) -> Language {
        self.catalog.language()
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The catalog as something that can be moved into a closure.
    ///
    /// A grid's cell closures are called per row, from wherever the exporter
    /// happens to run, with no reactive owner to read the context from - so
    /// they hold the words rather than fetch them.
    pub fn shared(&self) -> Arc<Catalog> {
        Arc::clone(&self.catalog)
    }

    /// Resolve one message.
    pub fn render(&self, message: &Message) -> String {
        self.catalog.render(message)
    }

    /// The language in force, wherever this is called from.
    pub fn current() -> Language {
        Self::get().language()
    }
}

/// The catalog in force for this render, whichever half is running.
///
/// On the server it is resolved from the request. In the browser it is read
/// back off the document the server sent. The two cannot disagree, which is the
/// whole design - see the note at the top of this module.
pub fn current_catalog() -> Arc<Catalog> {
    #[cfg(feature = "ssr")]
    {
        catalog_for(language_from_request())
    }

    #[cfg(feature = "hydrate")]
    {
        let language = language_from_document();
        Arc::new(catalog_from_document(language))
    }

    #[cfg(not(any(feature = "ssr", feature = "hydrate")))]
    {
        Arc::new(Catalog::builtin(Language::ENGLISH))
    }
}

/// The coverage the shell recorded, or a fresh calculation when it did not.
///
/// The fallback is for a component rendered outside a shell - a test, mostly.
/// In a real document the attribute is always there.
fn coverage_for(catalog: &Catalog) -> u8 {
    #[cfg(feature = "hydrate")]
    {
        if let Some(stamped) = document()
            .document_element()
            .and_then(|root| root.get_attribute(COVERAGE_ATTRIBUTE))
            .and_then(|raw| raw.parse::<u8>().ok())
        {
            return stamped;
        }
    }

    catalog.coverage()
}

/// The overlay, as the shell inlines it.
///
/// `{}` for English, because the built-in catalog is already in the bundle.
/// Serialisation cannot fail for a map of strings, but if it somehow did, an
/// empty object renders the page in English rather than emitting broken JSON
/// that would take hydration down with it.
///
/// `<` is escaped as `\u003c`. A `application/json` script is not executed, but
/// the parser still ends the element at the first `</script>` - so a translated
/// string containing one would close the tag and spill the rest of the catalog
/// into the page as text. `\u003c` is valid JSON and decodes back to `<`, so
/// nothing downstream notices.
pub fn overlay_json(catalog: &Catalog) -> String {
    serde_json::to_string(catalog.overlay())
        .unwrap_or_else(|_| "{}".to_owned())
        .replace('<', "\\u003c")
}

/// Resolve a [`Message`] against the catalog in context.
///
/// For messages that arrive already built - a rejection from a server function,
/// anything a validator produced. Use `l!` for a key written here.
pub fn t(message: &Message) -> String {
    Locale::get().render(message)
}

/// Resolve a message, or nothing.
///
/// The shape most call sites want, because a field's error is an `Option`.
pub fn t_opt(message: Option<&Message>) -> Option<String> {
    message.map(t)
}

/// A translated string, resolved now.
///
/// ```ignore
/// l!("common.save")
/// l!("validation.password.too_short", min = 12)
/// ```
///
/// The key is a literal and is checked against the built-in catalog at compile
/// time - see [`phonix_core::msg`].
#[macro_export]
macro_rules! l {
    ($($tokens:tt)*) => {
        $crate::i18n::t(&::phonix_core::msg!($($tokens)*))
    };
}

/// A translated string with a singular and a plural form.
///
/// ```ignore
/// lp!("auth.locked", minutes)
/// ```
#[macro_export]
macro_rules! lp {
    ($($tokens:tt)*) => {
        $crate::i18n::t(&::phonix_core::pmsg!($($tokens)*))
    };
}

// ---------------------------------------------------------------------------
// Resolving the language for a request (server)
// ---------------------------------------------------------------------------

/// Which language this request is in.
///
/// Order, each beating the one before:
///
/// 1. the built-in default, English
/// 2. `Accept-Language`, if it names one we offer
/// 3. the cookie the switcher wrote
///
/// The cookie is last because it is the only one that is an actual decision.
/// `Accept-Language` sits above the default and below the cookie: it is a good
/// guess for somebody who has never touched the switcher, and no guess at all
/// once they have.
///
/// A workspace-wide default belongs between the header and the cookie, and is
/// deliberately not built yet - it needs a column on `workspace_settings` and
/// a tenant lookup on a path that currently touches no database.
#[cfg(feature = "ssr")]
pub fn language_from_request() -> Language {
    let Some(parts) = use_context::<http::request::Parts>() else {
        return Language::ENGLISH;
    };

    let header = |name: http::HeaderName| {
        parts
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };

    let from_cookie = header(http::header::COOKIE)
        .and_then(|cookies| crate::theme::read_cookie(&cookies, COOKIE_NAME))
        .and_then(|value| Language::parse(&value));

    if let Some(language) = from_cookie {
        return language;
    }

    header(http::header::ACCEPT_LANGUAGE)
        .and_then(|accepts| Language::negotiate(&accepts))
        .unwrap_or(Language::ENGLISH)
}

// ---------------------------------------------------------------------------
// Reading the server's answer back (browser)
// ---------------------------------------------------------------------------

/// The language the server rendered this document in.
///
/// Off `<html lang>`, which the server already had to set for screen readers
/// and for `dir`. Reading it back is free and cannot disagree with what was
/// rendered - which is exactly the property hydration needs.
#[cfg(feature = "hydrate")]
pub fn language_from_document() -> Language {
    document()
        .document_element()
        .and_then(|root| root.get_attribute("lang"))
        .and_then(|code| Language::negotiate_tag(&code))
        .unwrap_or(Language::ENGLISH)
}

/// The overlay the server inlined, if there was one.
#[cfg(feature = "hydrate")]
pub fn catalog_from_document(language: Language) -> Catalog {
    let overlay = document()
        .get_element_by_id(CATALOG_ELEMENT_ID)
        .and_then(|element| element.text_content())
        .filter(|raw| !raw.trim().is_empty())
        // A damaged blob renders the application in English rather than not at
        // all. There is no version of this worth a frozen tab.
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();

    Catalog::with_overlay(language, overlay)
}

/// Persist the choice, so the next server render agrees with it.
///
/// `Secure` only over HTTPS, for the reason the appearance cookie gives: a
/// development server on plain `http://` would have it silently dropped, and
/// the language would appear not to stick with nothing reporting why.
#[cfg(feature = "hydrate")]
pub fn write_cookie(language: Language) {
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
        "{COOKIE_NAME}={}; Path=/; Max-Age={COOKIE_MAX_AGE_SECS}; SameSite=Lax{}",
        language.code(),
        if secure { "; Secure" } else { "" }
    );

    let _ = html_document.set_cookie(&cookie);
}

// ---------------------------------------------------------------------------
// The catalogs a deployment carries (server)
// ---------------------------------------------------------------------------

#[cfg(feature = "ssr")]
mod loaded {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::{Arc, OnceLock};

    use phonix_core::i18n::{Catalog, Language};

    static CATALOGS: OnceLock<Catalogs> = OnceLock::new();

    /// Every language's words, read once at boot.
    ///
    /// Immutable for the life of the process. Translations change on a deploy,
    /// not on a request, so re-reading the files per request would be a syscall
    /// answering a question whose answer cannot have changed - and a catalog
    /// that reloads under a running render could hand two halves of one page
    /// two different versions.
    pub struct Catalogs {
        by_code: BTreeMap<&'static str, Arc<Catalog>>,
    }

    impl Catalogs {
        /// Read `<dir>/<code>.json` for every language on offer.
        ///
        /// A missing or malformed file is a warning, not a failure. An
        /// operator mistyping one line of French must not stop the server from
        /// starting; the language simply falls through to English, which is
        /// what a partial catalog does anyway. The catalogs *in this
        /// repository* are held to a stricter standard than that - see
        /// `every_offered_language_is_a_finished_language` - but a deployment's
        /// own edits are not, and must not be able to take the site down.
        pub fn load(dir: &Path) -> Self {
            let mut by_code = BTreeMap::new();

            for language in Language::ALL {
                if *language == Language::ENGLISH {
                    // Compiled in. A file could only restate it, and would
                    // silently win over the source of truth if it drifted.
                    continue;
                }

                let path = dir.join(format!("{}.json", language.code()));

                let overlay: BTreeMap<String, String> = match std::fs::read_to_string(&path) {
                    Ok(raw) => match serde_json::from_str(&raw) {
                        Ok(parsed) => parsed,
                        Err(err) => {
                            tracing::warn!(
                                path = %path.display(),
                                %err,
                                "translation file is not a flat map of key to string; \
                                 falling back to English",
                            );
                            continue;
                        }
                    },
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(err) => {
                        tracing::warn!(path = %path.display(), %err, "cannot read translations");
                        continue;
                    }
                };

                // `_comment` and friends document the file for whoever opens
                // it next; `build.rs` drops them from the built-in catalog for
                // the same reason. Left in, they would be inlined into every
                // page as a key nothing can ever look up.
                let overlay = overlay
                    .into_iter()
                    .filter(|(key, _)| !key.starts_with('_'))
                    .collect();

                let catalog = Catalog::with_overlay(*language, overlay);

                tracing::info!(
                    language = language.code(),
                    coverage = catalog.coverage(),
                    keys = catalog.overlay().len(),
                    "loaded translations",
                );

                by_code.insert(language.code(), Arc::new(catalog));
            }

            Self { by_code }
        }

        /// The words for one language, English if nothing was loaded for it.
        pub fn get(&self, language: Language) -> Arc<Catalog> {
            self.by_code
                .get(language.code())
                .map(Arc::clone)
                .unwrap_or_else(|| Arc::new(Catalog::builtin(language)))
        }
    }

    /// Install the catalogs for the process. Called once, at boot.
    ///
    /// Ignores a second call rather than replacing: two sets of words live in a
    /// process only by mistake, and the second would leave pages already being
    /// rendered reading from the first.
    pub fn install(dir: &Path) {
        let _ = CATALOGS.set(Catalogs::load(dir));
    }

    /// The words for one language.
    ///
    /// Built-in English when nothing was installed - which is the case in every
    /// test, and in any build that never calls [`install`].
    pub fn catalog_for(language: Language) -> Arc<Catalog> {
        match CATALOGS.get() {
            Some(catalogs) => catalogs.get(language),
            None => Arc::new(Catalog::builtin(language)),
        }
    }
}

#[cfg(feature = "ssr")]
pub use loaded::{Catalogs, catalog_for, install};

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn a_locale_with_no_context_still_speaks_english() {
        // A component rendered on its own in a test has no shell above it.
        let locale = Locale::get();

        assert_eq!(locale.language(), Language::ENGLISH);
        assert_eq!(
            locale.render(&phonix_core::msg!("validation.password.needs_digit")),
            "Include at least one number."
        );
    }

    #[test]
    fn the_cookie_name_is_not_the_appearance_cookie() {
        // They are read by the same parser off the same header; one name for
        // both would decode a theme as a language.
        assert_ne!(COOKIE_NAME, crate::theme::COOKIE_NAME);
    }

    #[test]
    fn the_inlined_catalog_cannot_close_its_own_script_tag() {
        use std::collections::BTreeMap;

        let mut overlay = BTreeMap::new();
        overlay.insert(
            "common.save".to_owned(),
            "Save </script><script>alert(1)</script>".to_owned(),
        );

        let json = overlay_json(&Catalog::with_overlay(Language::ENGLISH, overlay));

        // Nothing a parser will read as the end of the element.
        assert!(!json.contains("</script>"));
        assert!(!json.contains('<'));

        // And it is still the same string once parsed, so the escape is
        // invisible to everything downstream.
        let parsed: BTreeMap<String, String> = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.get("common.save").map(String::as_str),
            Some("Save </script><script>alert(1)</script>")
        );
    }

    /// Every translation file in the repository, as the loader would read it.
    fn deployment_catalogs() -> Vec<(String, std::collections::BTreeMap<String, String>)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../locales");

        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };

        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            .filter_map(|entry| {
                let raw = std::fs::read_to_string(entry.path()).ok()?;
                let parsed: std::collections::BTreeMap<String, String> = serde_json::from_str(&raw)
                    .unwrap_or_else(|err| panic!("{:?} is not valid: {err}", entry.path()));
                Some((entry.file_name().to_string_lossy().into_owned(), parsed))
            })
            .collect()
    }

    #[test]
    fn every_offered_language_is_a_finished_language() {
        // `Language::ALL` is the switcher. Putting a language on it is a
        // promise that choosing it changes the page, and a catalog that covers
        // a third of the keys turns that promise into a half-English screen
        // that reads worse than English alone would have.
        //
        // The per-key fallback still exists and still runs - it is what keeps a
        // deployment's own edited file from blanking the interface. This is
        // about what *we* ship: a language arrives translated, and every key
        // added to the English catalog is added to the others in the same
        // commit. This test is where you find out that you forgot.
        let catalogs: std::collections::BTreeMap<_, _> =
            deployment_catalogs().into_iter().collect();

        for language in phonix_core::Language::ALL {
            if *language == phonix_core::Language::ENGLISH {
                // English is compiled in; there is nothing to fall back to and
                // nothing to translate.
                continue;
            }

            let file = format!("{}.json", language.code());
            let overlay = catalogs.get(&file).unwrap_or_else(|| {
                panic!("{language} is on the switcher but locales/{file} is not there")
            });

            let total = phonix_core::i18n::catalog::builtin_keys().count();
            let missing: Vec<&str> = phonix_core::i18n::catalog::builtin_keys()
                .filter(|key| !overlay.contains_key(*key))
                .collect();

            assert!(
                missing.is_empty(),
                "locales/{file} is missing {} of {total} keys, starting with {:?}",
                missing.len(),
                missing.iter().take(5).collect::<Vec<_>>(),
            );
        }
    }

    #[test]
    fn no_translation_names_a_key_the_application_does_not_have() {
        // A key that no longer exists is dead weight inlined into every page,
        // and - far worse - it is usually a *typo* for one that does, which
        // shows up as an untranslated sentence nobody can explain.
        for (file, overlay) in deployment_catalogs() {
            for key in overlay.keys() {
                if key.starts_with('_') {
                    continue;
                }

                assert!(
                    phonix_core::i18n::catalog::builtin_contains(key),
                    "{file} defines {key}, which i18n/en.json does not",
                );
            }
        }
    }

    #[test]
    fn no_translation_invents_a_blank_the_sentence_cannot_fill() {
        // `{minimum}` where the English says `{min}` renders the placeholder
        // verbatim on screen. The catalog is right to leave it standing rather
        // than emptying it, but nobody should find out that way.
        for (file, overlay) in deployment_catalogs() {
            for (key, translated) in &overlay {
                if key.starts_with('_') {
                    continue;
                }

                let Some(english) = phonix_core::i18n::catalog::builtin_lookup(key) else {
                    continue;
                };

                for blank in blanks(translated) {
                    assert!(
                        blanks(english).contains(&blank),
                        "{file}: {key} fills {{{blank}}}, which the English does not supply",
                    );
                }
            }
        }
    }

    /// The `{name}` placeholders in a sentence.
    fn blanks(template: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut rest = template;

        while let Some(open) = rest.find('{') {
            let Some(after) = rest.get(open + 1..) else {
                break;
            };
            let Some(close) = after.find('}') else {
                break;
            };
            let Some(name) = after.get(..close) else {
                break;
            };

            if !name.is_empty() {
                found.push(name.to_owned());
            }

            rest = after.get(close + 1..).unwrap_or("");
        }

        found
    }
}
