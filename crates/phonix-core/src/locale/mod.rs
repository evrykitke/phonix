//! Where and how an organization does business: currency, country, time zone.
//!
//! Three small validated types that exist for one reason - a code stored as
//! free text is a code that cannot be grouped by, converted, or added up. They
//! live together because they are asked as one set of questions on the
//! organization profile, and because everything after that (money, reports,
//! addresses on documents) reads them as a group.
//!
//! | Type              | Standard         | Stored as              |
//! | ----------------- | ---------------- | ---------------------- |
//! | [`Currency`]      | ISO 4217         | `CHAR(3)`, e.g. `KES`  |
//! | [`Country`]       | ISO 3166-1 alpha-2 | `CHAR(2)`, e.g. `KE` |
//! | [`Timezone`]      | IANA tz database | `TEXT`, e.g. `Africa/Nairobi` |
//!
//! Each parses on construction and serialises as its bare code, so the column,
//! the JSON on the wire and the value in the browser are the same characters.

pub mod country;
pub mod currency;
pub mod timezone;

pub use country::{Country, UnknownCountry};
pub use currency::{Currency, UnknownCurrency};
pub use timezone::{InvalidTimezone, Timezone};
