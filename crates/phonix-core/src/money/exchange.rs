//! Rates, and the evidence a converted amount has to carry with it.

use core::fmt;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use super::amount::{Money, MoneyError, Rounding, decimal_string, pow10, round_div};
use crate::locale::Currency;
use crate::{Message, msg};

/// Decimal places a rate is held at, matching `NUMERIC(20, 10)`.
///
/// Ten, not four. A rate is a ratio rather than an amount: JPY to USD is around
/// 0.0067, and four decimal places would round that to 0.0067 exactly - a tenth
/// of a percent of error, applied to every yen invoice in the ledger.
pub const RATE_SCALE: u32 = 10;

/// The largest scaled rate `NUMERIC(20, 10)` can hold: 9999999999.9999999999.
const MAX_RATE_SCALED: i128 = 99_999_999_999_999_999_999;

/// Longest name a rate source may carry, matching the column's CHECK.
pub const MAX_SOURCE_LEN: usize = 40;

/// A conversion factor: how many units of the quote currency one unit of the
/// base currency buys.
///
/// Positive by construction. A zero rate would make every converted amount
/// zero, and a negative one has no meaning at all - both are far more likely to
/// be a bad import than a real quotation, so neither can be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rate {
    /// The rate multiplied by `10^RATE_SCALE`.
    scaled: i128,
}

impl Rate {
    /// One to one. What a currency converts to itself at, and nothing else.
    pub const ONE: Self = Self {
        scaled: 10_000_000_000,
    };

    /// Build from the scaled integer - the rate times `10^RATE_SCALE`.
    pub fn from_scaled(scaled: i128) -> Result<Self, RateError> {
        if scaled <= 0 {
            return Err(RateError::NotPositive);
        }
        if scaled > MAX_RATE_SCALED {
            return Err(RateError::OutOfRange);
        }
        Ok(Self { scaled })
    }

    /// Parse a plain decimal string: `1.0925`, `0.0066841`, `132`.
    ///
    /// Up to [`RATE_SCALE`] decimal places. An eleventh digit is refused rather
    /// than rounded, for the same reason an amount's fifth is: a feed whose
    /// numbers change on the way in is a feed nobody can reconcile against.
    pub fn parse(raw: &str) -> Result<Self, RateError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(RateError::NotANumber);
        }

        // Caught here rather than falling through to `NotANumber`: a negative
        // rate is a well-formed number that is not a rate, and saying so names
        // what is wrong with the feed that produced it.
        if raw.starts_with('-') {
            return Err(RateError::NotPositive);
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
            return Err(RateError::NotANumber);
        }

        let mut scaled: i128 = 0;
        for byte in whole.bytes().chain(fraction.bytes()) {
            let digit = i128::from(byte - b'0');
            scaled = scaled
                .checked_mul(10)
                .and_then(|shifted| shifted.checked_add(digit))
                .ok_or(RateError::OutOfRange)?;
        }

        let missing = RATE_SCALE - fraction.len() as u32;
        scaled = scaled
            .checked_mul(pow10(missing).ok_or(RateError::OutOfRange)?)
            .ok_or(RateError::OutOfRange)?;

        Self::from_scaled(scaled)
    }

    /// The rate times `10^RATE_SCALE`. What goes in the column.
    pub const fn scaled(self) -> i128 {
        self.scaled
    }

    /// The digits at full storage scale - what a `NUMERIC(20, 10)` bind wants.
    pub fn to_storage_string(self) -> String {
        decimal_string(self.scaled, RATE_SCALE, RATE_SCALE)
    }
}

/// Crosses the wire as the string `"1.0925000000"`, for the reason
/// [`Money`] does: a JSON number is a double in most parsers, and ten decimal
/// places do not survive one.
impl Serialize for Rate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_storage_string())
    }
}

impl<'de> Deserialize<'de> for Rate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Trailing zeros trimmed, because ten of them on 1.0925 is noise on a
        // screen. The storage form keeps them; this one is for reading.
        let full = self.to_storage_string();
        let trimmed = full.trim_end_matches('0').trim_end_matches('.');
        f.write_str(if trimmed.is_empty() { "0" } else { trimmed })
    }
}

/// A rate as published: which pair, how much, on what day, by whom.
///
/// The date and the source are not decoration. "Which published rate did you
/// use" is the question an auditor asks, and a bare number cannot answer it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeRate {
    pub base: Currency,
    pub quote: Currency,
    pub rate: Rate,
    /// The day the rate was published, not the day it was fetched.
    pub as_of: NaiveDate,
    /// Where it came from: a central bank, a provider, or `manual`.
    pub source: String,
}

impl ExchangeRate {
    pub fn new(
        base: Currency,
        quote: Currency,
        rate: Rate,
        as_of: NaiveDate,
        source: impl Into<String>,
    ) -> Result<Self, RateError> {
        if base == quote {
            return Err(RateError::SamePair);
        }

        let source = source.into();
        // Checked here as well as by the column, so an import naming a source
        // nothing will accept fails where the caller still has the record in
        // hand rather than several layers down as a constraint violation.
        if source.is_empty() || source.chars().count() > MAX_SOURCE_LEN {
            return Err(RateError::SourceShape);
        }

        Ok(Self {
            base,
            quote,
            rate,
            as_of,
            source,
        })
    }
}

/// An amount, converted, with everything needed to prove how.
///
/// A foreign-currency document stores all six of these fields together. The
/// alternative - keeping the foreign amount and recomputing the base amount
/// when it is next needed - reprices history every time the rate moves, which
/// is the most common reason a foreign-currency ledger stops reconciling and
/// the hardest to notice, because nothing errors.
///
/// There is no constructor that takes the base amount. It can only be produced
/// by [`Money::convert`], so a snapshot with a base amount that does not follow
/// from its own rate cannot be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversion {
    /// The amount as transacted, in the currency it was transacted in.
    pub amount: Money,
    /// The same amount in the organization's own currency.
    pub base_amount: Money,
    /// The rate applied - quote per unit of base, as stored.
    pub rate: Rate,
    /// The day that rate was published.
    pub rate_date: NaiveDate,
    /// How the base amount was rounded, so it can be reproduced exactly.
    pub rounding: Rounding,
}

impl Conversion {
    pub const fn currency(&self) -> Currency {
        self.amount.currency()
    }

    pub const fn base_currency(&self) -> Currency {
        self.base_amount.currency()
    }
}

impl Money {
    /// Convert into the base currency using a published rate.
    ///
    /// `rate.base` must be this amount's currency: the rate says how many units
    /// of `quote` one unit of `base` buys, and applying it the wrong way round
    /// is an error that produces a plausible number, which is the worst kind.
    /// Inverting a rate is a decision with a spread in it, so it is refused
    /// here rather than guessed at - fetch the pair you actually need.
    ///
    /// The result is rounded to the base currency's minor unit, once, and the
    /// mode is recorded in the snapshot.
    pub fn convert(self, rate: &ExchangeRate, rounding: Rounding) -> Result<Conversion, RateError> {
        if rate.base != self.currency() {
            return Err(RateError::WrongPair {
                expected: self.currency(),
                found: rate.base,
            });
        }

        // scale 4 times scale 10 is scale 14; round back down to 4 in one step,
        // so there is exactly one rounding between the amount and the answer.
        let product = self
            .scaled()
            .checked_mul(rate.rate.scaled())
            .ok_or(RateError::OutOfRange)?;
        let divisor = pow10(RATE_SCALE).ok_or(RateError::OutOfRange)?;
        let converted = round_div(product, divisor, rounding).ok_or(RateError::OutOfRange)?;

        let base_amount = Money::from_scaled(rate.quote, converted)
            .and_then(|money| money.round_to_minor_unit(rounding))
            .map_err(RateError::Amount)?;

        Ok(Conversion {
            amount: self,
            base_amount,
            rate: rate.rate,
            rate_date: rate.as_of,
            rounding,
        })
    }
}

/// What can go wrong with a rate or a conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RateError {
    #[error("a rate must be greater than zero")]
    NotPositive,
    #[error("rate is outside the range NUMERIC(20, 10) can hold")]
    OutOfRange,
    #[error("not a decimal number with at most 10 decimal places")]
    NotANumber,
    #[error("a currency does not have a rate against itself")]
    SamePair,
    #[error("a rate source must be named, in at most 40 characters")]
    SourceShape,
    #[error("this rate converts from {found}, not from {expected}")]
    WrongPair { expected: Currency, found: Currency },
    #[error(transparent)]
    Amount(#[from] MoneyError),
}

impl RateError {
    /// What to say to whoever typed it, or to whoever is looking at the
    /// document that could not be converted.
    ///
    /// Here for the reason [`MoneyError::message`] is: these reach a rates
    /// screen and an invoice, and a form cannot render a `Display` string it
    /// has no catalog for. `WrongPair` in particular is worth naming - an
    /// inverted rate produces a *plausible* figure, which is the kind nobody
    /// catches.
    pub fn message(self) -> Message {
        match self {
            Self::NotPositive => msg!("money.error.rate_not_positive"),
            Self::OutOfRange => msg!("money.error.rate_out_of_range"),
            Self::NotANumber => msg!("money.error.rate_not_a_number"),
            Self::SamePair => msg!("money.error.rate_same_pair"),
            Self::SourceShape => msg!("money.error.rate_source"),
            Self::WrongPair { expected, found } => msg!(
                "money.error.rate_wrong_pair",
                expected = expected.code(),
                found = found.code(),
            ),
            Self::Amount(inner) => inner.message(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn currency(code: &str) -> Currency {
        Currency::parse(code).unwrap()
    }

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, 24).unwrap()
    }

    fn pair(base: &str, quote: &str, rate: &str) -> ExchangeRate {
        ExchangeRate::new(
            currency(base),
            currency(quote),
            Rate::parse(rate).unwrap(),
            day(),
            "ecb",
        )
        .unwrap()
    }

    #[test]
    fn parses_the_precision_a_rate_actually_needs() {
        assert_eq!(Rate::parse("1").unwrap(), Rate::ONE);
        assert_eq!(Rate::parse("1.0").unwrap(), Rate::ONE);
        assert_eq!(Rate::parse("0.0066841").unwrap().scaled(), 66_841_000);
        assert_eq!(Rate::parse("0.0000000001").unwrap().scaled(), 1);
    }

    #[test]
    fn refuses_a_rate_that_is_not_a_rate() {
        assert_eq!(Rate::parse("0"), Err(RateError::NotPositive));
        assert_eq!(Rate::parse("-1"), Err(RateError::NotPositive));
        assert_eq!(Rate::parse("0.00000000001"), Err(RateError::NotANumber));
        for bad in ["", ".", "1.", ".5", "abc", "1e-3"] {
            assert_eq!(Rate::parse(bad), Err(RateError::NotANumber), "{bad:?}");
        }
    }

    #[test]
    fn a_rate_has_to_say_where_it_came_from() {
        let long = "x".repeat(MAX_SOURCE_LEN + 1);
        for bad in ["", long.as_str()] {
            assert_eq!(
                ExchangeRate::new(Currency::USD, currency("EUR"), Rate::ONE, day(), bad,),
                Err(RateError::SourceShape),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn a_currency_has_no_rate_against_itself() {
        let same = ExchangeRate::new(Currency::USD, Currency::USD, Rate::ONE, day(), "manual");
        assert_eq!(same, Err(RateError::SamePair));
    }

    #[test]
    fn display_trims_the_zeros_but_storage_keeps_them() {
        let rate = Rate::parse("1.0925").unwrap();
        assert_eq!(rate.to_string(), "1.0925");
        assert_eq!(rate.to_storage_string(), "1.0925000000");
        assert_eq!(Rate::ONE.to_string(), "1");
        assert_eq!(Rate::ONE.to_storage_string(), "1.0000000000");
    }

    #[test]
    fn a_conversion_keeps_both_sides_and_the_evidence() {
        let amount = Money::parse(currency("EUR"), "100.00").unwrap();
        let converted = amount
            .convert(&pair("EUR", "USD", "1.0925"), Rounding::HalfUp)
            .unwrap();

        assert_eq!(converted.amount, amount);
        assert_eq!(converted.base_amount.to_display_string(), "109.25");
        assert_eq!(converted.base_currency(), currency("USD"));
        assert_eq!(converted.rate_date, day());
        assert_eq!(converted.rate.to_string(), "1.0925");
    }

    #[test]
    fn a_rate_pointing_the_wrong_way_is_refused_not_inverted() {
        // The number an inverted rate produces is plausible, which is exactly
        // why guessing here would be worse than failing.
        let amount = Money::parse(currency("USD"), "100.00").unwrap();
        assert_eq!(
            amount.convert(&pair("EUR", "USD", "1.0925"), Rounding::HalfUp),
            Err(RateError::WrongPair {
                expected: currency("USD"),
                found: currency("EUR"),
            })
        );
    }

    #[test]
    fn converting_into_a_zero_decimal_currency_lands_whole() {
        let amount = Money::parse(currency("USD"), "100.00").unwrap();
        let converted = amount
            .convert(&pair("USD", "JPY", "147.3821"), Rounding::HalfUp)
            .unwrap();
        assert_eq!(converted.base_amount.to_display_string(), "14738");
        assert_eq!(converted.base_amount.to_storage_string(), "14738.0000");
    }

    #[test]
    fn a_small_rate_keeps_its_precision_through_the_multiply() {
        // 1,000,000 yen at 0.0066841 is 6684.10 dollars. Held at four decimal
        // places the rate would be 0.0067, giving 6700.00 - sixteen dollars of
        // error on one invoice.
        let amount = Money::parse(currency("JPY"), "1000000").unwrap();
        let converted = amount
            .convert(&pair("JPY", "USD", "0.0066841"), Rounding::HalfUp)
            .unwrap();
        assert_eq!(converted.base_amount.to_display_string(), "6684.10");
    }

    #[test]
    fn the_rounding_mode_travels_with_the_answer() {
        let amount = Money::parse(currency("EUR"), "1.00").unwrap();
        let rate = pair("EUR", "USD", "1.005");

        let up = amount.convert(&rate, Rounding::HalfUp).unwrap();
        let even = amount.convert(&rate, Rounding::HalfEven).unwrap();

        assert_eq!(up.base_amount.to_display_string(), "1.01");
        assert_eq!(even.base_amount.to_display_string(), "1.00");
        assert_eq!(up.rounding, Rounding::HalfUp);
        assert_eq!(even.rounding, Rounding::HalfEven);
    }

    #[test]
    fn a_conversion_round_trips_through_json() {
        let amount = Money::parse(currency("EUR"), "100.00").unwrap();
        let converted = amount
            .convert(&pair("EUR", "USD", "1.0925"), Rounding::HalfUp)
            .unwrap();

        let json = serde_json::to_string(&converted).unwrap();
        assert_eq!(
            serde_json::from_str::<Conversion>(&json).unwrap(),
            converted
        );
    }
}
