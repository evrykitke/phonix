//! The `organization_profile` row: who this workspace legally is.
//!
//! One row per tenant database, created by migration 0010 with an empty legal
//! name. Empty is a meaningful state - it is what "nobody has filled this in
//! yet" looks like - so the row is seeded by the migration rather than created
//! on first save, and every read returns something.
//!
//! # The logo is not part of a save
//!
//! [`save`] does not touch `logo_file_id`, and [`ProfileUpdate`] has no field
//! for it. Setting the logo is [`set_logo`], a separate statement, for the same
//! reason the mail relay's password is separate from its host: a draft opened
//! before somebody else changed the logo would otherwise put the old one back
//! on every document, silently, as a side effect of correcting a postcode.
//!
//! # Codes in, domain types out
//!
//! `country_code`, `currency_code` and `timezone` are TEXT in Postgres and
//! validated types in Rust. The conversion happens in [`FromRow`] rather than
//! by deriving it on the raw columns, so a row that somehow holds a code no
//! longer in the table is refused loudly here instead of defaulting to dollars
//! several layers up.

use chrono::{DateTime, Utc};
use phonix_core::identity::UserId;
use phonix_core::locale::{Country, Currency, Timezone};
use phonix_core::organization::OrganizationProfile;
use sqlx::{FromRow, PgExecutor, Row};
use uuid::Uuid;

use crate::error::DbError;

/// The stored row.
#[derive(Debug, Clone)]
pub struct ProfileRow {
    pub profile: OrganizationProfile,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<UserId>,
}

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for ProfileRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let raw_currency: String = row.try_get("currency_code")?;
        let raw_country: Option<String> = row.try_get("country_code")?;
        let raw_timezone: String = row.try_get("timezone")?;
        let fiscal_month: i16 = row.try_get("fiscal_year_start_month")?;

        let currency = Currency::parse(&raw_currency).map_err(|err| sqlx::Error::ColumnDecode {
            index: "currency_code".to_owned(),
            source: Box::new(err),
        })?;

        // Transposed rather than defaulted: a stored code that no longer
        // resolves is a decode failure, not an address in no country.
        let country = raw_country
            .as_deref()
            .map(Country::parse)
            .transpose()
            .map_err(|err| sqlx::Error::ColumnDecode {
                index: "country_code".to_owned(),
                source: Box::new(err),
            })?;

        let timezone = Timezone::parse(&raw_timezone).map_err(|err| sqlx::Error::ColumnDecode {
            index: "timezone".to_owned(),
            source: Box::new(err),
        })?;

        let fiscal_year_start_month =
            u8::try_from(fiscal_month).map_err(|_| sqlx::Error::ColumnDecode {
                index: "fiscal_year_start_month".to_owned(),
                source: format!("month {fiscal_month} is not a month").into(),
            })?;

        Ok(Self {
            profile: OrganizationProfile {
                legal_name: row.try_get("legal_name")?,
                trading_name: row.try_get("trading_name")?,
                registration_number: row.try_get("registration_number")?,
                tax_id: row.try_get("tax_id")?,
                industry: row.try_get("industry")?,
                email: row.try_get("email")?,
                phone: row.try_get("phone")?,
                website: row.try_get("website")?,
                address_line1: row.try_get("address_line1")?,
                address_line2: row.try_get("address_line2")?,
                city: row.try_get("city")?,
                region: row.try_get("region")?,
                postal_code: row.try_get("postal_code")?,
                country,
                currency,
                timezone,
                fiscal_year_start_month,
                logo_file_id: row.try_get("logo_file_id")?,
            },
            updated_at: row.try_get("updated_at")?,
            updated_by: row.try_get("updated_by")?,
        })
    }
}

const SELECT: &str = "SELECT legal_name, trading_name, registration_number, tax_id, industry, \
     email, phone, website, address_line1, address_line2, city, region, postal_code, \
     country_code, currency_code, timezone, fiscal_year_start_month, logo_file_id, \
     updated_at, updated_by \
     FROM organization_profile WHERE id";

/// This workspace's profile. Always present - the migration seeds it.
pub async fn load<'e, E>(executor: E) -> Result<ProfileRow, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, ProfileRow>(SELECT)
        .fetch_one(executor)
        .await
        .map_err(DbError::Query)
}

/// What a save writes.
///
/// Borrowed, so the service does not clone a profile it already holds. No
/// `logo_file_id` - see the module note.
#[derive(Debug, Clone)]
pub struct ProfileUpdate<'a> {
    pub legal_name: &'a str,
    pub trading_name: Option<&'a str>,
    pub registration_number: Option<&'a str>,
    pub tax_id: Option<&'a str>,
    pub industry: Option<&'a str>,
    pub email: Option<&'a str>,
    pub phone: Option<&'a str>,
    pub website: Option<&'a str>,
    pub address_line1: Option<&'a str>,
    pub address_line2: Option<&'a str>,
    pub city: Option<&'a str>,
    pub region: Option<&'a str>,
    pub postal_code: Option<&'a str>,
    pub country: Option<Country>,
    pub currency: Currency,
    pub timezone: &'a Timezone,
    pub fiscal_year_start_month: u8,
    pub updated_by: Option<UserId>,
}

/// Replace the row, apart from the logo.
pub async fn save<'e, E>(executor: E, update: ProfileUpdate<'_>) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE organization_profile
            SET legal_name              = $1,
                trading_name            = $2,
                registration_number     = $3,
                tax_id                  = $4,
                industry                = $5,
                email                   = $6,
                phone                   = $7,
                website                 = $8,
                address_line1           = $9,
                address_line2           = $10,
                city                    = $11,
                region                  = $12,
                postal_code             = $13,
                country_code            = $14,
                currency_code           = $15,
                timezone                = $16,
                fiscal_year_start_month = $17,
                updated_at              = now(),
                updated_by              = $18
          WHERE id",
    )
    .bind(update.legal_name)
    .bind(update.trading_name)
    .bind(update.registration_number)
    .bind(update.tax_id)
    .bind(update.industry)
    .bind(update.email)
    .bind(update.phone)
    .bind(update.website)
    .bind(update.address_line1)
    .bind(update.address_line2)
    .bind(update.city)
    .bind(update.region)
    .bind(update.postal_code)
    .bind(update.country.map(Country::code))
    .bind(update.currency.code())
    .bind(update.timezone.as_str())
    .bind(i16::from(update.fiscal_year_start_month))
    .bind(update.updated_by)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Point the profile at an uploaded logo, and say which one it replaced.
///
/// The previous id is what lets the caller delete the file it displaced, so
/// changing a logo ten times leaves one file rather than ten.
pub async fn set_logo<'e, E>(
    executor: E,
    file_id: Uuid,
    updated_by: Option<UserId>,
) -> Result<Option<Uuid>, DbError>
where
    E: PgExecutor<'e>,
{
    // `RETURNING` on an UPDATE sees the new row, so the old value is captured
    // by a CTE reading it inside the same statement - the same shape as
    // `identity::user::set_avatar`, and for the same reason.
    let previous: Option<Option<Uuid>> = sqlx::query_scalar(
        "WITH previous AS (
             SELECT logo_file_id FROM organization_profile WHERE id
         )
         UPDATE organization_profile
            SET logo_file_id = $1,
                updated_at   = now(),
                updated_by   = $2
           FROM previous
          WHERE organization_profile.id
      RETURNING previous.logo_file_id",
    )
    .bind(file_id)
    .bind(updated_by)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(previous.flatten())
}

/// Remove the logo, and say which file it was.
pub async fn clear_logo<'e, E>(
    executor: E,
    updated_by: Option<UserId>,
) -> Result<Option<Uuid>, DbError>
where
    E: PgExecutor<'e>,
{
    let previous: Option<Option<Uuid>> = sqlx::query_scalar(
        "WITH previous AS (
             SELECT logo_file_id FROM organization_profile WHERE id
         )
         UPDATE organization_profile
            SET logo_file_id = NULL,
                updated_at   = now(),
                updated_by   = $1
           FROM previous
          WHERE organization_profile.id
      RETURNING previous.logo_file_id",
    )
    .bind(updated_by)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(previous.flatten())
}
