//! A thing to say, before anybody has decided what language to say it in.
//!
//! # Why the back end returns this and not a sentence
//!
//! A validator that returns `"Include at least one number."` has made the
//! language decision in `phonix-services`, on the server, in a place that knows
//! nothing about who is reading. By the time that string reaches the browser it
//! is finished English, and no amount of translation in the view layer can
//! undo it.
//!
//! So a validator returns a [`Message`]: the key of the sentence and the values
//! that go in its blanks. It crosses the wire as data, and the view resolves it
//! against the reader's catalog at the moment it is drawn.
//!
//! # Rendering without a catalog
//!
//! [`Message`] implements `Display`, resolving against the built-in English.
//! That is what makes `#[error("{field}: {message}")]` still produce a readable
//! log line, and what a test asserts against. It is a convenience for the
//! server's own eyes - never the path the browser takes.

use core::fmt;

use serde::{Deserialize, Serialize};

use super::catalog::{self, Catalog};

/// One value filling one blank.
///
/// A `Vec` of these rather than a map: there are rarely more than two, the
/// order is stable so the wire form is stable, and a map would sort them by
/// name for no benefit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arg {
    pub name: String,
    pub value: String,
}

/// A sentence identified by key, with its blanks filled.
///
/// Built by [`msg!`](crate::msg) rather than by hand, because the macro is what
/// checks the key exists at compile time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub key: String,

    /// Empty for the overwhelming majority of messages, and skipped on the wire
    /// when it is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<Arg>,

    /// Set when the sentence has a singular and a plural form, in which case
    /// `key` names the stem and the catalog holds `{key}.one` and
    /// `{key}.other`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,

    /// `key` is finished text, not a key. See [`Message::literal`].
    #[serde(default, skip_serializing_if = "is_false")]
    pub literal: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

impl Message {
    /// A message with no blanks.
    ///
    /// Public because the wire needs it and tests want it, but call sites
    /// should use [`msg!`](crate::msg): this accepts any string, including one
    /// that names no sentence at all.
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            args: Vec::new(),
            count: None,
            literal: false,
        }
    }

    /// Text that has not been keyed yet.
    ///
    /// The migration hatch, and deliberately an ugly one. Some corners of the
    /// application still assemble English - file rejections interpolate a file
    /// type's label, and those labels are themselves untranslated - and the
    /// choice is between wrapping them and holding up everything else until
    /// they are done.
    ///
    /// Every call site is a thing still to do, and `Message::literal` is what
    /// makes them greppable. The job is finished when this function has no
    /// callers and can be deleted.
    pub fn literal(text: impl Into<String>) -> Self {
        Self {
            key: text.into(),
            args: Vec::new(),
            count: None,
            literal: true,
        }
    }

    /// Fill one blank. `{name}` in the sentence becomes `value`.
    #[must_use]
    pub fn arg(mut self, name: impl Into<String>, value: impl fmt::Display) -> Self {
        self.args.push(Arg {
            name: name.into(),
            value: value.to_string(),
        });
        self
    }

    /// Mark the message plural, and fill `{count}`.
    #[must_use]
    pub fn count(mut self, count: i64) -> Self {
        self.count = Some(count);
        self.arg("count", count)
    }

    /// Resolve against a catalog.
    pub fn render(&self, catalog: &Catalog) -> String {
        catalog.render(self)
    }

    /// Resolve against the built-in English, whatever else is loaded.
    ///
    /// The fallback of last resort, and the one `Display` uses.
    pub fn render_builtin(&self) -> String {
        catalog::builtin_only().render(self)
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render_builtin())
    }
}

/// Convenience for the many places that hold one message and want it as text.
impl From<Message> for String {
    fn from(message: Message) -> Self {
        message.render_builtin()
    }
}

/// Build a [`Message`], checking the key at compile time.
///
/// ```ignore
/// msg!("validation.password.needs_digit")
/// msg!("validation.password.too_short", min = 12)
/// ```
///
/// The key must be a literal, and it must be one that `i18n/en.json` defines.
/// A typo is a build error naming the key, rather than a screen reading
/// `validation.pasword.needs_digit` that nobody notices until a customer does.
///
/// That check is the single largest reason this is a macro and not a function,
/// and the reason it is worth having rather than reaching for a general
/// translation crate: none of them can fail your build over a key.
#[macro_export]
macro_rules! msg {
    ($key:literal) => {{
        const _: () = ::core::assert!(
            $crate::i18n::catalog::builtin_contains($key),
            ::core::concat!("no such translation key: ", $key),
        );
        $crate::i18n::Message::new($key)
    }};

    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {{
        const _: () = ::core::assert!(
            $crate::i18n::catalog::builtin_contains($key),
            ::core::concat!("no such translation key: ", $key),
        );
        $crate::i18n::Message::new($key)
            $(.arg(::core::stringify!($name), $value))+
    }};
}

/// Build a plural [`Message`]: one sentence for a count of one, another for
/// every other count.
///
/// ```ignore
/// pmsg!("auth.locked", minutes)
/// ```
///
/// `key` names the stem. `i18n/en.json` must define `{key}.one` and
/// `{key}.other`, and `build.rs` refuses a pair with a half missing - the half
/// nobody notices is missing until a count reaches it in production.
///
/// `{count}` is filled automatically.
#[macro_export]
macro_rules! pmsg {
    ($key:literal, $count:expr) => {{
        const _: () = ::core::assert!(
            $crate::i18n::catalog::builtin_contains(::core::concat!($key, ".one")),
            ::core::concat!("no such translation key: ", $key, ".one"),
        );
        const _: () = ::core::assert!(
            $crate::i18n::catalog::builtin_contains(::core::concat!($key, ".other")),
            ::core::concat!("no such translation key: ", $key, ".other"),
        );
        $crate::i18n::Message::new($key).count(::core::convert::Into::<i64>::into($count))
    }};

    ($key:literal, $count:expr, $($name:ident = $value:expr),+ $(,)?) => {{
        const _: () = ::core::assert!(
            $crate::i18n::catalog::builtin_contains(::core::concat!($key, ".one")),
            ::core::concat!("no such translation key: ", $key, ".one"),
        );
        const _: () = ::core::assert!(
            $crate::i18n::catalog::builtin_contains(::core::concat!($key, ".other")),
            ::core::concat!("no such translation key: ", $key, ".other"),
        );
        $crate::i18n::Message::new($key)
            .count(::core::convert::Into::<i64>::into($count))
            $(.arg(::core::stringify!($name), $value))+
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_with_no_blanks_renders_its_sentence() {
        assert_eq!(
            msg!("validation.password.needs_digit").to_string(),
            "Include at least one number."
        );
    }

    #[test]
    fn blanks_are_filled_by_name() {
        assert_eq!(
            msg!("validation.password.too_short", min = 12).to_string(),
            "Use at least 12 characters."
        );

        assert_eq!(
            msg!("validation.workspace_slug.length", min = 3, max = 40).to_string(),
            "Use between 3 and 40 characters."
        );
    }

    #[test]
    fn a_count_of_one_reads_differently() {
        assert_eq!(
            pmsg!("auth.locked", 1_i64).to_string(),
            "Too many failed attempts. Try again in 1 minute."
        );
        assert_eq!(
            pmsg!("auth.locked", 15_i64).to_string(),
            "Too many failed attempts. Try again in 15 minutes."
        );
        // Zero takes the plural form, which is what English does.
        assert_eq!(
            pmsg!("auth.locked", 0_i64).to_string(),
            "Too many failed attempts. Try again in 0 minutes."
        );
    }

    #[test]
    fn messages_survive_the_wire() {
        let message = msg!("validation.password.too_short", min = 12);
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(
            serde_json::from_str::<Message>(&json).unwrap().to_string(),
            message.to_string()
        );

        // A message with nothing in it is the common case, and its wire form
        // carries neither an empty array nor a null.
        let plain = msg!("validation.email.required");
        assert_eq!(
            serde_json::to_string(&plain).unwrap(),
            "{\"key\":\"validation.email.required\"}"
        );
    }

    #[test]
    fn an_unknown_key_renders_as_itself_rather_than_as_nothing() {
        // Only reachable through `Message::new`, which the wire uses: a build
        // that adds a key can send one to a browser running an older bundle.
        // The key is at least searchable; a blank is not.
        assert_eq!(
            Message::new("nothing.defines.this").to_string(),
            "nothing.defines.this"
        );
    }
}
