//! A tax rate, and the window it was in force for.
//!
//! # Six decimal places, and why not four
//!
//! `NUMERIC(9, 6)`, held here as an integer at scale 6. Four would be enough
//! for every headline VAT rate ever published and not enough for the ones that
//! are a fraction: India has cess rates quoted to five places, and several US
//! states publish combined district rates like `0.0862500`. A rate rounded on
//! the way in is a rate nobody can reconcile a return against.
//!
//! Nine digits in total leaves three before the point, so 100% is expressible
//! and so is a penalty rate above it. A rate is not capped at 1: withholding
//! and excise arrangements exist that are quoted as multipliers.
//!
//! # Zero is a rate
//!
//! Unlike an exchange rate, which cannot be zero, a zero-rated supply is an
//! ordinary and important thing: it is *not* the same as an exempt one, and the
//! difference is whether input tax can be recovered. So `0.000000` parses, and
//! the zero-rated code carries it.

use std::fmt;

use chrono::NaiveDate;
use phonix_core::Message;
use phonix_core::msg;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Decimal places a rate is held and stored at - the `6` in `NUMERIC(9, 6)`.
pub const RATE_SCALE: u32 = 6;

/// The largest value `NUMERIC(9, 6)` holds: `999.999999`.
pub const MAX_RATE_SCALED: i128 = 999_999_999;

/// `10^RATE_SCALE`, as the denominator of the ratio a rate really is.
pub const RATE_ONE: i128 = 1_000_000;

/// Meaningful decimal places a *percentage* carries.
///
/// Two fewer than the proportion it becomes, because dividing by a hundred is
/// what moves the point. `0.0001%` is the smallest rate `NUMERIC(9, 6)` can
/// tell from zero.
pub const PERCENT_PLACES: u32 = RATE_SCALE - 2;

/// A tax rate, as a proportion. 18% is `0.180000`.
///
/// A proportion rather than a percentage, because that is what the arithmetic
/// wants and a percentage would mean dividing by a hundred somewhere - and
/// "somewhere" is where a factor of a hundred goes missing. The settings screen
/// shows a percentage; the type holds a proportion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaxRate {
    /// The proportion multiplied by `10^RATE_SCALE`.
    scaled: i128,
}

impl TaxRate {
    /// Nothing. A zero-rated supply, which is not an exempt one.
    pub const ZERO: Self = Self { scaled: 0 };

    /// Build from the scaled integer - the proportion times `10^RATE_SCALE`.
    pub fn from_scaled(scaled: i128) -> Result<Self, TaxRateError> {
        if scaled < 0 {
            return Err(TaxRateError::Negative);
        }
        if scaled > MAX_RATE_SCALED {
            return Err(TaxRateError::OutOfRange);
        }
        Ok(Self { scaled })
    }

    /// Parse a proportion: `0.18`, `0.086250`, `0`.
    ///
    /// Up to [`RATE_SCALE`] decimal places; a seventh is refused rather than
    /// rounded, for the reason an amount's fifth is.
    pub fn parse(raw: &str) -> Result<Self, TaxRateError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(TaxRateError::NotANumber);
        }
        if raw.starts_with('-') {
            return Err(TaxRateError::Negative);
        }

        let digits = raw.strip_prefix('+').unwrap_or(raw);
        let (whole, fraction) = match digits.split_once('.') {
            Some((whole, fraction)) => (whole, fraction),
            None => (digits, ""),
        };

        if whole.is_empty()
            || fraction.len() > RATE_SCALE as usize
            || (digits.contains('.') && fraction.is_empty())
            || !whole.bytes().all(|byte| byte.is_ascii_digit())
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(TaxRateError::NotANumber);
        }

        let mut scaled: i128 = 0;
        for byte in whole.bytes().chain(fraction.bytes()) {
            let digit = i128::from(byte - b'0');
            scaled = scaled
                .checked_mul(10)
                .and_then(|shifted| shifted.checked_add(digit))
                .ok_or(TaxRateError::OutOfRange)?;
        }

        let missing = RATE_SCALE - fraction.len() as u32;
        scaled = scaled
            .checked_mul(pow10(missing).ok_or(TaxRateError::OutOfRange)?)
            .ok_or(TaxRateError::OutOfRange)?;

        Self::from_scaled(scaled)
    }

    /// Parse what somebody typed into a percentage box: `18`, `8.625`, `0`.
    ///
    /// A separate constructor rather than a flag, because the two readings of
    /// "18" differ by a factor of a hundred and a caller that has to remember
    /// which one it is holding will eventually forget. The screen says
    /// *percent*; this is the only door that word comes through.
    ///
    /// Dividing by a hundred moves the point two columns, so a percentage
    /// carries **four** meaningful decimal places rather than six -
    /// `0.0001%` is the smallest rate a `NUMERIC(9, 6)` can tell from zero.
    /// Trailing zeroes past that are allowed, because `8.62500%` is a rate
    /// somebody copied out of a published table and it means what it says; a
    /// non-zero seventh digit is refused rather than rounded away.
    pub fn parse_percent(raw: &str) -> Result<Self, TaxRateError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(TaxRateError::NotANumber);
        }
        if raw.starts_with('-') {
            return Err(TaxRateError::Negative);
        }

        let digits = raw.strip_prefix('+').unwrap_or(raw);
        let (whole, fraction) = match digits.split_once('.') {
            Some((whole, fraction)) => (whole, fraction),
            None => (digits, ""),
        };

        if whole.is_empty()
            || fraction.len() > RATE_SCALE as usize
            || (digits.contains('.') && fraction.is_empty())
            || !whole.bytes().all(|byte| byte.is_ascii_digit())
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(TaxRateError::NotANumber);
        }

        let mut typed: i128 = 0;
        for byte in whole.bytes().chain(fraction.bytes()) {
            let digit = i128::from(byte - b'0');
            typed = typed
                .checked_mul(10)
                .and_then(|shifted| shifted.checked_add(digit))
                .ok_or(TaxRateError::OutOfRange)?;
        }

        // The typed value is `typed / 10^places` percent, and the proportion
        // wanted is a hundredth of that at scale 6 - so the point moves by
        // `PERCENT_PLACES - places`, in whichever direction that turns out to
        // be.
        let places = fraction.len() as u32;
        let scaled = if places <= PERCENT_PLACES {
            typed
                .checked_mul(pow10(PERCENT_PLACES - places).ok_or(TaxRateError::OutOfRange)?)
                .ok_or(TaxRateError::OutOfRange)?
        } else {
            let factor = pow10(places - PERCENT_PLACES).ok_or(TaxRateError::OutOfRange)?;
            // Refused rather than rounded, for the reason `parse` refuses a
            // seventh place: a rate that changes on the way in is a rate
            // nobody can reconcile a filed return against.
            if typed % factor != 0 {
                return Err(TaxRateError::NotANumber);
            }
            typed / factor
        };

        Self::from_scaled(scaled)
    }

    /// The proportion times `10^RATE_SCALE`. What goes in the column.
    pub const fn scaled(self) -> i128 {
        self.scaled
    }

    pub const fn is_zero(self) -> bool {
        self.scaled == 0
    }

    /// The digits at full storage scale - what a `NUMERIC(9, 6)` bind wants.
    pub fn to_storage_string(self) -> String {
        decimal_string(self.scaled, RATE_SCALE)
    }

    /// What a settings screen prints: `18%`, `8.625%`, `0%`.
    ///
    /// Trailing zeroes trimmed, because `18.000000%` is a rate written by a
    /// machine and this is read by a person.
    pub fn to_percent_string(self) -> String {
        // Times a hundred is two places off the scale, so the percentage is the
        // same digits read two columns further left.
        let text = decimal_string(self.scaled, RATE_SCALE - 2);
        let trimmed = match text.split_once('.') {
            Some((whole, fraction)) => {
                let fraction = fraction.trim_end_matches('0');
                if fraction.is_empty() {
                    whole.to_owned()
                } else {
                    format!("{whole}.{fraction}")
                }
            }
            None => text,
        };
        format!("{trimmed}%")
    }
}

impl fmt::Display for TaxRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_percent_string())
    }
}

/// Crosses the wire as the string `"0.180000"`, for the reason
/// [`phonix_core::Money`] does: a JSON number is a double in most parsers, and
/// a rate that changes on the way to the browser is a preview that disagrees
/// with the posting.
impl Serialize for TaxRate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_storage_string())
    }
}

impl<'de> Deserialize<'de> for TaxRate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// One rate, and the window it was in force for.
///
/// `valid_to` is exclusive and `None` means "still". Half-open, so the day a
/// rate changes belongs to exactly one row - the alternative is a closed range
/// where somebody eventually writes yesterday's end date as today's start and
/// two rates are live at once. The database enforces the non-overlap with an
/// exclusion constraint; this type is what a screen edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxRatePeriod {
    pub rate: TaxRate,
    pub valid_from: NaiveDate,
    /// Exclusive. `None` is open-ended.
    pub valid_to: Option<NaiveDate>,
}

impl TaxRatePeriod {
    /// Whether this rate governs a document dated `on`.
    pub fn covers(&self, on: NaiveDate) -> bool {
        on >= self.valid_from && self.valid_to.is_none_or(|end| on < end)
    }

    /// Well-formed as a window: it has to open before it closes.
    pub fn check(&self) -> Result<(), TaxRateError> {
        match self.valid_to {
            Some(end) if end <= self.valid_from => Err(TaxRateError::BackwardsPeriod),
            _ => Ok(()),
        }
    }
}

/// A stored rate window, with the identity the database gave it.
///
/// Here rather than in the repository so that it can cross the wire: a rates
/// screen needs the id to edit or remove a window, and the browser is where
/// that screen runs. [`TaxRatePeriod`] is the value; this is the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxRateRow {
    pub id: Uuid,
    pub tax_code_id: Uuid,
    pub period: TaxRatePeriod,
}

/// A rate being added or edited on a screen.
///
/// Separate from [`TaxRatePeriod`] because the screen holds a percentage the
/// person typed, which is not yet known to be a number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxRateInput {
    /// As typed, in percent.
    pub percent: String,
    pub valid_from: NaiveDate,
    pub valid_to: Option<NaiveDate>,
}

impl TaxRateInput {
    /// Turn what was typed into a window, or say what is wrong with it.
    pub fn parse(&self) -> Result<TaxRatePeriod, TaxRateError> {
        let period = TaxRatePeriod {
            rate: TaxRate::parse_percent(&self.percent)?,
            valid_from: self.valid_from,
            valid_to: self.valid_to,
        };
        period.check()?;
        Ok(period)
    }
}

/// What can be wrong with a rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TaxRateError {
    #[error("a tax rate cannot be negative")]
    Negative,
    #[error("a tax rate is outside the range NUMERIC(9, 6) can hold")]
    OutOfRange,
    #[error("not a decimal number with at most 6 decimal places")]
    NotANumber,
    #[error("a rate's period ends before it starts")]
    BackwardsPeriod,
}

impl TaxRateError {
    /// What to say to whoever typed it.
    pub fn message(self) -> Message {
        match self {
            Self::Negative => msg!("tax.error.rate_negative"),
            Self::OutOfRange => msg!("tax.error.rate_out_of_range"),
            Self::NotANumber => msg!("tax.error.rate_not_a_number"),
            Self::BackwardsPeriod => msg!("tax.error.rate_backwards_period"),
        }
    }
}

/// `10^n`, or `None` if it does not fit.
///
/// A loop rather than `i128::pow`, which panics on overflow - and this crate
/// does not panic, because a panic in the wasm bundle freezes the tab.
pub(crate) fn pow10(n: u32) -> Option<i128> {
    let mut value: i128 = 1;
    for _ in 0..n {
        value = value.checked_mul(10)?;
    }
    Some(value)
}

/// Render a non-negative scaled integer with `dp` decimal places.
///
/// `dp` narrower than `scale` simply moves the point - the caller here only
/// ever asks for a narrower `dp` when the digits it drops are known to be
/// wanted on the other side of it.
fn decimal_string(scaled: i128, dp: u32) -> String {
    let Some(factor) = pow10(dp) else {
        // Unreachable: `dp` is at most 6. Returning the raw digits still beats
        // a panic in a wasm bundle.
        return scaled.to_string();
    };

    let whole = scaled / factor;
    let fraction = (scaled % factor).unsigned_abs();
    if dp == 0 {
        return whole.to_string();
    }
    format!("{whole}.{fraction:0width$}", width = dp as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(year: i32, month: u32, of: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, of).expect("a real date")
    }

    #[test]
    fn a_proportion_parses_at_six_places() {
        assert_eq!(TaxRate::parse("0.18").unwrap().scaled(), 180_000);
        assert_eq!(TaxRate::parse("0.086250").unwrap().scaled(), 86_250);
        assert_eq!(TaxRate::parse("0").unwrap(), TaxRate::ZERO);
        assert_eq!(TaxRate::parse("1").unwrap().scaled(), 1_000_000);
    }

    #[test]
    fn a_seventh_decimal_place_is_refused_rather_than_rounded() {
        // A rate that changes on the way in is a rate nobody can reconcile a
        // filed return against.
        assert_eq!(
            TaxRate::parse("0.1234567"),
            Err(TaxRateError::NotANumber),
            "a seventh place must be refused",
        );
    }

    #[test]
    fn a_percentage_is_its_own_door() {
        assert_eq!(TaxRate::parse_percent("18").unwrap().scaled(), 180_000);
        assert_eq!(TaxRate::parse_percent("8.625").unwrap().scaled(), 86_250);
        assert_eq!(TaxRate::parse_percent("0").unwrap(), TaxRate::ZERO);
        // Trailing zeroes past the fourth place are what a published table
        // looks like, and they mean what they say.
        assert_eq!(TaxRate::parse_percent("8.62500").unwrap().scaled(), 86_250);
        // The smallest rate the column can tell from zero.
        assert_eq!(TaxRate::parse_percent("0.0001").unwrap().scaled(), 1);
    }

    #[test]
    fn the_two_doors_do_not_agree_and_that_is_the_point() {
        // "18" through one is eighteen percent; through the other it is
        // eighteen times the price. Both are legitimate; conflating them is a
        // factor of a hundred in the ledger.
        assert_ne!(
            TaxRate::parse("18").unwrap(),
            TaxRate::parse_percent("18").unwrap()
        );
    }

    #[test]
    fn zero_is_a_rate_but_a_negative_one_is_not() {
        // Zero-rated is not exempt, and the difference is recoverability.
        assert!(TaxRate::parse("0.000000").is_ok());
        assert_eq!(TaxRate::parse("-0.1"), Err(TaxRateError::Negative));
        assert_eq!(TaxRate::parse_percent("-1"), Err(TaxRateError::Negative));
    }

    #[test]
    fn a_rate_the_column_cannot_hold_is_refused() {
        assert_eq!(TaxRate::parse("1000"), Err(TaxRateError::OutOfRange));
        assert!(TaxRate::parse("999.999999").is_ok());
    }

    #[test]
    fn storage_digits_are_what_the_column_wants() {
        assert_eq!(
            TaxRate::parse("0.18").unwrap().to_storage_string(),
            "0.180000"
        );
        assert_eq!(TaxRate::ZERO.to_storage_string(), "0.000000");
    }

    #[test]
    fn a_percentage_is_printed_for_a_person_not_a_machine() {
        assert_eq!(TaxRate::parse("0.18").unwrap().to_percent_string(), "18%");
        assert_eq!(
            TaxRate::parse("0.086250").unwrap().to_percent_string(),
            "8.625%"
        );
        assert_eq!(TaxRate::ZERO.to_percent_string(), "0%");
    }

    #[test]
    fn a_rate_crosses_the_wire_as_its_digits() {
        // A JSON number is a double in most parsers, and a preview that
        // disagrees with the posting is the whole failure this avoids.
        let rate = TaxRate::parse("0.086250").unwrap();
        let json = serde_json::to_string(&rate).unwrap();
        assert_eq!(json, "\"0.086250\"");
        assert_eq!(serde_json::from_str::<TaxRate>(&json).unwrap(), rate);
    }

    #[test]
    fn a_period_is_half_open_so_the_changeover_day_belongs_to_one_row() {
        let period = TaxRatePeriod {
            rate: TaxRate::parse("0.20").unwrap(),
            valid_from: day(2024, 1, 1),
            valid_to: Some(day(2026, 4, 1)),
        };

        assert!(!period.covers(day(2023, 12, 31)));
        assert!(period.covers(day(2024, 1, 1)));
        assert!(period.covers(day(2026, 3, 31)));
        // The first day of the new rate is not the last day of the old one.
        assert!(!period.covers(day(2026, 4, 1)));
    }

    #[test]
    fn an_open_ended_period_covers_everything_after_it_opens() {
        let period = TaxRatePeriod {
            rate: TaxRate::parse("0.20").unwrap(),
            valid_from: day(2026, 4, 1),
            valid_to: None,
        };

        assert!(period.covers(day(2099, 1, 1)));
        assert!(!period.covers(day(2026, 3, 31)));
    }

    #[test]
    fn a_period_that_ends_before_it_starts_is_refused() {
        let backwards = TaxRatePeriod {
            rate: TaxRate::ZERO,
            valid_from: day(2026, 4, 1),
            valid_to: Some(day(2026, 1, 1)),
        };
        assert_eq!(backwards.check(), Err(TaxRateError::BackwardsPeriod));

        // Equal is also refused: a half-open window of zero length covers no
        // day at all, so it is a row that can never apply.
        let empty = TaxRatePeriod {
            valid_to: Some(day(2026, 4, 1)),
            ..backwards
        };
        assert_eq!(empty.check(), Err(TaxRateError::BackwardsPeriod));
    }

    #[test]
    fn what_a_screen_submits_becomes_a_window_or_says_why_not() {
        let input = TaxRateInput {
            percent: "18".to_owned(),
            valid_from: day(2026, 1, 1),
            valid_to: None,
        };
        assert_eq!(input.parse().unwrap().rate.scaled(), 180_000);

        let bad = TaxRateInput {
            percent: "eighteen".to_owned(),
            ..input
        };
        assert_eq!(bad.parse(), Err(TaxRateError::NotANumber));
    }
}
