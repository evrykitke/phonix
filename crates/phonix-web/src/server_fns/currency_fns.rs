//! Which currencies this workspace deals in, and what a rate was on a day.
//!
//! # Reading is ungated
//!
//! Every screen with an amount on it needs the currency list to render a
//! picker, and a posting routine needs a rate - so requiring a permission to
//! *read* would mean granting the administration area to anybody who can raise
//! a document. Changing the list, or recording a rate, requires
//! `Administration.Settings`, checked in the service where the write happens.

use chrono::NaiveDate;
use leptos::prelude::*;
use phonix_core::locale::Currency;
use phonix_core::money::{ExchangeRate, WorkspaceCurrency};
use serde::{Deserialize, Serialize};

/// Every currency on the workspace's list, enabled or not.
///
/// What the settings screen renders. A picker wants [`enabled_currencies`].
#[server(name = WorkspaceCurrencies, prefix = "/api", endpoint = "currencies")]
pub async fn workspace_currencies() -> Result<Vec<WorkspaceCurrency>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, _) = pool_and_caller().await?;

    phonix_services::currency::list(&pool)
        .await
        .map_err(service_error)
}

/// The currencies a picker should offer.
#[server(name = EnabledCurrencies, prefix = "/api", endpoint = "currencies/enabled")]
pub async fn enabled_currencies() -> Result<Vec<Currency>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, _) = pool_and_caller().await?;

    Ok(phonix_services::currency::enabled(&pool)
        .await
        .map_err(service_error)?
        .into_iter()
        .map(|row| row.currency)
        .collect())
}

/// Add a currency to the workspace's list, or change how it is shown.
#[server(name = SaveCurrency, prefix = "/api", endpoint = "currencies/save")]
pub async fn save_currency(
    code: String,
    is_enabled: bool,
    symbol: Option<String>,
) -> Result<Vec<WorkspaceCurrency>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    let currency = Currency::parse(&code).map_err(|_| ServerFnError::new("Unknown currency."))?;

    let symbol = symbol
        .as_deref()
        .map(str::trim)
        .filter(|symbol| !symbol.is_empty());

    phonix_services::currency::save(&pool, &caller, currency, is_enabled, symbol)
        .await
        .map_err(service_error)?;

    // The list as it now stands, so the screen re-renders from what was stored
    // rather than from what it hoped it sent.
    phonix_services::currency::list(&pool)
        .await
        .map_err(service_error)
}

/// Switch a currency on or off.
///
/// There is no delete: rates and posted documents still have to resolve. The
/// base currency cannot be switched off, and the service says so.
#[server(name = SetCurrencyEnabled, prefix = "/api", endpoint = "currencies/enable")]
pub async fn set_currency_enabled(
    code: String,
    is_enabled: bool,
) -> Result<Vec<WorkspaceCurrency>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    let currency = Currency::parse(&code).map_err(|_| ServerFnError::new("Unknown currency."))?;

    phonix_services::currency::set_enabled(&pool, &caller, currency, is_enabled)
        .await
        .map_err(service_error)?;

    phonix_services::currency::list(&pool)
        .await
        .map_err(service_error)
}

/// One published rate, as a screen submits it.
///
/// The rate crosses as a string for the reason every amount does: a JSON number
/// is an IEEE double in most parsers, and ten decimal places do not survive one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateEntry {
    pub base_code: String,
    pub quote_code: String,
    /// A plain decimal string: `1.0925`, `0.0066841`.
    pub rate: String,
    pub as_of: NaiveDate,
    pub source: String,
}

/// The most recent rates for a pair, newest first.
#[server(name = RecentRates, prefix = "/api", endpoint = "currencies/rates")]
pub async fn recent_rates(
    base_code: String,
    quote_code: String,
    limit: i64,
) -> Result<Vec<ExchangeRate>, ServerFnError> {
    use crate::state::{pool_and_caller, service_error};

    let (pool, _) = pool_and_caller().await?;

    let base = Currency::parse(&base_code).map_err(|_| ServerFnError::new("Unknown currency."))?;
    let quote =
        Currency::parse(&quote_code).map_err(|_| ServerFnError::new("Unknown currency."))?;

    phonix_services::currency::recent_rates(&pool, base, quote, limit)
        .await
        .map_err(service_error)
}

/// Record a published rate.
///
/// Every part of the entry is parsed here and refused as a whole: an inverted
/// pair, a rate with an eleventh decimal place and an unnamed source are all
/// things that produce a *plausible* wrong number, which is the kind nobody
/// catches.
#[server(name = RecordRate, prefix = "/api", endpoint = "currencies/rates/save")]
pub async fn record_rate(entry: RateEntry) -> Result<(), ServerFnError> {
    use phonix_core::money::{ExchangeRate, Rate};

    use crate::state::{pool_and_caller, service_error};

    let (pool, caller) = pool_and_caller().await?;

    let base =
        Currency::parse(&entry.base_code).map_err(|_| ServerFnError::new("Unknown currency."))?;
    let quote =
        Currency::parse(&entry.quote_code).map_err(|_| ServerFnError::new("Unknown currency."))?;
    // Parsed here and refused as a whole. Each of these produces a *plausible*
    // wrong number if it is let through, which is the kind nobody catches.
    let rate = Rate::parse(&entry.rate).map_err(|err| ServerFnError::new(err.to_string()))?;
    let rate = ExchangeRate::new(base, quote, rate, entry.as_of, &entry.source)
        .map_err(|err| ServerFnError::new(err.to_string()))?;

    phonix_services::currency::record_rate(&pool, &caller, &rate)
        .await
        .map_err(service_error)
}
