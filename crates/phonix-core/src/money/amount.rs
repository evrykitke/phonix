//! [`Money`]: an exact amount in a known currency.

use core::cmp::Ordering;
use core::fmt;

use serde::{Deserialize, Serialize};

use crate::i18n::Message;
use crate::locale::Currency;
use crate::msg;

/// The largest scaled value `NUMERIC(19, 4)` can hold: 999999999999999.9999.
///
/// Written as the scaled integer because that is the form [`Money`] keeps, and
/// the form the range check has to compare against. Note that it exceeds
/// [`i64::MAX`], which is why the amount is an `i128`: the column is a digit
/// wider than a 64-bit integer.
pub const MAX_SCALED: i128 = 9_999_999_999_999_999_999;

/// An exact amount of one currency.
///
/// `Copy`, because it is five words and passing it by reference would read
/// worse everywhere for no gain. Deliberately **not** `Ord`, `Add` or `Sum`:
/// every one of those would have to answer what happens when the currencies
/// differ, and the only honest answer is a `Result`. See [`Money::checked_add`]
/// and [`Money::compare`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Money {
    /// The amount multiplied by `10^SCALE`.
    scaled: i128,
    currency: Currency,
}

impl Money {
    /// Decimal places every amount is stored at, whatever the currency.
    ///
    /// Four, matching `NUMERIC(19, 4)`. Not the currency's minor units: a unit
    /// price of 0.0125 has to survive being written down, and rounding it where
    /// it is entered is how a large order comes out wrong by much more than the
    /// rounding.
    pub const SCALE: u32 = 4;

    /// Nothing, in a currency. The identity for a running total.
    pub const fn zero(currency: Currency) -> Self {
        Self {
            scaled: 0,
            currency,
        }
    }

    /// Build from the scaled integer - the amount times `10^SCALE`.
    ///
    /// This is the constructor the database layer uses, and the one to reach
    /// for when the value already exists as an exact integer. Out-of-range
    /// values are refused rather than saturated: an amount the column cannot
    /// hold is an amount that will fail on write, and failing here names the
    /// problem while the caller still knows what it was doing.
    pub fn from_scaled(currency: Currency, scaled: i128) -> Result<Self, MoneyError> {
        if !(-MAX_SCALED..=MAX_SCALED).contains(&scaled) {
            return Err(MoneyError::OutOfRange);
        }
        Ok(Self { scaled, currency })
    }

    /// Build from a whole number of currency units: `from_units(USD, 25)` is
    /// twenty-five dollars.
    pub fn from_units(currency: Currency, units: i64) -> Result<Self, MoneyError> {
        let scaled = i128::from(units)
            .checked_mul(SCALE_FACTOR)
            .ok_or(MoneyError::OutOfRange)?;
        Self::from_scaled(currency, scaled)
    }

    /// Parse a plain decimal string: `-1234.56`, `0`, `7.0125`.
    ///
    /// Strict on purpose. No group separators, no currency symbols, and **no
    /// more than [`SCALE`](Self::SCALE) decimal places** - a fifth digit is
    /// refused rather than rounded away, because a number that comes back
    /// different from the one that was sent is worse than an error message.
    /// Locale-specific input (a decimal comma, a thousands space) belongs to
    /// the input widget, which hands this the normalised form.
    pub fn parse(currency: Currency, raw: &str) -> Result<Self, MoneyError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(MoneyError::NotANumber);
        }

        let (negative, digits) = match raw.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, raw.strip_prefix('+').unwrap_or(raw)),
        };

        let (whole, fraction) = match digits.split_once('.') {
            Some((whole, fraction)) => (whole, fraction),
            None => (digits, ""),
        };

        // "", ".5" and "1." are all refusals. Each is somebody's half-typed
        // number, and guessing which half they meant is how a fat-fingered
        // field becomes a posted amount.
        if whole.is_empty()
            || fraction.len() > Self::SCALE as usize
            || (digits.contains('.') && fraction.is_empty())
            || !whole.bytes().all(|byte| byte.is_ascii_digit())
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(MoneyError::NotANumber);
        }

        let mut scaled: i128 = 0;
        for byte in whole.bytes().chain(fraction.bytes()) {
            let digit = i128::from(byte - b'0');
            scaled = scaled
                .checked_mul(10)
                .and_then(|shifted| shifted.checked_add(digit))
                .ok_or(MoneyError::OutOfRange)?;
        }

        // Pad the decimal places that were not typed. "12.5" is 125000 at
        // scale 4, not 125.
        let missing = Self::SCALE - fraction.len() as u32;
        scaled = scaled
            .checked_mul(pow10(missing).ok_or(MoneyError::OutOfRange)?)
            .ok_or(MoneyError::OutOfRange)?;

        Self::from_scaled(currency, if negative { -scaled } else { scaled })
    }

    /// The amount times `10^SCALE`. What goes in the column.
    pub const fn scaled(self) -> i128 {
        self.scaled
    }

    pub const fn currency(self) -> Currency {
        self.currency
    }

    pub const fn is_zero(self) -> bool {
        self.scaled == 0
    }

    pub const fn is_negative(self) -> bool {
        self.scaled < 0
    }

    /// The same amount with the sign removed.
    pub const fn abs(self) -> Self {
        Self {
            scaled: self.scaled.abs(),
            currency: self.currency,
        }
    }

    /// The same amount, sign flipped. A credit note is a negated invoice.
    pub const fn negate(self) -> Self {
        Self {
            scaled: -self.scaled,
            currency: self.currency,
        }
    }

    pub fn checked_add(self, other: Self) -> Result<Self, MoneyError> {
        self.assert_same_currency(other)?;
        let scaled = self
            .scaled
            .checked_add(other.scaled)
            .ok_or(MoneyError::OutOfRange)?;
        Self::from_scaled(self.currency, scaled)
    }

    pub fn checked_sub(self, other: Self) -> Result<Self, MoneyError> {
        self.assert_same_currency(other)?;
        let scaled = self
            .scaled
            .checked_sub(other.scaled)
            .ok_or(MoneyError::OutOfRange)?;
        Self::from_scaled(self.currency, scaled)
    }

    /// Multiply by a whole number - a line quantity, most often.
    pub fn checked_mul_int(self, factor: i64) -> Result<Self, MoneyError> {
        let scaled = self
            .scaled
            .checked_mul(i128::from(factor))
            .ok_or(MoneyError::OutOfRange)?;
        Self::from_scaled(self.currency, scaled)
    }

    /// Multiply by the ratio `numerator / denominator`, rounding **once**.
    ///
    /// The primitive behind applying a tax rate, a discount percentage or a
    /// unit price with fractional digits. Both operands are integers at a scale
    /// the caller chose - 18% is `180_000 / 1_000_000` at scale 6 - because a
    /// ratio of two integers is exact where a decimal factor is already a
    /// rounding.
    ///
    /// The point is the *once*. The product is formed at the combined scale and
    /// divided back down in a single step, so an amount multiplied by a rate
    /// has one rounding between the input and the answer rather than one per
    /// operation. [`convert`](Self::convert) does the same thing for a rate,
    /// and for the same reason.
    ///
    /// The result keeps the storage scale, and is *not* rounded to the
    /// currency's minor unit: that is a separate, later act - see
    /// [`round_to_minor_unit`](Self::round_to_minor_unit).
    pub fn scale_by(
        self,
        numerator: i128,
        denominator: i128,
        mode: Rounding,
    ) -> Result<Self, MoneyError> {
        // A denominator of zero or less needs no check of its own: `round_div`
        // answers `None` for one, which is the same `OutOfRange` as an
        // overflow. Both are a caller bug rather than anything a person typed.
        let product = self
            .scaled
            .checked_mul(numerator)
            .ok_or(MoneyError::OutOfRange)?;
        let scaled = round_div(product, denominator, mode).ok_or(MoneyError::OutOfRange)?;
        Self::from_scaled(self.currency, scaled)
    }

    /// Sum a sequence, refusing a currency that does not belong.
    ///
    /// Takes the currency explicitly so that an empty sequence still has an
    /// answer. A total of nothing is zero of a known currency; taking the
    /// currency from the first element means an empty basket has none at all.
    pub fn total(
        currency: Currency,
        amounts: impl IntoIterator<Item = Self>,
    ) -> Result<Self, MoneyError> {
        amounts
            .into_iter()
            .try_fold(Self::zero(currency), |running, next| {
                running.checked_add(next)
            })
    }

    /// Compare two amounts of the same currency.
    ///
    /// Not `Ord`, because ordering dollars against yen is not a question with
    /// an answer, and a trait implementation would have to invent one.
    pub fn compare(self, other: Self) -> Result<Ordering, MoneyError> {
        self.assert_same_currency(other)?;
        Ok(self.scaled.cmp(&other.scaled))
    }

    /// Round to the currency's own minor unit, keeping the storage scale.
    ///
    /// **The only place money is rounded.** A yen amount comes back whole, a
    /// dollar amount to the cent, a dinar amount to the thousandth - and all
    /// three still at scale 4, so the result is still what the column holds.
    ///
    /// Do this once, where an amount becomes a figure somebody is charged.
    /// Rounding intermediate values is how a total stops matching the sum of
    /// its own lines.
    pub fn round_to_minor_unit(self, mode: Rounding) -> Result<Self, MoneyError> {
        self.round_to(u32::from(self.currency.minor_units()), mode)
    }

    /// Round to an arbitrary number of decimal places, keeping the storage
    /// scale. A `dp` at or beyond [`SCALE`](Self::SCALE) is a no-op, because
    /// there is nothing out there to round.
    pub fn round_to(self, dp: u32, mode: Rounding) -> Result<Self, MoneyError> {
        if dp >= Self::SCALE {
            return Ok(self);
        }
        let factor = pow10(Self::SCALE - dp).ok_or(MoneyError::OutOfRange)?;
        let rounded = round_div(self.scaled, factor, mode)
            .and_then(|units| units.checked_mul(factor))
            .ok_or(MoneyError::OutOfRange)?;
        Self::from_scaled(self.currency, rounded)
    }

    /// Divide into `parts` amounts that add back up to exactly this one.
    ///
    /// Ten dollars three ways is 3.34, 3.33, 3.33 - never three of 3.3333,
    /// which is a third of a cent nobody can pay and a penny missing from the
    /// ledger. The remainder goes to the earliest parts: arbitrary, but
    /// *stated*, and stated beats surprising.
    pub fn split(self, parts: usize) -> Result<Vec<Self>, MoneyError> {
        self.allocate(&vec![1; parts])
    }

    /// Divide in proportion to `weights`, so the parts add back up to exactly
    /// this amount.
    ///
    /// This is how a document-level discount reaches its lines, how freight is
    /// apportioned, and how a tax computed on a total is pushed back down.
    /// Largest-remainder: everyone gets their truncated share, then the units
    /// left over go one each to the largest remainders. The total is preserved
    /// by construction rather than by a correcting entry on the last line.
    ///
    /// Rounds to the currency's minor unit, since splitting into fractions of a
    /// cent would defeat the purpose. Weights must be non-negative, and not all
    /// zero.
    pub fn allocate(&self, weights: &[i64]) -> Result<Vec<Self>, MoneyError> {
        if weights.is_empty() {
            return Err(MoneyError::NothingToAllocateTo);
        }
        if weights.iter().any(|weight| *weight < 0) {
            return Err(MoneyError::NegativeWeight);
        }

        let total_weight: i128 = weights
            .iter()
            .try_fold(0i128, |sum, weight| sum.checked_add(i128::from(*weight)))
            .ok_or(MoneyError::OutOfRange)?;
        if total_weight == 0 {
            return Err(MoneyError::NothingToAllocateTo);
        }

        let dp = u32::from(self.currency.minor_units()).min(Self::SCALE);
        let factor = pow10(Self::SCALE - dp).ok_or(MoneyError::OutOfRange)?;

        // Allocate over whole minor units and scale back up at the end.
        // Working in the unit somebody can actually pay is what makes the parts
        // add up to the whole.
        let total_units =
            round_div(self.scaled, factor, Rounding::HalfUp).ok_or(MoneyError::OutOfRange)?;
        let negative = total_units < 0;
        let magnitude = total_units.checked_abs().ok_or(MoneyError::OutOfRange)?;

        // (truncated share, remainder, original position)
        let mut shares: Vec<(i128, i128, usize)> = Vec::with_capacity(weights.len());
        let mut distributed: i128 = 0;
        for (position, weight) in weights.iter().enumerate() {
            let numerator = magnitude
                .checked_mul(i128::from(*weight))
                .ok_or(MoneyError::OutOfRange)?;
            let share = numerator / total_weight;
            shares.push((share, numerator % total_weight, position));
            distributed = distributed
                .checked_add(share)
                .ok_or(MoneyError::OutOfRange)?;
        }

        // Largest remainder first, ties by original position, so the result is
        // deterministic rather than merely correct.
        let mut leftover = magnitude - distributed;
        shares.sort_by(|left, right| right.1.cmp(&left.1).then(left.2.cmp(&right.2)));
        for share in shares.iter_mut() {
            if leftover == 0 {
                break;
            }
            share.0 += 1;
            leftover -= 1;
        }

        shares.sort_by_key(|share| share.2);
        shares
            .into_iter()
            .map(|(units, _, _)| {
                let signed = if negative { -units } else { units };
                signed
                    .checked_mul(factor)
                    .ok_or(MoneyError::OutOfRange)
                    .and_then(|scaled| Self::from_scaled(self.currency, scaled))
            })
            .collect()
    }

    /// The digits, at the currency's own minor unit. No symbol, no grouping, no
    /// sign except a leading `-`.
    ///
    /// Deliberately not localised: `1.234,56` in Berlin and `1 234,56` in Paris
    /// are the same amount, and choosing between them needs a reader, which
    /// this type does not have. See [`crate::locale::Currency`] for why that
    /// belongs to whatever is doing the rendering.
    pub fn to_display_string(self) -> String {
        decimal_string(
            self.scaled,
            Self::SCALE,
            u32::from(self.currency.minor_units()),
        )
    }

    /// The digits at full storage scale - what a `NUMERIC(19, 4)` bind wants.
    ///
    /// Text rather than a numeric type because the driver has no lossless
    /// integer binding for `NUMERIC`, and text is exact in both directions.
    pub fn to_storage_string(self) -> String {
        decimal_string(self.scaled, Self::SCALE, Self::SCALE)
    }

    fn assert_same_currency(self, other: Self) -> Result<(), MoneyError> {
        if self.currency == other.currency {
            Ok(())
        } else {
            Err(MoneyError::CurrencyMismatch {
                expected: self.currency,
                found: other.currency,
            })
        }
    }
}

/// `1234.56 USD`. The currency is part of the value, so it is part of the text.
impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.to_display_string(), self.currency)
    }
}

/// Crosses the wire as `{"amount": "1234.5600", "currency": "USD"}`.
///
/// The amount is a **string**, not a JSON number. JSON numbers are IEEE doubles
/// in most parsers, and handing an exact decimal to a double is the very thing
/// this type exists to prevent - it would be undone silently, in transit,
/// between a browser that got it right and a server that got it right.
#[derive(Serialize, Deserialize)]
struct Wire {
    amount: String,
    currency: Currency,
}

impl Serialize for Money {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Wire {
            amount: self.to_storage_string(),
            currency: self.currency,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Money {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = Wire::deserialize(deserializer)?;
        Self::parse(wire.currency, &wire.amount).map_err(serde::de::Error::custom)
    }
}

/// How to resolve a value sitting exactly on a rounding boundary.
///
/// A document stores which one it used. Leaving it implicit is where
/// reconciliation disputes come from: two systems agreeing on every rate and
/// every line, disagreeing by a cent, and neither able to say why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rounding {
    /// Half away from zero: 2.5 becomes 3, -2.5 becomes -3.
    ///
    /// What commercial invoicing means by "round up", and what most tax
    /// authorities specify. The default, because it is what a person checking
    /// the arithmetic by hand will do.
    #[default]
    HalfUp,
    /// Half to even: 2.5 becomes 2, 3.5 becomes 4.
    ///
    /// Banker's rounding. Unbiased across many values, which is why financial
    /// reporting standards ask for it, and surprising on any single one.
    HalfEven,
}

/// What can go wrong with an amount.
///
/// Most of these are programmer errors rather than anything a person did, which
/// is why this is a plain error type and not [`crate::identity::FieldError`].
/// [`MoneyError::message`] exists for the one that does reach a form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MoneyError {
    #[error("cannot combine {expected} with {found}")]
    CurrencyMismatch { expected: Currency, found: Currency },
    #[error("amount is outside the range NUMERIC(19, 4) can hold")]
    OutOfRange,
    #[error("not a decimal number with at most 4 decimal places")]
    NotANumber,
    #[error("cannot allocate across no weights, or across weights that are all zero")]
    NothingToAllocateTo,
    #[error("allocation weights cannot be negative")]
    NegativeWeight,
}

impl MoneyError {
    /// What to say to whoever typed it.
    pub fn message(self) -> Message {
        match self {
            Self::CurrencyMismatch { .. } => msg!("money.error.currency_mismatch"),
            Self::OutOfRange => msg!("money.error.out_of_range"),
            Self::NotANumber => msg!("money.error.not_a_number"),
            Self::NothingToAllocateTo | Self::NegativeWeight => {
                msg!("money.error.cannot_allocate")
            }
        }
    }
}

/// `10^Money::SCALE`.
const SCALE_FACTOR: i128 = 10_000;

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

/// Divide, resolving the half case according to `mode`.
///
/// `divisor` must be positive. Rust truncates division toward zero and gives
/// the remainder the sign of the dividend, so the *magnitude* is what gets
/// compared against half the divisor and the increment goes in whichever
/// direction the value was already heading - which is what makes -2.5 round to
/// -3 under `HalfUp`.
pub(crate) fn round_div(value: i128, divisor: i128, mode: Rounding) -> Option<i128> {
    if divisor <= 0 {
        return None;
    }

    let quotient = value / divisor;
    let remainder = value % divisor;
    if remainder == 0 {
        return Some(quotient);
    }

    let twice = remainder.checked_abs()?.checked_mul(2)?;
    let step_up = match mode {
        Rounding::HalfUp => twice >= divisor,
        Rounding::HalfEven => twice > divisor || (twice == divisor && quotient % 2 != 0),
    };

    if !step_up {
        return Some(quotient);
    }
    if value < 0 {
        quotient.checked_sub(1)
    } else {
        quotient.checked_add(1)
    }
}

/// Render a scaled integer as a decimal string with `dp` places, rounding half
/// up if `dp` is narrower than the value's own scale.
pub(crate) fn decimal_string(scaled: i128, scale: u32, dp: u32) -> String {
    let dp = dp.min(scale);
    let value = if dp == scale {
        scaled
    } else {
        match pow10(scale - dp).and_then(|factor| round_div(scaled, factor, Rounding::HalfUp)) {
            Some(rounded) => rounded,
            // Unreachable for any value this module can build - the factor is
            // at most 10^4. Returning the unrounded digits still beats a panic
            // in a wasm bundle.
            None => scaled,
        }
    };

    let negative = value < 0;
    let magnitude = value.unsigned_abs();
    let divisor = pow10(dp).map(i128::unsigned_abs).unwrap_or(1);

    let whole = magnitude / divisor;
    let sign = if negative { "-" } else { "" };

    if dp == 0 {
        format!("{sign}{whole}")
    } else {
        let fraction = magnitude % divisor;
        format!("{sign}{whole}.{fraction:0width$}", width = dp as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usd(raw: &str) -> Money {
        Money::parse(Currency::USD, raw).unwrap()
    }

    fn jpy(raw: &str) -> Money {
        Money::parse(Currency::parse("JPY").unwrap(), raw).unwrap()
    }

    fn kwd(raw: &str) -> Money {
        Money::parse(Currency::parse("KWD").unwrap(), raw).unwrap()
    }

    #[test]
    fn the_classic_float_failure_does_not_happen_here() {
        // 0.1 + 0.2 is not 0.3 in binary floating point. It is here.
        let sum = usd("0.10").checked_add(usd("0.20")).unwrap();
        assert_eq!(sum, usd("0.30"));
        assert_eq!(sum.to_display_string(), "0.30");
    }

    #[test]
    fn parses_the_shapes_a_person_types() {
        assert_eq!(usd("0").scaled(), 0);
        assert_eq!(usd("1").scaled(), 10_000);
        assert_eq!(usd("12.5").scaled(), 125_000);
        assert_eq!(usd("12.50").scaled(), 125_000);
        assert_eq!(usd("7.0125").scaled(), 70_125);
        assert_eq!(usd("-1234.56").scaled(), -12_345_600);
        assert_eq!(usd("+3").scaled(), 30_000);
        assert_eq!(usd("  42.00  ").scaled(), 420_000);
    }

    #[test]
    fn refuses_a_fifth_decimal_place_rather_than_rounding_it_away() {
        // Silently dropping the digit would mean the value read back differs
        // from the value sent, which is worse than an error.
        assert_eq!(
            Money::parse(Currency::USD, "1.00001"),
            Err(MoneyError::NotANumber)
        );
    }

    #[test]
    fn refuses_half_typed_numbers() {
        for bad in [
            "", " ", ".", ".5", "1.", "1,000", "$5", "1e3", "abc", "--1", "1.2.3",
        ] {
            assert_eq!(
                Money::parse(Currency::USD, bad),
                Err(MoneyError::NotANumber),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn refuses_an_amount_the_column_cannot_hold() {
        assert!(Money::from_scaled(Currency::USD, MAX_SCALED).is_ok());
        assert!(Money::from_scaled(Currency::USD, -MAX_SCALED).is_ok());
        assert_eq!(
            Money::from_scaled(Currency::USD, MAX_SCALED + 1),
            Err(MoneyError::OutOfRange)
        );
        // Overflow in the sum, not just in the operand.
        let big = Money::from_scaled(Currency::USD, MAX_SCALED).unwrap();
        assert_eq!(big.checked_add(big), Err(MoneyError::OutOfRange));
    }

    #[test]
    fn adding_two_currencies_is_an_error_not_a_number() {
        let mismatch = usd("1.00").checked_add(jpy("100"));
        assert_eq!(
            mismatch,
            Err(MoneyError::CurrencyMismatch {
                expected: Currency::USD,
                found: Currency::parse("JPY").unwrap(),
            })
        );
    }

    #[test]
    fn an_empty_total_still_knows_its_currency() {
        let nothing = Money::total(Currency::USD, []).unwrap();
        assert!(nothing.is_zero());
        assert_eq!(nothing.currency(), Currency::USD);
    }

    #[test]
    fn minor_units_are_not_all_two() {
        // The reason `round_to_minor_unit` asks the currency rather than
        // assuming cents.
        assert_eq!(
            jpy("100.4999")
                .round_to_minor_unit(Rounding::HalfUp)
                .unwrap()
                .to_display_string(),
            "100"
        );
        assert_eq!(
            kwd("1.2345")
                .round_to_minor_unit(Rounding::HalfUp)
                .unwrap()
                .to_display_string(),
            "1.235"
        );
        assert_eq!(
            usd("1.2350")
                .round_to_minor_unit(Rounding::HalfUp)
                .unwrap()
                .to_display_string(),
            "1.24"
        );
    }

    #[test]
    fn half_up_goes_away_from_zero_in_both_directions() {
        assert_eq!(
            usd("2.005").round_to(2, Rounding::HalfUp).unwrap(),
            usd("2.01")
        );
        assert_eq!(
            usd("-2.005").round_to(2, Rounding::HalfUp).unwrap(),
            usd("-2.01")
        );
        assert_eq!(
            usd("2.004").round_to(2, Rounding::HalfUp).unwrap(),
            usd("2.00")
        );
    }

    #[test]
    fn half_even_breaks_the_tie_towards_the_even_digit() {
        assert_eq!(
            usd("2.005").round_to(2, Rounding::HalfEven).unwrap(),
            usd("2.00")
        );
        assert_eq!(
            usd("2.015").round_to(2, Rounding::HalfEven).unwrap(),
            usd("2.02")
        );
        assert_eq!(
            usd("-2.005").round_to(2, Rounding::HalfEven).unwrap(),
            usd("-2.00")
        );
        // Not a tie: HalfEven only differs on the exact half.
        assert_eq!(
            usd("2.006").round_to(2, Rounding::HalfEven).unwrap(),
            usd("2.01")
        );
    }

    #[test]
    fn rounding_beyond_the_storage_scale_changes_nothing() {
        assert_eq!(
            usd("1.2345").round_to(4, Rounding::HalfUp).unwrap(),
            usd("1.2345")
        );
        assert_eq!(
            usd("1.2345").round_to(9, Rounding::HalfUp).unwrap(),
            usd("1.2345")
        );
    }

    #[test]
    fn ten_dollars_three_ways_is_not_three_and_a_third() {
        let parts = usd("10.00").split(3).unwrap();
        let rendered: Vec<String> = parts.iter().map(|part| part.to_display_string()).collect();
        assert_eq!(rendered, ["3.34", "3.33", "3.33"]);
        assert_eq!(Money::total(Currency::USD, parts).unwrap(), usd("10.00"));
    }

    #[test]
    fn an_allocation_always_adds_back_up_to_the_whole() {
        let cases: &[(&str, &[i64])] = &[
            ("100.00", &[1, 1, 1]),
            ("0.05", &[1, 1, 1, 1, 1, 1, 1]),
            ("-10.00", &[1, 1, 1]),
            ("999.99", &[7, 11, 13]),
            ("1.00", &[0, 1, 0]),
            ("12345.67", &[1]),
        ];

        for (amount, weights) in cases {
            let whole = usd(amount);
            let parts = whole.allocate(weights).unwrap();
            assert_eq!(parts.len(), weights.len(), "{amount} over {weights:?}");
            assert_eq!(
                Money::total(Currency::USD, parts).unwrap(),
                whole,
                "{amount} over {weights:?} did not add back up"
            );
        }
    }

    #[test]
    fn an_allocation_in_a_zero_decimal_currency_stays_whole() {
        let parts = jpy("100").split(3).unwrap();
        let rendered: Vec<String> = parts.iter().map(|part| part.to_display_string()).collect();
        assert_eq!(rendered, ["34", "33", "33"]);
        assert_eq!(
            Money::total(Currency::parse("JPY").unwrap(), parts).unwrap(),
            jpy("100")
        );
    }

    #[test]
    fn proportions_are_respected_not_just_the_total() {
        let parts = usd("100.00").allocate(&[1, 3]).unwrap();
        let rendered: Vec<String> = parts.iter().map(|part| part.to_display_string()).collect();
        assert_eq!(rendered, ["25.00", "75.00"]);
    }

    #[test]
    fn refuses_an_allocation_that_has_nowhere_to_go() {
        assert_eq!(
            usd("1.00").allocate(&[]),
            Err(MoneyError::NothingToAllocateTo)
        );
        assert_eq!(
            usd("1.00").allocate(&[0, 0]),
            Err(MoneyError::NothingToAllocateTo)
        );
        assert_eq!(
            usd("1.00").allocate(&[1, -1]),
            Err(MoneyError::NegativeWeight)
        );
    }

    #[test]
    fn the_storage_string_is_always_four_places() {
        assert_eq!(usd("1").to_storage_string(), "1.0000");
        assert_eq!(jpy("1").to_storage_string(), "1.0000");
        assert_eq!(usd("-0.5").to_storage_string(), "-0.5000");
        assert_eq!(usd("0").to_storage_string(), "0.0000");
    }

    #[test]
    fn display_carries_the_currency_because_the_amount_alone_is_not_one() {
        assert_eq!(usd("1234.5").to_string(), "1234.50 USD");
        assert_eq!(jpy("1234").to_string(), "1234 JPY");
        assert_eq!(kwd("1.5").to_string(), "1.500 KWD");
    }

    #[test]
    fn crosses_the_wire_as_a_string_not_a_double() {
        let amount = usd("12345678901.2345");
        let json = serde_json::to_string(&amount).unwrap();
        assert_eq!(json, r#"{"amount":"12345678901.2345","currency":"USD"}"#);
        assert_eq!(serde_json::from_str::<Money>(&json).unwrap(), amount);
    }

    #[test]
    fn a_wire_amount_that_is_not_an_amount_is_refused() {
        assert!(serde_json::from_str::<Money>(r#"{"amount":"1.00001","currency":"USD"}"#).is_err());
        assert!(serde_json::from_str::<Money>(r#"{"amount":"1.00","currency":"ZZZ"}"#).is_err());
    }

    #[test]
    fn negation_and_absolute_value_round_trip() {
        let amount = usd("-42.50");
        assert_eq!(amount.negate(), usd("42.50"));
        assert_eq!(amount.abs(), usd("42.50"));
        assert_eq!(usd("42.50").abs(), usd("42.50"));
        assert!(amount.is_negative());
    }

    #[test]
    fn comparison_refuses_across_currencies() {
        assert_eq!(usd("1.00").compare(usd("2.00")).unwrap(), Ordering::Less);
        assert_eq!(usd("2.00").compare(usd("2.00")).unwrap(), Ordering::Equal);
        assert!(usd("1.00").compare(jpy("1")).is_err());
    }

    #[test]
    fn every_error_has_something_to_say() {
        for error in [
            MoneyError::OutOfRange,
            MoneyError::NotANumber,
            MoneyError::NothingToAllocateTo,
            MoneyError::NegativeWeight,
            MoneyError::CurrencyMismatch {
                expected: Currency::USD,
                found: Currency::parse("JPY").unwrap(),
            },
        ] {
            let message = error.message();
            assert!(message.key.starts_with("money."), "{}", message.key);
            assert!(!message.render_builtin().is_empty());
        }
    }
}
