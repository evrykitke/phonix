//! `currencies` and `exchange_rates`: what this workspace deals in, and what a
//! rate was on a day.
//!
//! # The row is the selection, not the currency
//!
//! Names and minor units come from `phonix_core::locale::Currency`, which is
//! compiled in. This table says only *which* codes the workspace uses, so a row
//! decodes into a `Currency` and there is nothing else in it to read. See
//! migration 0015 for why the ISO list is not duplicated per tenant.
//!
//! # Numerics cross as text
//!
//! `NUMERIC` has no lossless integer binding in the driver, and the whole point
//! of [`Money`](phonix_core::money::Money) and [`Rate`] is that they are exact.
//! So amounts and rates are bound as `$n::numeric` and read back with `::text`.
//! It looks roundabout and it is the only form that cannot lose a digit.
//!
//! # There is no delete
//!
//! A currency the workspace has stopped using is disabled, not removed. Rates
//! and posted documents still have to resolve, and a foreign key error naming
//! `exchange_rates` is not a useful answer to somebody tidying up a picker.

use chrono::NaiveDate;
use phonix_core::identity::UserId;
use phonix_core::locale::Currency;
use phonix_core::money::{ExchangeRate, Rate, WorkspaceCurrency};

/// What a currency row decodes into.
///
/// The shared type under the name this crate has always used, so no caller
/// changed when it moved to `phonix-core` to be able to cross the wire.
pub type CurrencyRow = WorkspaceCurrency;
use sqlx::{FromRow, PgExecutor, Row};

use crate::error::DbError;

/// One currency the workspace has switched on.
///
/// A local wrapper rather than an implementation on
/// [`WorkspaceCurrency`](phonix_core::money::WorkspaceCurrency) itself:
/// `FromRow` belongs to sqlx and the type belongs to `phonix-core`, so this
/// crate may implement one for the other only through a type it owns. The
/// shared type is what crosses the wire to the settings screen.
struct CurrencyRowDecode(CurrencyRow);

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for CurrencyRowDecode {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let code: String = row.try_get("code")?;

        // Refused rather than defaulted, exactly as `organization_profile`
        // does: a stored code this build cannot resolve has no minor units, and
        // an amount whose scale is a guess is worse than a failed read.
        let currency = Currency::parse(&code).map_err(|err| sqlx::Error::ColumnDecode {
            index: "code".to_owned(),
            source: Box::new(err),
        })?;

        Ok(Self(CurrencyRow {
            currency,
            is_enabled: row.try_get("is_enabled")?,
            symbol: row.try_get("symbol")?,
        }))
    }
}

/// Every currency on the workspace's list, enabled or not, by code.
pub async fn list<'e, E>(executor: E) -> Result<Vec<CurrencyRow>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query_as::<_, CurrencyRowDecode>(
        "SELECT code, is_enabled, symbol FROM currencies ORDER BY code",
    )
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(rows.into_iter().map(|row| row.0).collect())
}

/// The ones a picker should offer.
pub async fn enabled<'e, E>(executor: E) -> Result<Vec<CurrencyRow>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query_as::<_, CurrencyRowDecode>(
        "SELECT code, is_enabled, symbol FROM currencies WHERE is_enabled ORDER BY code",
    )
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(rows.into_iter().map(|row| row.0).collect())
}

/// Add a currency to the workspace's list, or update the one already there.
///
/// Idempotent, because "use EUR" is a statement about the end state rather than
/// an event - a settings screen saving twice must not be an error.
pub async fn upsert<'e, E>(
    executor: E,
    currency: Currency,
    is_enabled: bool,
    symbol: Option<&str>,
    actor: Option<UserId>,
) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "INSERT INTO currencies (code, is_enabled, symbol, updated_by)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (code) DO UPDATE
            SET is_enabled = EXCLUDED.is_enabled,
                symbol     = EXCLUDED.symbol,
                updated_at = now(),
                updated_by = EXCLUDED.updated_by",
    )
    .bind(currency.code())
    .bind(is_enabled)
    .bind(symbol)
    .bind(actor)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Switch one on or off. `false` when it was not on the list at all.
pub async fn set_enabled<'e, E>(
    executor: E,
    currency: Currency,
    is_enabled: bool,
    actor: Option<UserId>,
) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let affected = sqlx::query(
        "UPDATE currencies
            SET is_enabled = $2, updated_at = now(), updated_by = $3
          WHERE code = $1",
    )
    .bind(currency.code())
    .bind(is_enabled)
    .bind(actor)
    .execute(executor)
    .await
    .map_err(DbError::Query)?
    .rows_affected();

    Ok(affected > 0)
}

/// The rate to use for a document dated `on`: the most recently published one
/// at or before that date.
///
/// **Never interpolated.** The answer is a rate somebody published on a stated
/// day, because that is the only kind an auditor accepts. A document dated
/// before the earliest rate on file gets `None` rather than the earliest one -
/// extrapolating backwards is inventing a quotation.
///
/// When two sources published on the same day, the most recently recorded wins.
/// Pass `source` to pin it to one feed instead.
pub async fn rate_on<'e, E>(
    executor: E,
    base: Currency,
    quote: Currency,
    on: NaiveDate,
    source: Option<&str>,
) -> Result<Option<ExchangeRate>, DbError>
where
    E: PgExecutor<'e>,
{
    let row = sqlx::query(
        "SELECT base_code, quote_code, rate::text AS rate, as_of, source
           FROM exchange_rates
          WHERE base_code = $1
            AND quote_code = $2
            AND as_of <= $3
            AND ($4::text IS NULL OR source = $4)
          ORDER BY as_of DESC, created_at DESC
          LIMIT 1",
    )
    .bind(base.code())
    .bind(quote.code())
    .bind(on)
    .bind(source)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)?;

    row.map(decode_rate).transpose().map_err(DbError::Query)
}

/// The most recent rates for a pair, newest first. For a rates screen.
pub async fn recent_rates<'e, E>(
    executor: E,
    base: Currency,
    quote: Currency,
    limit: i64,
) -> Result<Vec<ExchangeRate>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query(
        "SELECT base_code, quote_code, rate::text AS rate, as_of, source
           FROM exchange_rates
          WHERE base_code = $1 AND quote_code = $2
          ORDER BY as_of DESC, created_at DESC
          LIMIT $3",
    )
    .bind(base.code())
    .bind(quote.code())
    .bind(limit)
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    rows.into_iter()
        .map(decode_rate)
        .collect::<Result<Vec<_>, _>>()
        .map_err(DbError::Query)
}

/// Record a published rate.
///
/// Upserts on (pair, day, source), so re-running a feed for a day it already
/// covered corrects the row instead of leaving two rows and a question about
/// which one a document used.
pub async fn record_rate<'e, E>(
    executor: E,
    rate: &ExchangeRate,
    actor: Option<UserId>,
) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "INSERT INTO exchange_rates (base_code, quote_code, rate, as_of, source, created_by)
         VALUES ($1, $2, $3::numeric, $4, $5, $6)
         ON CONFLICT (base_code, quote_code, as_of, source) DO UPDATE
            SET rate       = EXCLUDED.rate,
                created_at = now(),
                created_by = EXCLUDED.created_by",
    )
    .bind(rate.base.code())
    .bind(rate.quote.code())
    .bind(rate.rate.to_storage_string())
    .bind(rate.as_of)
    .bind(&rate.source)
    .bind(actor)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// One row into an [`ExchangeRate`].
///
/// Not a `FromRow` impl, because building one goes through
/// [`ExchangeRate::new`] - which is what refuses a row quoting a currency
/// against itself, and that check is worth keeping on the read path too.
fn decode_rate(row: sqlx::postgres::PgRow) -> Result<ExchangeRate, sqlx::Error> {
    let base_code: String = row.try_get("base_code")?;
    let quote_code: String = row.try_get("quote_code")?;
    let rate_text: String = row.try_get("rate")?;

    let base = Currency::parse(&base_code).map_err(|err| sqlx::Error::ColumnDecode {
        index: "base_code".to_owned(),
        source: Box::new(err),
    })?;
    let quote = Currency::parse(&quote_code).map_err(|err| sqlx::Error::ColumnDecode {
        index: "quote_code".to_owned(),
        source: Box::new(err),
    })?;
    let rate = Rate::parse(&rate_text).map_err(|err| sqlx::Error::ColumnDecode {
        index: "rate".to_owned(),
        source: Box::new(err),
    })?;

    ExchangeRate::new(
        base,
        quote,
        rate,
        row.try_get("as_of")?,
        row.try_get::<String, _>("source")?,
    )
    .map_err(|err| sqlx::Error::ColumnDecode {
        index: "base_code".to_owned(),
        source: Box::new(err),
    })
}
