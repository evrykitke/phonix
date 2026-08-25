//! How many.
//!
//! # Four decimal places, and why it is not an integer
//!
//! Because half a metre of cable, 0.25 hours of work and 1.5 kilograms of
//! anything are ordinary things to sell. An integer quantity would push the
//! fraction into the unit price, where it becomes somebody else's rounding.
//!
//! Four places matches `NUMERIC(19, 4)` and matches [`Money`]'s own scale,
//! which is what makes `unit_price × quantity` a ratio of two integers with a
//! single rounding rather than a chain of them.
//!
//! # Negative is allowed
//!
//! A returned item on a credit note is a negative quantity, and a discount line
//! is a negative amount. Refusing either would mean modelling a credit note as
//! a different document with the same fields, which is how two code paths end
//! up disagreeing about tax.
//!
//! # Why this is not in `phonix-core` yet
//!
//! Because only one app needs it. `core` takes a thing when the third app wants
//! it - two is a coincidence - and moving it later is a re-export, which is
//! cheap. Putting it in `core` now would be guessing at what Procurement means
//! by a quantity before Procurement exists.

use std::fmt;

use phonix_core::Message;
use phonix_core::money::{Money, MoneyError, Rounding};
use phonix_core::msg;
use serde::{Deserialize, Serialize};

/// Decimal places a quantity is held and stored at.
pub const QUANTITY_SCALE: u32 = 4;

/// `10^QUANTITY_SCALE`.
pub const QUANTITY_ONE: i128 = 10_000;

/// The largest magnitude `NUMERIC(19, 4)` holds.
pub const MAX_QUANTITY_SCALED: i128 = 999_999_999_999_999_999;

/// How many, at four decimal places.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Quantity {
    scaled: i128,
}

impl Quantity {
    pub const ZERO: Self = Self { scaled: 0 };
    pub const ONE: Self = Self {
        scaled: QUANTITY_ONE,
    };

    /// Build from the scaled integer - the quantity times `10^QUANTITY_SCALE`.
    pub fn from_scaled(scaled: i128) -> Result<Self, QuantityError> {
        if scaled.unsigned_abs() > MAX_QUANTITY_SCALED.unsigned_abs() {
            return Err(QuantityError::OutOfRange);
        }
        Ok(Self { scaled })
    }

    /// Whole units.
    pub fn from_units(units: i64) -> Result<Self, QuantityError> {
        i128::from(units)
            .checked_mul(QUANTITY_ONE)
            .ok_or(QuantityError::OutOfRange)
            .and_then(Self::from_scaled)
    }

    /// Parse what somebody typed: `1`, `2.5`, `-1`, `0.125`.
    ///
    /// A fifth decimal place is refused rather than rounded, for the reason an
    /// amount's fifth is: a quantity that changes on the way in is a line whose
    /// total nobody can check by hand.
    pub fn parse(raw: &str) -> Result<Self, QuantityError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(QuantityError::NotANumber);
        }

        let (negative, digits) = match raw.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, raw.strip_prefix('+').unwrap_or(raw)),
        };

        let (whole, fraction) = match digits.split_once('.') {
            Some((whole, fraction)) => (whole, fraction),
            None => (digits, ""),
        };

        if whole.is_empty()
            || fraction.len() > QUANTITY_SCALE as usize
            || (digits.contains('.') && fraction.is_empty())
            || !whole.bytes().all(|byte| byte.is_ascii_digit())
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(QuantityError::NotANumber);
        }

        let mut scaled: i128 = 0;
        for byte in whole.bytes().chain(fraction.bytes()) {
            let digit = i128::from(byte - b'0');
            scaled = scaled
                .checked_mul(10)
                .and_then(|shifted| shifted.checked_add(digit))
                .ok_or(QuantityError::OutOfRange)?;
        }

        let missing = QUANTITY_SCALE - fraction.len() as u32;
        scaled = scaled
            .checked_mul(pow10(missing).ok_or(QuantityError::OutOfRange)?)
            .ok_or(QuantityError::OutOfRange)?;

        Self::from_scaled(if negative { -scaled } else { scaled })
    }

    /// The quantity times `10^QUANTITY_SCALE`. What goes in the column.
    pub const fn scaled(self) -> i128 {
        self.scaled
    }

    pub const fn is_zero(self) -> bool {
        self.scaled == 0
    }

    pub const fn is_negative(self) -> bool {
        self.scaled < 0
    }

    /// The digits at full storage scale - what a `NUMERIC(19, 4)` bind wants.
    pub fn to_storage_string(self) -> String {
        decimal_string(self.scaled, QUANTITY_SCALE, QUANTITY_SCALE)
    }

    /// What a line shows: trailing zeroes trimmed, because `2.0000` is a
    /// quantity written by a machine and this is read by a person.
    pub fn to_display_string(self) -> String {
        let text = decimal_string(self.scaled, QUANTITY_SCALE, QUANTITY_SCALE);
        match text.split_once('.') {
            Some((whole, fraction)) => {
                let fraction = fraction.trim_end_matches('0');
                if fraction.is_empty() {
                    whole.to_owned()
                } else {
                    format!("{whole}.{fraction}")
                }
            }
            None => text,
        }
    }

    /// `unit_price × this`, rounded **once**.
    ///
    /// The only multiplication on a line, and it goes through
    /// [`Money::scale_by`] rather than through a float: a quantity is a ratio
    /// of two integers, and so is the answer.
    ///
    /// The result keeps the storage scale and is *not* rounded to the
    /// currency's minor unit - that happens once, later, where the figure
    /// becomes money somebody pays.
    pub fn times(self, unit_price: Money, rounding: Rounding) -> Result<Money, MoneyError> {
        unit_price.scale_by(self.scaled, QUANTITY_ONE, rounding)
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_display_string())
    }
}

/// Crosses the wire as the string `"2.5000"`, for the reason every amount
/// does: a JSON number is an IEEE double in most parsers, and a quantity that
/// changes in transit is a line total the browser and the server disagree on.
impl Serialize for Quantity {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_storage_string())
    }
}

impl<'de> Deserialize<'de> for Quantity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// What can be wrong with a quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QuantityError {
    #[error("not a number with at most 4 decimal places")]
    NotANumber,
    #[error("that quantity is outside the range NUMERIC(19, 4) can hold")]
    OutOfRange,
}

impl QuantityError {
    pub fn message(self) -> Message {
        match self {
            Self::NotANumber => msg!("books.error.quantity_not_a_number"),
            Self::OutOfRange => msg!("books.error.quantity_out_of_range"),
        }
    }
}

/// `10^n`, or `None` if it does not fit.
///
/// A loop rather than `i128::pow`, which panics on overflow - and this crate
/// does not panic, because a panic in the wasm bundle freezes the tab.
fn pow10(n: u32) -> Option<i128> {
    let mut value: i128 = 1;
    for _ in 0..n {
        value = value.checked_mul(10)?;
    }
    Some(value)
}

/// Render a scaled integer as a decimal string with `dp` places.
fn decimal_string(scaled: i128, scale: u32, dp: u32) -> String {
    let Some(factor) = pow10(scale) else {
        return scaled.to_string();
    };

    let negative = scaled < 0;
    let magnitude = scaled.unsigned_abs();
    let whole = magnitude / factor.unsigned_abs();
    let fraction = magnitude % factor.unsigned_abs();

    let sign = if negative { "-" } else { "" };
    if dp == 0 {
        return format!("{sign}{whole}");
    }
    format!("{sign}{whole}.{fraction:0width$}", width = dp as usize)
}

#[cfg(test)]
mod tests {
    use phonix_core::locale::Currency;

    use super::*;

    fn usd(amount: &str) -> Money {
        Money::parse(Currency::parse("USD").unwrap(), amount).unwrap()
    }

    #[test]
    fn a_quantity_parses_at_four_places() {
        assert_eq!(Quantity::parse("1").unwrap(), Quantity::ONE);
        assert_eq!(Quantity::parse("2.5").unwrap().scaled(), 25_000);
        assert_eq!(Quantity::parse("0.125").unwrap().scaled(), 1_250);
        assert_eq!(Quantity::parse("0").unwrap(), Quantity::ZERO);
    }

    #[test]
    fn a_fifth_decimal_place_is_refused_rather_than_rounded() {
        // A quantity that changes on the way in is a line total nobody can
        // check by hand.
        assert_eq!(
            Quantity::parse("1.00005"),
            Err(QuantityError::NotANumber),
            "a fifth place must be refused",
        );
    }

    #[test]
    fn negative_is_a_quantity() {
        // A returned item on a credit note. Refusing it would mean modelling a
        // credit note as a different document with the same fields.
        let returned = Quantity::parse("-2").unwrap();

        assert!(returned.is_negative());
        assert_eq!(returned.scaled(), -20_000);
    }

    #[test]
    fn what_is_not_a_number_is_refused() {
        for bad in ["", "  ", "two", "1.2.3", "1..2", "1-", "+"] {
            assert_eq!(
                Quantity::parse(bad),
                Err(QuantityError::NotANumber),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_line_total_is_one_multiplication_with_one_rounding() {
        // 3 at 19.99 is 59.97, exactly - not 59.969999999999999.
        let total = Quantity::parse("3")
            .unwrap()
            .times(usd("19.99"), Rounding::HalfUp)
            .unwrap();

        assert_eq!(total, usd("59.97"));
    }

    #[test]
    fn a_fractional_quantity_keeps_the_fraction_out_of_the_unit_price() {
        // Half an hour at 90.00 is 45.00. An integer quantity would have made
        // this 1 at 45.00, and the rate on the document would have been wrong.
        let total = Quantity::parse("0.5")
            .unwrap()
            .times(usd("90.00"), Rounding::HalfUp)
            .unwrap();

        assert_eq!(total, usd("45.00"));
    }

    #[test]
    fn a_quantity_past_four_places_of_product_rounds_once_and_not_twice() {
        // 0.3333 at 10.00 is 3.333, which the storage scale holds exactly.
        let total = Quantity::parse("0.3333")
            .unwrap()
            .times(usd("10.00"), Rounding::HalfUp)
            .unwrap();

        assert_eq!(total, usd("3.3330"));
    }

    #[test]
    fn a_negative_quantity_gives_a_negative_line() {
        let total = Quantity::parse("-2")
            .unwrap()
            .times(usd("19.99"), Rounding::HalfUp)
            .unwrap();

        assert_eq!(total, usd("-39.98"));
    }

    #[test]
    fn storage_digits_are_what_the_column_wants_and_display_is_for_a_person() {
        let two = Quantity::parse("2").unwrap();
        assert_eq!(two.to_storage_string(), "2.0000");
        assert_eq!(two.to_display_string(), "2");

        let half = Quantity::parse("2.5").unwrap();
        assert_eq!(half.to_storage_string(), "2.5000");
        assert_eq!(half.to_display_string(), "2.5");

        let owed = Quantity::parse("-1.25").unwrap();
        assert_eq!(owed.to_storage_string(), "-1.2500");
        assert_eq!(owed.to_display_string(), "-1.25");
    }

    #[test]
    fn a_quantity_crosses_the_wire_as_its_digits() {
        let quantity = Quantity::parse("2.5").unwrap();
        let json = serde_json::to_string(&quantity).unwrap();

        assert_eq!(json, "\"2.5000\"");
        assert_eq!(serde_json::from_str::<Quantity>(&json).unwrap(), quantity);
    }
}
