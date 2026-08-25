//! Which currencies this workspace deals in, and what a rate was on a day.
//!
//! # Reading is ungated; writing is `Settings`
//!
//! The same split the workspace policy makes. Every screen with an amount on it
//! needs the currency list to render a picker, and a posting routine needs a
//! rate - so requiring a permission to *read* would mean granting the
//! administration area to anybody who can raise a document. Changing the list,
//! or recording a rate, is administration.
//!
//! # The list is audited as one record
//!
//! [`kinds::CURRENCIES`] is a singleton. "Who switched EUR off" and "who loaded
//! Tuesday's rates" are the same question about the same screen, and a history
//! per currency code would be a hundred and sixty histories nobody navigates
//! to.
//!
//! # A rate is recorded, never invented
//!
//! [`rate_on`] answers with a rate somebody published on a stated day, or with
//! nothing. It never interpolates between two rates and never extrapolates
//! backwards past the earliest on file: an auditor asks *which published rate
//! was used*, and "a blend of Tuesday and Thursday" is not an answer.

use chrono::NaiveDate;
use phonix_core::locale::Currency;
use phonix_core::money::ExchangeRate;
use phonix_core::permissions;
use phonix_db::currency as store;
use phonix_db::currency::CurrencyRow;
use phonix_db::organization;
use phonix_db::sqlx::{PgExecutor, PgPool};

use crate::audit::{self, Target, kinds};
use crate::caller::{Caller, acting_user};
use crate::error::{ServiceError, ServiceResult};

/// Every currency on the workspace's list, enabled or not.
///
/// Ungated: this is what a settings screen renders and what a picker filters.
pub async fn list<'e, E>(executor: E) -> ServiceResult<Vec<CurrencyRow>>
where
    E: PgExecutor<'e>,
{
    store::list(executor).await.map_err(ServiceError::from)
}

/// The ones a picker should offer.
pub async fn enabled<'e, E>(executor: E) -> ServiceResult<Vec<CurrencyRow>>
where
    E: PgExecutor<'e>,
{
    store::enabled(executor).await.map_err(ServiceError::from)
}

/// Add a currency to the workspace's list, or change how it is shown.
///
/// Idempotent, because "use EUR" is a statement about the end state rather than
/// an event: a settings screen saving twice must not be an error.
pub async fn save(
    pool: &PgPool,
    caller: &Caller,
    currency: Currency,
    is_enabled: bool,
    symbol: Option<&str>,
) -> ServiceResult<()> {
    caller.require(permissions::SETTINGS)?;
    acting_user(caller)?;

    let before = snapshot(pool).await?;
    store::upsert(pool, currency, is_enabled, symbol, caller.user_id()).await?;
    let after = snapshot(pool).await?;

    audit::updated(
        pool,
        caller,
        Target::singleton(kinds::CURRENCIES).fact("currency", currency.code()),
        &before,
        &after,
    )
    .await;

    Ok(())
}

/// Switch one on or off.
///
/// There is no delete. A currency the workspace has stopped using is disabled:
/// rates and posted documents still have to resolve, and a foreign-key error
/// naming `exchange_rates` is not a useful answer to somebody tidying a picker.
///
/// The base currency cannot be switched off. Every amount in the workspace is
/// expressed against it, and a picker that could not offer it would be a screen
/// that cannot show its own totals.
pub async fn set_enabled(
    pool: &PgPool,
    caller: &Caller,
    currency: Currency,
    is_enabled: bool,
) -> ServiceResult<()> {
    caller.require(permissions::SETTINGS)?;
    acting_user(caller)?;

    if !is_enabled {
        let profile = organization::load(pool).await?;
        if profile.profile.currency == currency {
            return Err(ServiceError::rejected(
                "currency",
                phonix_core::msg!("error.currency.base_locked", code = currency.code()),
            ));
        }
    }

    let before = snapshot(pool).await?;
    if !store::set_enabled(pool, currency, is_enabled, caller.user_id()).await? {
        return Err(ServiceError::rejected(
            "currency",
            phonix_core::msg!("error.currency.gone", code = currency.code()),
        ));
    }
    let after = snapshot(pool).await?;

    audit::updated(
        pool,
        caller,
        Target::singleton(kinds::CURRENCIES).fact("currency", currency.code()),
        &before,
        &after,
    )
    .await;

    Ok(())
}

/// The rate to use for a document dated `on`.
///
/// Ungated: this is the posting path. See the module note for why it never
/// interpolates.
pub async fn rate_on<'e, E>(
    executor: E,
    base: Currency,
    quote: Currency,
    on: NaiveDate,
    source: Option<&str>,
) -> ServiceResult<Option<ExchangeRate>>
where
    E: PgExecutor<'e>,
{
    store::rate_on(executor, base, quote, on, source)
        .await
        .map_err(ServiceError::from)
}

/// The most recent rates for a pair, newest first. For a rates screen.
pub async fn recent_rates<'e, E>(
    executor: E,
    base: Currency,
    quote: Currency,
    limit: i64,
) -> ServiceResult<Vec<ExchangeRate>>
where
    E: PgExecutor<'e>,
{
    // Clamped rather than trusted: the limit reaches a `LIMIT` clause from a
    // screen control, and a page asking for a million rows is a page that times
    // out for everybody sharing the pool.
    let limit = limit.clamp(1, 500);

    store::recent_rates(executor, base, quote, limit)
        .await
        .map_err(ServiceError::from)
}

/// Record a published rate.
///
/// Upserts on (pair, day, source), so re-running a feed for a day it already
/// covered corrects the row instead of leaving two rows and a question about
/// which one a document used.
///
/// Both currencies have to be on the workspace's list first - the column's
/// foreign keys say so, and asking here means the answer names the currency
/// rather than a constraint.
pub async fn record_rate(pool: &PgPool, caller: &Caller, rate: &ExchangeRate) -> ServiceResult<()> {
    caller.require(permissions::SETTINGS)?;
    acting_user(caller)?;

    let listed = store::list(pool).await?;
    for currency in [rate.base, rate.quote] {
        if !listed.iter().any(|row| row.currency == currency) {
            return Err(ServiceError::rejected(
                "currency",
                phonix_core::msg!("error.currency.not_listed", code = currency.code()),
            ));
        }
    }

    store::record_rate(pool, rate, caller.user_id()).await?;

    // A fact rather than a diff: a rate is an addition to a growing series, not
    // a field that moved, and drawing it as a before and an after would claim a
    // change to something that did not previously exist.
    audit::changed_json(
        pool,
        caller,
        Target::singleton(kinds::CURRENCIES)
            .fact(
                "pair",
                format!("{}/{}", rate.base.code(), rate.quote.code()),
            )
            .fact("as_of", rate.as_of.to_string())
            .fact("source", &rate.source),
        serde_json::Value::Null,
        serde_json::json!({ "rate": rate.rate.to_storage_string() }),
    )
    .await;

    Ok(())
}

/// The workspace's currency list, in the shape the audit diff records it.
///
/// Codes and their display choices, sorted - which is what makes "EUR was
/// switched off" a one-line diff rather than a rewritten table.
async fn snapshot(pool: &PgPool) -> ServiceResult<Vec<CurrencySnapshot>> {
    Ok(store::list(pool)
        .await?
        .into_iter()
        .map(|row| CurrencySnapshot {
            code: row.currency.code().to_owned(),
            is_enabled: row.is_enabled,
            symbol: row.symbol,
        })
        .collect())
}

/// One currency, as the trail records it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct CurrencySnapshot {
    code: String,
    is_enabled: bool,
    symbol: Option<String>,
}
