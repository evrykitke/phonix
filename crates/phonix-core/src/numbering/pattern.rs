//! The format a document number is rendered from.

use core::fmt;

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use super::period::{MAX_SCOPE_LEN, fiscal_year};
use crate::i18n::Message;
use crate::msg;

/// Longest pattern accepted, matching the column's CHECK.
pub const MAX_PATTERN_LEN: usize = 60;

/// Widest zero-padded counter. Eighteen digits is what a `BIGINT` counter can
/// reach without the padding becoming a lie.
pub const MAX_COUNTER_WIDTH: usize = 18;

/// The counter [`Pattern::preview`] renders, so a settings screen shows a
/// number that is obviously an example rather than one somebody might quote.
pub const SAMPLE_COUNTER: i64 = 42;

/// A validated document-number format: `INV-{YYYY}-{NNNNNN}`, or `#-#####-####`.
///
/// Parsed once and rendered many times. The validation is not decoration - a
/// pattern reaches a legal document, and the failure modes it rules out are all
/// silent ones. A typo like `{YYY}` would otherwise print literally on an
/// invoice, and a pattern with no counter at all would give every document in
/// the workspace the same number.
///
/// # Tokens
///
/// | Token     | Renders                                        |
/// | --------- | ---------------------------------------------- |
/// | `{YYYY}`  | Calendar year, four digits                     |
/// | `{YY}`    | Calendar year, two digits                      |
/// | `{MM}`    | Month, zero-padded                             |
/// | `{DD}`    | Day, zero-padded                               |
/// | `{FY}`    | Financial year — see [`fiscal_year`]           |
/// | `{SCOPE}` | The sequence's scope key: a branch, a till     |
/// | `{N...}`  | Counter digits, as many as there are `N`s      |
/// | `#`       | One counter digit                              |
///
/// Upper case, exactly as written. `{nnnn}` is refused rather than treated as a
/// counter, because the alternative is a pattern that looks right in a settings
/// box and prints `{nnnn}` on the document.
///
/// # The counter is one field, however it is spelled
///
/// `#` and `{N...}` are two spellings of the same thing: a digit slot. **Every**
/// slot in a pattern belongs to the same counter, filled right to left and
/// zero-padded, with whatever lies between the groups kept verbatim. That is
/// what makes a grouped reference number work:
///
/// ```text
/// #-#####-####      counter        42  ->  0-00000-0042
///                   counter   123_456  ->  0-00012-3456
/// INV-{YYYY}-#####  counter        42  ->  INV-2026-00042
/// ```
///
/// Mixing the two spellings in one pattern is refused. Not because it could not
/// be given a meaning, but because `INV #{NNNNN}` reads as a hash followed by a
/// five-digit counter and would render a *six*-digit one - a pattern that looks
/// right in a settings box and prints something else on the document, which is
/// the failure this whole type exists to prevent. Write `INV-{NNNNN}` or
/// `INV-#####` and the question does not arise.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Pattern {
    raw: String,
    /// Compiled once, so rendering is a walk rather than a re-parse - and so
    /// that each run of slots knows how much of the counter lies to its right.
    pieces: Vec<Piece>,
    counter_width: usize,
}

impl Pattern {
    /// Validate a pattern.
    pub fn parse(raw: &str) -> Result<Self, PatternError> {
        let raw = raw.trim();

        if raw.is_empty() {
            return Err(PatternError::Empty);
        }
        if raw.chars().count() > MAX_PATTERN_LEN {
            return Err(PatternError::TooLong);
        }

        let mut pieces: Vec<Piece> = Vec::new();
        let mut literal = String::new();
        let mut counter_width = 0usize;
        let mut braces = false;
        let mut hashes = false;

        let mut chars = raw.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '{' => {
                    // Read to the matching brace. A nested `{`, or none at all,
                    // is a typo rather than something to render literally: the
                    // character it would print is a character nobody wants on
                    // an invoice.
                    let mut name = String::new();
                    let mut closed = false;
                    for inner in chars.by_ref() {
                        match inner {
                            '}' => {
                                closed = true;
                                break;
                            }
                            '{' => return Err(PatternError::Unbalanced),
                            _ => name.push(inner),
                        }
                    }
                    if !closed {
                        return Err(PatternError::Unbalanced);
                    }

                    push_literal(&mut pieces, &mut literal);
                    match Token::parse(&name)? {
                        Token::Date(part) => pieces.push(Piece::Date(part)),
                        Token::Scope => pieces.push(Piece::Scope),
                        Token::Slots(width) => {
                            braces = true;
                            counter_width = counter_width.saturating_add(width);
                            pieces.push(Piece::Slots { tail: 0 });
                        }
                    }
                }
                // A closing brace with no opening one before it.
                '}' => return Err(PatternError::Unbalanced),
                '#' => {
                    let mut width = 1usize;
                    while chars.peek() == Some(&'#') {
                        chars.next();
                        width = width.saturating_add(1);
                    }
                    hashes = true;
                    counter_width = counter_width.saturating_add(width);
                    push_literal(&mut pieces, &mut literal);
                    pieces.push(Piece::Slots { tail: 0 });
                }
                _ => literal.push(ch),
            }
        }
        push_literal(&mut pieces, &mut literal);

        if braces && hashes {
            return Err(PatternError::MixedCounters);
        }
        if counter_width == 0 {
            return Err(PatternError::NoCounter);
        }
        if counter_width > MAX_COUNTER_WIDTH {
            return Err(PatternError::CounterTooWide);
        }

        // Each run of slots needs to know how many counter digits sit to its
        // right, which is only knowable once the whole pattern has been read.
        // This is what lets `#-#####-####` fill from the right without the
        // renderer having to look ahead.
        assign_tails(raw, &mut pieces);

        Ok(Self {
            raw: raw.to_owned(),
            pieces,
            counter_width,
        })
    }

    /// The pattern as stored.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// How many digits the counter is padded to, across every run of slots.
    ///
    /// `#-#####-####` is ten, not one.
    pub const fn counter_width(&self) -> usize {
        self.counter_width
    }

    /// Render a number.
    ///
    /// Infallible: every failure this can have was settled by [`parse`](Self::parse).
    ///
    /// A counter past its padding **widens the number** rather than being
    /// refused or truncated. `INV-{NNNN}` issues `INV-10000` on the
    /// ten-thousandth document. Ugly, and the alternative is refusing to
    /// invoice - a business does not stop trading because its number format ran
    /// out of digits, and truncating would issue a duplicate. In a grouped
    /// pattern the **leftmost** run is the one that widens, so `#-#####-####`
    /// at ten billion is `10-00000-0000`: the groups a reader has learned the
    /// shape of keep it.
    ///
    /// The settings screen is where somebody widens the pattern before any of
    /// that happens.
    pub fn render(&self, context: NumberContext<'_>) -> String {
        // `unsigned_abs` so the digit string is digits and nothing else. A
        // negative counter cannot occur - the column has `CHECK (counter >= 0)`
        // - and a stray minus sign must not be able to shift a group.
        let digits = format!(
            "{:0width$}",
            context.counter.unsigned_abs(),
            width = self.counter_width
        );
        let mut remaining = digits.as_str();

        let mut out = String::with_capacity(self.raw.len() + self.counter_width);
        for piece in &self.pieces {
            match piece {
                Piece::Literal(text) => out.push_str(text),
                Piece::Date(part) => part.render(&mut out, &context),
                Piece::Scope => out.push_str(context.scope),
                Piece::Slots { tail } => {
                    // Everything except what the runs to the right have
                    // reserved. On the leftmost run that is its own width plus
                    // any overflow, which is what makes the number widen there.
                    let take = remaining.len().saturating_sub(*tail);
                    if let Some(chunk) = remaining.get(..take) {
                        out.push_str(chunk);
                    }
                    remaining = remaining.get(take..).unwrap_or("");
                }
            }
        }
        out
    }

    /// Render an example, for a settings screen.
    ///
    /// A different act from rendering a real number, and the reason it is a
    /// separate function: this one is safe to show at any time. Handing a
    /// document the number it is *going* to get promises something that may
    /// not be kept - see the module documentation.
    pub fn preview(&self, on: NaiveDate, scope: &str, fiscal_year_start_month: u8) -> String {
        self.render(NumberContext {
            counter: SAMPLE_COUNTER,
            on,
            scope,
            fiscal_year_start_month,
        })
    }
}

/// Flush the literal text collected since the last token.
fn push_literal(pieces: &mut Vec<Piece>, literal: &mut String) {
    if !literal.is_empty() {
        pieces.push(Piece::Literal(core::mem::take(literal)));
    }
}

/// Fill in each slot run's `tail`: how many counter digits lie to its right.
fn assign_tails(raw: &str, pieces: &mut [Piece]) {
    let mut widths = run_widths(raw);
    let mut tail = 0usize;
    for piece in pieces.iter_mut().rev() {
        if let Piece::Slots { tail: slot } = piece {
            *slot = tail;
            tail = tail.saturating_add(widths.pop().unwrap_or(0));
        }
    }
}

/// The width of each run of counter slots, left to right.
///
/// A second, deliberately simple pass over the raw pattern. The alternative is
/// carrying per-run widths through the parse, and a run's own width is wanted
/// in exactly one place.
fn run_widths(raw: &str) -> Vec<usize> {
    let mut widths = Vec::new();
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                let mut name = String::new();
                for inner in chars.by_ref() {
                    if inner == '}' {
                        break;
                    }
                    name.push(inner);
                }
                if !name.is_empty() && name.bytes().all(|byte| byte == b'N') {
                    widths.push(name.len());
                }
            }
            '#' => {
                let mut width = 1usize;
                while chars.peek() == Some(&'#') {
                    chars.next();
                    width = width.saturating_add(1);
                }
                widths.push(width);
            }
            _ => {}
        }
    }
    widths
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

/// Serialised as the bare pattern string, so the column and the wire hold the
/// same characters and neither has to know about this struct.
impl Serialize for Pattern {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.raw)
    }
}

impl<'de> Deserialize<'de> for Pattern {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// Everything a pattern needs in order to become a number.
#[derive(Debug, Clone, Copy)]
pub struct NumberContext<'a> {
    /// The value the sequence just allocated.
    pub counter: i64,
    /// The document's own date, not today. A document backdated into last month
    /// carries last month's tokens, which is what makes a monthly reset and the
    /// number it prints agree.
    pub on: NaiveDate,
    /// The sequence's scope key: a branch, a till, a warehouse.
    pub scope: &'a str,
    /// From `organization_profile`.
    pub fiscal_year_start_month: u8,
}

/// One compiled step of a pattern.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Piece {
    Literal(String),
    Date(DatePart),
    Scope,
    /// A run of counter digits, carrying how many digits lie to its right.
    Slots {
        tail: usize,
    },
}

/// What a `{...}` turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    Date(DatePart),
    Scope,
    Slots(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DatePart {
    Year4,
    Year2,
    Month,
    Day,
    FiscalYear,
}

impl Token {
    fn parse(name: &str) -> Result<Self, PatternError> {
        match name {
            "YYYY" => Ok(Self::Date(DatePart::Year4)),
            "YY" => Ok(Self::Date(DatePart::Year2)),
            "MM" => Ok(Self::Date(DatePart::Month)),
            "DD" => Ok(Self::Date(DatePart::Day)),
            "FY" => Ok(Self::Date(DatePart::FiscalYear)),
            "SCOPE" => Ok(Self::Scope),
            _ if !name.is_empty() && name.bytes().all(|byte| byte == b'N') => {
                if name.len() > MAX_COUNTER_WIDTH {
                    Err(PatternError::CounterTooWide)
                } else {
                    Ok(Self::Slots(name.len()))
                }
            }
            _ => Err(PatternError::UnknownToken),
        }
    }
}

impl DatePart {
    fn render(self, out: &mut String, context: &NumberContext<'_>) {
        match self {
            Self::Year4 => out.push_str(&format!("{:04}", context.on.year())),
            // `% 100` on a negative year would render a sign. Years before 1 AD
            // are not a case, but `unsigned_abs` costs nothing and cannot be
            // the reason a number comes out with a hyphen in the middle.
            Self::Year2 => {
                out.push_str(&format!("{:02}", context.on.year().unsigned_abs() % 100));
            }
            Self::Month => out.push_str(&format!("{:02}", context.on.month())),
            Self::Day => out.push_str(&format!("{:02}", context.on.day())),
            Self::FiscalYear => {
                let year = fiscal_year(context.on, context.fiscal_year_start_month);
                out.push_str(&format!("{year:04}"));
            }
        }
    }
}

/// Why a pattern was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
    #[error("a document number pattern cannot be empty")]
    Empty,
    #[error("a document number pattern is at most {MAX_PATTERN_LEN} characters")]
    TooLong,
    #[error("every {{ in a pattern needs a matching }}")]
    Unbalanced,
    #[error("that is not a token this build understands")]
    UnknownToken,
    #[error("a pattern needs a counter, written as #s or as a run of Ns")]
    NoCounter,
    #[error("use either #s or a run of Ns for the counter, not both")]
    MixedCounters,
    #[error("a counter is at most {MAX_COUNTER_WIDTH} digits wide")]
    CounterTooWide,
}

impl PatternError {
    /// What to say to whoever typed it. Every one of these reaches a form.
    pub fn message(self) -> Message {
        match self {
            Self::Empty => msg!("numbering.error.empty"),
            Self::TooLong => msg!("numbering.error.too_long", max = MAX_PATTERN_LEN),
            Self::Unbalanced => msg!("numbering.error.unbalanced"),
            Self::UnknownToken => msg!("numbering.error.unknown_token"),
            Self::NoCounter => msg!("numbering.error.no_counter"),
            Self::MixedCounters => msg!("numbering.error.mixed_counters"),
            Self::CounterTooWide => {
                msg!("numbering.error.counter_too_wide", max = MAX_COUNTER_WIDTH)
            }
        }
    }
}

/// Whether a scope key is one the column and `{SCOPE}` will accept.
///
/// Restricted rather than free text because it is rendered straight into a
/// document number: a scope with a slash or a space in it produces a number
/// that cannot be typed into a search box or quoted down a phone.
pub fn is_valid_scope(scope: &str) -> bool {
    scope.len() <= MAX_SCOPE_LEN
        && scope
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(year: i32, month: u32, of: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, of).unwrap()
    }

    fn render(pattern: &str, counter: i64) -> String {
        Pattern::parse(pattern).unwrap().render(NumberContext {
            counter,
            on: day(2026, 8, 24),
            scope: "NBO",
            fiscal_year_start_month: 4,
        })
    }

    #[test]
    fn the_example_from_the_design_renders_as_written() {
        assert_eq!(render("INV-{YYYY}-{NNNNNN}", 42), "INV-2026-000042");
    }

    #[test]
    fn every_token_renders() {
        assert_eq!(render("{YYYY}|{NNNN}", 1), "2026|0001");
        assert_eq!(render("{YY}|{NNNN}", 1), "26|0001");
        assert_eq!(render("{MM}|{NNNN}", 1), "08|0001");
        assert_eq!(render("{DD}|{NNNN}", 1), "24|0001");
        assert_eq!(render("{SCOPE}|{NNNN}", 1), "NBO|0001");
        // August 2026 with an April year opening is FY2026.
        assert_eq!(render("{FY}|{NNNN}", 1), "2026|0001");
    }

    #[test]
    fn a_hash_is_one_counter_digit() {
        assert_eq!(render("#####", 42), "00042");
        assert_eq!(render("INV-#####", 42), "INV-00042");
        assert_eq!(
            Pattern::parse("#####").unwrap().counter_width(),
            Pattern::parse("{NNNNN}").unwrap().counter_width()
        );
    }

    #[test]
    fn a_grouped_mask_fills_from_the_right() {
        // The shape the design asked for: one counter, groups kept.
        assert_eq!(render("#-#####-####", 1), "0-00000-0001");
        assert_eq!(render("#-#####-####", 42), "0-00000-0042");
        assert_eq!(render("#-#####-####", 123_456), "0-00012-3456");
        assert_eq!(render("#-#####-####", 9_999_999_999), "9-99999-9999");
    }

    #[test]
    fn groups_can_carry_other_tokens_between_them() {
        assert_eq!(render("{YYYY}-##-####", 7), "2026-00-0007");
        assert_eq!(render("{SCOPE}/###/###", 1_234), "NBO/001/234");
    }

    #[test]
    fn a_fiscal_year_token_follows_the_organizations_own_year() {
        let pattern = Pattern::parse("FY{FY}/{NNNN}").unwrap();
        let march = NumberContext {
            counter: 1,
            on: day(2026, 3, 31),
            scope: "",
            fiscal_year_start_month: 4,
        };
        let april = NumberContext {
            on: day(2026, 4, 1),
            ..march
        };

        assert_eq!(pattern.render(march), "FY2025/0001");
        assert_eq!(pattern.render(april), "FY2026/0001");
    }

    #[test]
    fn a_pattern_with_no_tokens_but_a_counter_is_fine() {
        assert_eq!(render("{NNNNN}", 7), "00007");
    }

    #[test]
    fn text_around_and_between_tokens_survives() {
        assert_eq!(render("A{YYYY}B{NNN}C", 5), "A2026B005C");
        assert_eq!(render("{NNN} trailing", 5), "005 trailing");
        assert_eq!(render("leading {NNN}", 5), "leading 005");
    }

    #[test]
    fn a_counter_past_its_padding_widens_rather_than_truncating() {
        // Truncating would issue a duplicate; refusing would stop the business
        // invoicing. Widening is the only option that does neither.
        assert_eq!(render("INV-{NNNN}", 9_999), "INV-9999");
        assert_eq!(render("INV-{NNNN}", 10_000), "INV-10000");
        assert_eq!(render("INV-{NNNN}", 123_456_789), "INV-123456789");
    }

    #[test]
    fn the_leftmost_group_is_the_one_that_widens() {
        // The right-hand groups are the ones a reader has learned the shape of,
        // so overflow is pushed left rather than smeared across all of them.
        assert_eq!(render("#-#####-####", 10_000_000_000), "10-00000-0000");
        assert_eq!(render("##-####", 1_234_567), "123-4567");
    }

    #[test]
    fn a_pattern_without_a_counter_is_refused() {
        // Otherwise every document in the workspace shares one number.
        assert_eq!(Pattern::parse("INV-{YYYY}"), Err(PatternError::NoCounter));
        assert_eq!(Pattern::parse("INVOICE"), Err(PatternError::NoCounter));
    }

    #[test]
    fn two_runs_of_one_spelling_are_one_counter() {
        // The first draft refused this outright. It has a reading now, and it
        // is the same reading `##-##` has.
        assert_eq!(render("{NN}-{NN}", 1_234), "12-34");
    }

    #[test]
    fn mixing_the_two_spellings_is_refused() {
        // `INV #{NNNNN}` reads as a hash and a five-digit counter, and would
        // render a six-digit one. Refusing is the only answer that cannot
        // surprise anybody.
        for bad in ["INV #{NNNNN}", "{NNN}-###", "##{NN}"] {
            assert_eq!(
                Pattern::parse(bad),
                Err(PatternError::MixedCounters),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn a_typo_is_refused_rather_than_printed_on_an_invoice() {
        for bad in [
            "{YYY}-{NNN}",
            "{nnnn}",
            "{Year}-{NNN}",
            "{}-{NNN}",
            "{ }{NNN}",
        ] {
            assert_eq!(
                Pattern::parse(bad),
                Err(PatternError::UnknownToken),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn an_unbalanced_brace_is_refused() {
        for bad in ["{NNN", "NNN}", "{NNN}}", "}{NNN}", "{{NNN}"] {
            assert!(
                matches!(
                    Pattern::parse(bad),
                    Err(PatternError::Unbalanced | PatternError::UnknownToken)
                ),
                "{bad:?} must be refused, got {:?}",
                Pattern::parse(bad)
            );
        }
    }

    #[test]
    fn the_ceilings_are_enforced() {
        assert_eq!(Pattern::parse(""), Err(PatternError::Empty));
        assert_eq!(Pattern::parse("   "), Err(PatternError::Empty));

        let wide = format!("{{{}}}", "N".repeat(MAX_COUNTER_WIDTH + 1));
        assert_eq!(Pattern::parse(&wide), Err(PatternError::CounterTooWide));

        // And across groups, not only within one run.
        let half = "#".repeat(MAX_COUNTER_WIDTH / 2 + 1);
        let spread = format!("{half}-{half}");
        assert_eq!(Pattern::parse(&spread), Err(PatternError::CounterTooWide));

        let widest = format!("{{{}}}", "N".repeat(MAX_COUNTER_WIDTH));
        assert_eq!(
            Pattern::parse(&widest).map(|pattern| pattern.counter_width()),
            Ok(MAX_COUNTER_WIDTH)
        );

        let long = format!("{}{{NNN}}", "x".repeat(MAX_PATTERN_LEN));
        assert_eq!(Pattern::parse(&long), Err(PatternError::TooLong));
    }

    #[test]
    fn a_preview_is_obviously_an_example() {
        let pattern = Pattern::parse("INV-{YYYY}-{NNNNNN}").unwrap();
        assert_eq!(pattern.preview(day(2026, 8, 24), "", 1), "INV-2026-000042");

        let mask = Pattern::parse("#-#####-####").unwrap();
        assert_eq!(mask.preview(day(2026, 8, 24), "", 1), "0-00000-0042");
    }

    #[test]
    fn round_trips_through_json_as_the_bare_pattern() {
        let pattern = Pattern::parse("INV-{YYYY}-{NNNNNN}").unwrap();
        let json = serde_json::to_string(&pattern).unwrap();
        assert_eq!(json, r#""INV-{YYYY}-{NNNNNN}""#);
        assert_eq!(serde_json::from_str::<Pattern>(&json).unwrap(), pattern);
        assert!(serde_json::from_str::<Pattern>(r#""no counter""#).is_err());

        let mask = Pattern::parse("#-#####-####").unwrap();
        // `r##"` rather than `r#"`: the content opens with `"#`, which is what
        // ends a single-hash raw string.
        assert_eq!(serde_json::to_string(&mask).unwrap(), r##""#-#####-####""##);
    }

    #[test]
    fn a_scope_has_to_be_typeable() {
        for good in ["", "NBO", "till-2", "WH_01", "0"] {
            assert!(is_valid_scope(good), "{good:?} should be allowed");
        }
        for bad in [
            "a b",
            "a/b",
            "a.b",
            "brânch",
            &"x".repeat(MAX_SCOPE_LEN + 1),
        ] {
            assert!(!is_valid_scope(bad), "{bad:?} should be refused");
        }
    }

    #[test]
    fn every_error_has_something_to_say() {
        for error in [
            PatternError::Empty,
            PatternError::TooLong,
            PatternError::Unbalanced,
            PatternError::UnknownToken,
            PatternError::NoCounter,
            PatternError::MixedCounters,
            PatternError::CounterTooWide,
        ] {
            let message = error.message();
            assert!(message.key.starts_with("numbering."), "{}", message.key);
            assert!(!message.render_builtin().is_empty());
        }
    }
}
