//! Syntax colouring for the two languages this report ever shows.
//!
//! # Why this is not a JavaScript library
//!
//! The report is server-rendered and the code it displays is already on the
//! server. Highlighting it here means no bundle to ship, no second pass in the
//! browser, no flash of uncoloured code, and nothing to load at the moment the
//! profiler is most wanted - which, per `report`'s module doc, is when
//! everything else is broken. It also keeps the promise
//! `tools/vendor-scalar.mjs` makes about third-party JavaScript: the least of
//! it, pinned, and only where it earns its place.
//!
//! # It is a lexer, not a parser
//!
//! It does not know types from values, or which `impl` is which. It knows
//! strings, comments, numbers, keywords and the shape of a name, which is what
//! makes code readable at a glance. Anything it cannot classify is emitted
//! plain rather than guessed at, so the worst case is uncoloured text and
//! never wrong text.
//!
//! # Multi-line constructs
//!
//! A block comment or a raw string can span lines, and the source view emits
//! one element per line - so a span may not cross a newline. [`per_line`]
//! tokenises the whole text once and then closes and reopens the span at each
//! boundary, which keeps the markup well formed without the lexer having to
//! know anything about lines.

use std::fmt::Write as _;

use crate::report::escape;

/// What the report can colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Sql,
}

/// A token's CSS class, or plain text.
///
/// Short names because they repeat on every line of every panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Plain,
    Keyword,
    Str,
    Number,
    Comment,
    Type,
    Function,
    Attribute,
    Lifetime,
    Macro,
    Param,
}

impl Class {
    fn name(self) -> Option<&'static str> {
        match self {
            Self::Plain => None,
            Self::Keyword => Some("k"),
            Self::Str => Some("s"),
            Self::Number => Some("n"),
            Self::Comment => Some("c"),
            Self::Type => Some("t"),
            Self::Function => Some("f"),
            Self::Attribute => Some("a"),
            Self::Lifetime => Some("l"),
            Self::Macro => Some("m"),
            Self::Param => Some("v"),
        }
    }
}

/// Space-separated rather than an array of literals.
///
/// rustfmt explodes an array to one element per line as soon as a single
/// element passes its width threshold, and fifty-five one-word lines is not a
/// more readable list of keywords than a paragraph of them.
const RUST_KEYWORDS: &str = concat!(
    "as async await break const continue crate dyn else enum extern false ",
    "fn for if impl in let loop match mod move mut pub ref return self Self ",
    "static struct super trait true type union unsafe use where while",
);

const SQL_KEYWORDS: &str = concat!(
    "and as asc between by case cast conflict count create delete desc ",
    "distinct do drop else end exists from full group having in inner insert ",
    "into is join left like limit not null nothing offset on or order outer ",
    "returning right select set table then union update values when where with",
);

/// Whether `word` is in a space-separated list.
///
/// `fold` for SQL, which is written in either case and means the same thing;
/// Rust's keywords are not, and `Self` is a different word from `self`.
fn listed(list: &str, word: &str, fold: bool) -> bool {
    list.split_ascii_whitespace().any(|entry| {
        if fold {
            entry.eq_ignore_ascii_case(word)
        } else {
            entry == word
        }
    })
}

/// Colour a whole block, newlines and all.
///
/// For the SQL panel, where a statement is one element.
pub fn block(lang: Lang, text: &str) -> String {
    let mut html = String::with_capacity(text.len() * 2);

    for (class, piece) in tokens(lang, text) {
        push(&mut html, class, piece);
    }

    html
}

/// Colour a block and hand back one string of HTML per line.
///
/// The source view numbers its gutter, so each line is its own element and a
/// span may not cross the boundary between two.
pub fn per_line(lang: Lang, text: &str) -> Vec<String> {
    let mut lines = vec![String::new()];

    for (class, piece) in tokens(lang, text) {
        // A token holding newlines is a block comment or a multi-line string.
        // Split it and let each part carry its own span.
        for (index, part) in piece.split('\n').enumerate() {
            if index > 0 {
                lines.push(String::new());
            }

            if !part.is_empty() {
                let line = lines.last_mut().expect("there is always a current line");
                push(line, class, part);
            }
        }
    }

    lines
}

fn push(html: &mut String, class: Class, text: &str) {
    match class.name() {
        Some(name) => {
            let _ = write!(html, "<span class=\"{name}\">{}</span>", escape(text));
        }
        None => html.push_str(&escape(text)),
    }
}

/// Split `text` into classified pieces, in order, covering every byte.
///
/// The invariant that makes this safe to slice: every delimiter it looks for is
/// ASCII, and no byte of a multi-byte UTF-8 sequence is ASCII, so a boundary
/// found here is always a character boundary.
fn tokens(lang: Lang, text: &str) -> Vec<(Class, &str)> {
    let bytes = text.as_bytes();
    let mut out: Vec<(Class, &str)> = Vec::new();
    let mut at = 0;

    while at < bytes.len() {
        let start = at;
        let class = match lang {
            Lang::Rust => rust_token(bytes, &mut at),
            Lang::Sql => sql_token(bytes, &mut at),
        };

        // A lexer that does not advance is a hang, and a hang in a report is
        // indistinguishable from a server that has died. Belt and braces.
        if at == start {
            at += 1;
        }

        let piece = &text[start..at];

        // Merge neighbouring plain runs so the output is not one span per
        // character of ordinary punctuation.
        match out.last_mut() {
            Some((last, previous)) if *last == class && class == Class::Plain => {
                let joined = &text[start - previous.len()..at];
                *previous = joined;
            }
            _ => out.push((class, piece)),
        }
    }

    out
}

fn rust_token(bytes: &[u8], at: &mut usize) -> Class {
    match bytes[*at] {
        b'/' if bytes.get(*at + 1) == Some(&b'/') => {
            take_while(bytes, at, |byte| byte != b'\n');
            Class::Comment
        }
        b'/' if bytes.get(*at + 1) == Some(&b'*') => {
            block_comment(bytes, at);
            Class::Comment
        }
        b'r' if matches!(bytes.get(*at + 1), Some(b'"') | Some(b'#')) => {
            raw_string(bytes, at);
            Class::Str
        }
        b'"' => {
            string(bytes, at, b'"');
            Class::Str
        }
        // `'a` is a lifetime, `'a'` is a character. The difference is what
        // comes two bytes along.
        b'\'' => {
            if bytes.get(*at + 2) == Some(&b'\'') || bytes.get(*at + 1) == Some(&b'\\') {
                string(bytes, at, b'\'');
                Class::Str
            } else {
                *at += 1;
                take_while(bytes, at, is_word);
                Class::Lifetime
            }
        }
        b'#' if bytes.get(*at + 1) == Some(&b'[') || bytes.get(*at + 1) == Some(&b'!') => {
            attribute(bytes, at);
            Class::Attribute
        }
        byte if byte.is_ascii_digit() => {
            take_while(bytes, at, |byte| {
                byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.'
            });
            Class::Number
        }
        byte if is_word_start(byte) => {
            let start = *at;
            take_while(bytes, at, is_word);
            let word = &bytes[start..*at];

            if bytes.get(*at) == Some(&b'!') {
                *at += 1;
                return Class::Macro;
            }

            let word = std::str::from_utf8(word).unwrap_or_default();

            if listed(RUST_KEYWORDS, word, false) {
                Class::Keyword
            } else if word.starts_with(|first: char| first.is_uppercase()) {
                Class::Type
            } else if bytes.get(*at) == Some(&b'(') {
                Class::Function
            } else {
                Class::Plain
            }
        }
        _ => {
            *at += 1;
            Class::Plain
        }
    }
}

fn sql_token(bytes: &[u8], at: &mut usize) -> Class {
    match bytes[*at] {
        b'-' if bytes.get(*at + 1) == Some(&b'-') => {
            take_while(bytes, at, |byte| byte != b'\n');
            Class::Comment
        }
        b'/' if bytes.get(*at + 1) == Some(&b'*') => {
            block_comment(bytes, at);
            Class::Comment
        }
        b'\'' => {
            string(bytes, at, b'\'');
            Class::Str
        }
        // A quoted identifier, which is a name rather than a value - but
        // colouring it as a string is closer to right than leaving it plain.
        b'"' => {
            string(bytes, at, b'"');
            Class::Str
        }
        // `$1`, which is most of what a prepared statement's interesting parts
        // look like - sqlx logs them unsubstituted.
        b'$' => {
            *at += 1;
            take_while(bytes, at, |byte| byte.is_ascii_digit());
            Class::Param
        }
        byte if byte.is_ascii_digit() => {
            take_while(bytes, at, |byte| byte.is_ascii_digit() || byte == b'.');
            Class::Number
        }
        byte if is_word_start(byte) => {
            let start = *at;
            take_while(bytes, at, is_word);
            let word = std::str::from_utf8(&bytes[start..*at]).unwrap_or_default();

            if listed(SQL_KEYWORDS, word, true) {
                Class::Keyword
            } else {
                Class::Plain
            }
        }
        _ => {
            *at += 1;
            Class::Plain
        }
    }
}

fn take_while(bytes: &[u8], at: &mut usize, mut wanted: impl FnMut(u8) -> bool) {
    while *at < bytes.len() && wanted(bytes[*at]) {
        *at += 1;
    }
}

fn is_word_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

/// From the opening quote to the closing one, honouring backslash escapes.
///
/// An unterminated string runs to the end of the text rather than panicking:
/// this is a window onto a file, and the window can cut a string in half.
fn string(bytes: &[u8], at: &mut usize, quote: u8) {
    *at += 1;

    while *at < bytes.len() {
        match bytes[*at] {
            b'\\' => *at += 2,
            byte if byte == quote => {
                *at += 1;
                return;
            }
            _ => *at += 1,
        }
    }
}

/// `r"..."`, `r#"..."#`, `r##"..."##` - the hash count has to match.
fn raw_string(bytes: &[u8], at: &mut usize) {
    *at += 1;

    let mut hashes = 0;
    while bytes.get(*at) == Some(&b'#') {
        hashes += 1;
        *at += 1;
    }

    if bytes.get(*at) != Some(&b'"') {
        return;
    }

    *at += 1;

    while *at < bytes.len() {
        if bytes[*at] == b'"' {
            let closing = *at + 1;
            let matched = bytes[closing..]
                .iter()
                .take(hashes)
                .filter(|byte| **byte == b'#')
                .count();

            if matched == hashes {
                *at = closing + hashes;
                return;
            }
        }

        *at += 1;
    }
}

/// Nested, because Rust's are.
fn block_comment(bytes: &[u8], at: &mut usize) {
    *at += 2;
    let mut depth = 1;

    while *at < bytes.len() && depth > 0 {
        if bytes[*at] == b'/' && bytes.get(*at + 1) == Some(&b'*') {
            depth += 1;
            *at += 2;
        } else if bytes[*at] == b'*' && bytes.get(*at + 1) == Some(&b'/') {
            depth -= 1;
            *at += 2;
        } else {
            *at += 1;
        }
    }
}

/// `#[derive(Debug)]` and `#![allow(...)]`, to the matching bracket.
fn attribute(bytes: &[u8], at: &mut usize) {
    take_while(bytes, at, |byte| byte != b'[');

    let mut depth = 0;

    while *at < bytes.len() {
        match bytes[*at] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                *at += 1;

                if depth == 0 {
                    return;
                }

                continue;
            }
            _ => {}
        }

        *at += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(lang: Lang, text: &str) -> Vec<(Class, &str)> {
        tokens(lang, text)
    }

    /// The invariant everything else rests on: the pieces put the input back
    /// together. A lexer that drops a byte silently deletes code from the
    /// panel, which is worse than not colouring it at all.
    #[test]
    fn every_byte_survives_tokenising() {
        for text in [
            "let x = \"a\\\"b\"; // done",
            "SELECT id FROM t WHERE slug = $1 -- note",
            "/* nested /* deeper */ still */ after",
            "r#\"raw \" string\"# end",
            "let s = \"unterminated",
            "",
            "héllo // ünïcode",
        ] {
            for lang in [Lang::Rust, Lang::Sql] {
                let joined: String = classes(lang, text)
                    .iter()
                    .map(|(_, piece)| *piece)
                    .collect();

                assert_eq!(joined, text, "{lang:?} lost or duplicated bytes");
            }
        }
    }

    #[test]
    fn rust_keywords_strings_and_comments_are_found() {
        let found = classes(Lang::Rust, "let name = \"x\"; // why");

        assert!(found.contains(&(Class::Keyword, "let")));
        assert!(found.contains(&(Class::Str, "\"x\"")));
        assert!(found.contains(&(Class::Comment, "// why")));
    }

    /// `'a` and `'a'` differ by one byte and mean entirely different things.
    #[test]
    fn a_lifetime_is_not_a_character() {
        assert!(classes(Lang::Rust, "&'a str").contains(&(Class::Lifetime, "'a")));
        assert!(classes(Lang::Rust, "let c = 'a';").contains(&(Class::Str, "'a'")));
    }

    #[test]
    fn a_type_a_call_and_a_macro_are_told_apart() {
        let found = classes(Lang::Rust, "Duration::from_millis(9); write!(f)");

        assert!(found.contains(&(Class::Type, "Duration")));
        assert!(found.contains(&(Class::Function, "from_millis")));
        assert!(found.contains(&(Class::Macro, "write!")));
    }

    /// A raw string's hashes have to match, or the rest of the file is eaten
    /// as one string.
    #[test]
    fn a_raw_string_ends_on_the_matching_hashes() {
        let found = classes(Lang::Rust, "let q = r#\"a \"quoted\" thing\"#; let y = 1;");

        assert!(found.contains(&(Class::Str, "r#\"a \"quoted\" thing\"#")));
        assert!(found.contains(&(Class::Keyword, "let")));
        assert!(found.contains(&(Class::Number, "1")));
    }

    #[test]
    fn sql_keywords_are_case_insensitive_and_params_stand_out() {
        let found = classes(Lang::Sql, "select id from tenants where slug = $1");

        assert!(found.contains(&(Class::Keyword, "select")));
        assert!(found.contains(&(Class::Keyword, "where")));
        assert!(found.contains(&(Class::Param, "$1")));
    }

    /// A span may never cross a line, or the gutter view emits broken markup.
    #[test]
    fn a_block_comment_is_closed_and_reopened_on_each_line() {
        let lines = per_line(Lang::Rust, "/* one\ntwo */\nlet x = 1;");

        assert_eq!(lines.len(), 3);

        for line in &lines {
            assert_eq!(
                line.matches("<span").count(),
                line.matches("</span>").count(),
                "unbalanced spans in {line:?}"
            );
        }
    }

    /// Markup in the source itself must reach the page as text. The report has
    /// this test for every other panel; a code view is no exception.
    #[test]
    fn code_cannot_carry_markup_into_the_page() {
        let html = block(Lang::Rust, "let x = \"<script>alert(1)</script>\";");

        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn plain_runs_are_merged_rather_than_split_per_character() {
        let found = classes(Lang::Rust, "a + b - c");

        assert!(
            found.len() < 5,
            "ordinary punctuation should not be one span each: {found:?}"
        );
    }

    /// An empty line has to survive as an empty line, or the gutter numbers
    /// stop matching the file.
    #[test]
    fn blank_lines_are_kept() {
        assert_eq!(per_line(Lang::Rust, "a\n\nb").len(), 3);
    }
}
