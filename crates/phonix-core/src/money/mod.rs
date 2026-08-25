//! Amounts of money, and what it takes to move one between currencies.
//!
//! # Why this is a type and not a `f64`
//!
//! `0.1 + 0.2` is not `0.3` in binary floating point, and an accounting system
//! whose totals disagree with a hand calculation by a cent is an accounting
//! system nobody will use twice. Every amount here is an exact integer of
//! hundredths of a cent - [`Money::SCALE`] decimal places - which is precisely
//! what `NUMERIC(19, 4)` holds, so a value that round-trips in Rust round-trips
//! in Postgres and neither one is quietly approximating the other.
//!
//! # An amount is never a number on its own
//!
//! [`Money`] is an amount *and* a currency, and there is no way to build one
//! without both. Adding dollars to yen is a `Result`, not a silent success -
//! see [`MoneyError::CurrencyMismatch`]. The database follows the same rule:
//! an amount column is always accompanied by a currency column.
//!
//! # Storage scale and minor units are different numbers
//!
//! Everything is stored at four decimal places regardless of currency, because
//! a unit price of 0.0125 is an ordinary thing and rounding it at the line is
//! how a thousand-unit order comes out wrong. What the *currency* rounds to is
//! [`Currency::minor_units`] - 0 for the yen, 3 for the Kuwaiti dinar - and
//! applying it is an explicit act: [`Money::round_to_minor_unit`]. That is the
//! one function in the workspace that rounds money, so a rounding argument has
//! exactly one place to be settled.
//!
//! # Conversions carry their evidence
//!
//! [`Money::convert`] does not return a number. It returns a [`Conversion`]:
//! both amounts, both currencies, the rate and the date the rate was published.
//! A document stores all six together, because recomputing a base amount later
//! from today's rate silently rewrites history, and it is the single most
//! common way a foreign-currency ledger stops reconciling.
//!
//! # This module compiles to wasm
//!
//! Which is the point. The total the browser previews and the total the server
//! posts come out of this code, not out of two implementations that agree until
//! the day they do not.

mod amount;
mod exchange;
mod selection;

pub use amount::{MAX_SCALED, Money, MoneyError, Rounding};
pub use exchange::{Conversion, ExchangeRate, MAX_SOURCE_LEN, RATE_SCALE, Rate, RateError};
pub use selection::WorkspaceCurrency;
