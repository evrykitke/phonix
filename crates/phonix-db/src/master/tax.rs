//! `master.tax_codes`, `tax_rates`, `tax_groups` and `tax_group_members`.
//!
//! # Rates cross as text
//!
//! `NUMERIC(9, 6)` has no lossless integer binding in the driver, and the whole
//! point of [`TaxRate`] is that it is exact. So a rate is bound as
//! `$n::numeric` and read back with `::text`, exactly as `core.exchange_rates`
//! does. It looks roundabout and it is the only form that cannot lose a digit.
//!
//! # There is no delete for a code that has been used
//!
//! [`delete_code`] exists and the database refuses it the moment the code is in
//! a group, because `tax_group_members` references it without a cascade. That
//! is deliberate: retiring a tax is `is_active = false`, and the documents that
//! carried it still have to resolve their snapshot back to something.
//!
//! # Overlapping rates are refused by Postgres, not by Rust
//!
//! The exclusion constraint in migration 0002 is what makes two simultaneously
//! live rates impossible. [`save_rate`] therefore maps the constraint violation
//! into [`DbError::TaxRateOverlap`] rather than checking first: a check-then-
//! insert is a race, and the race is two administrators filing a rate change on
//! the same afternoon.

use chrono::NaiveDate;
use phonix_core::identity::UserId;
use phonix_core::locale::Country;
use phonix_tax::code::{TaxCode, TaxCodeInput, TaxKind};
use phonix_tax::group::{TaxGroup, TaxGroupMember};
use phonix_tax::rate::{TaxRate, TaxRatePeriod, TaxRateRow};
use sqlx::{FromRow, PgExecutor, Row};
use uuid::Uuid;

use crate::error::DbError;

/// The name of the exclusion constraint that refuses two live rates at once.
///
/// Matched against the constraint a violation names, so the failure comes back
/// as something a form can render rather than as a Postgres string.
const OVERLAP_CONSTRAINT: &str = "tax_rates_no_overlap";

/// The unique indexes that refuse two codes, or two groups, with one code.
const CODE_INDEX: &str = "tax_codes_code_key";
const GROUP_CODE_INDEX: &str = "tax_groups_code_key";

/// Turn a unique-index violation into something a form can render.
fn as_code_conflict(err: sqlx::Error, entity: &'static str, index: &str, code: &str) -> DbError {
    match &err {
        sqlx::Error::Database(db) if db.constraint() == Some(index) => DbError::CodeExists {
            entity,
            code: code.to_owned(),
        },
        _ => DbError::Query(err),
    }
}

/// A tax code row.
///
/// A local wrapper rather than an implementation on [`TaxCode`] itself:
/// `FromRow` belongs to sqlx and `TaxCode` belongs to `phonix-tax`, so this
/// crate may implement one for the other only through a type it owns.
struct CodeRow(TaxCode);

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for CodeRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let stored_kind: String = row.try_get("kind")?;

        // Refused rather than defaulted: the kind decides whether an amount
        // posts as a liability, a recoverable asset or a deduction.
        let kind = TaxKind::parse(&stored_kind).ok_or_else(|| sqlx::Error::ColumnDecode {
            index: "kind".to_owned(),
            source: format!("unrecognised tax kind '{stored_kind}'").into(),
        })?;

        let stored_country: Option<String> = row.try_get("country_code")?;
        let country = stored_country
            .map(|code| {
                Country::parse(&code).map_err(|err| sqlx::Error::ColumnDecode {
                    index: "country_code".to_owned(),
                    source: Box::new(err),
                })
            })
            .transpose()?;

        Ok(Self(TaxCode {
            id: row.try_get("id")?,
            code: row.try_get("code")?,
            name: row.try_get("name")?,
            kind,
            country,
            region_code: row.try_get("region_code")?,
            is_compound: row.try_get("is_compound")?,
            is_recoverable: row.try_get("is_recoverable")?,
            is_active: row.try_get("is_active")?,
        }))
    }
}

/// A rate row. A local wrapper, for the reason [`CodeRow`] is - and the shape
/// it decodes into lives in `phonix-tax` because a rates screen needs it in the
/// browser.
struct RateRow(TaxRateRow);

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for RateRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let digits: String = row.try_get("rate")?;
        let rate = TaxRate::parse(&digits).map_err(|err| sqlx::Error::ColumnDecode {
            index: "rate".to_owned(),
            source: Box::new(err),
        })?;

        Ok(Self(TaxRateRow {
            id: row.try_get("id")?,
            tax_code_id: row.try_get("tax_code_id")?,
            period: TaxRatePeriod {
                rate,
                valid_from: row.try_get("valid_from")?,
                valid_to: row.try_get("valid_to")?,
            },
        }))
    }
}

// --- tax codes ----------------------------------------------------------

/// Every tax code, by code.
pub async fn list_codes<'e, E>(executor: E) -> Result<Vec<TaxCode>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query_as::<_, CodeRow>(
        "SELECT id, code, name, kind, country_code, region_code,
                is_compound, is_recoverable, is_active
           FROM master.tax_codes
          ORDER BY lower(code)",
    )
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(rows.into_iter().map(|row| row.0).collect())
}

/// One tax code.
pub async fn find_code<'e, E>(executor: E, id: Uuid) -> Result<Option<TaxCode>, DbError>
where
    E: PgExecutor<'e>,
{
    let row = sqlx::query_as::<_, CodeRow>(
        "SELECT id, code, name, kind, country_code, region_code,
                is_compound, is_recoverable, is_active
           FROM master.tax_codes
          WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(row.map(|row| row.0))
}

/// Create a tax code. Returns its id.
pub async fn insert_code<'e, E>(
    executor: E,
    draft: &TaxCodeInput,
    actor: Option<UserId>,
) -> Result<Uuid, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_scalar(
        "INSERT INTO master.tax_codes
             (code, name, kind, country_code, region_code,
              is_compound, is_recoverable, is_active, created_by, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9)
         RETURNING id",
    )
    .bind(&draft.code)
    .bind(&draft.name)
    .bind(draft.kind.as_str())
    .bind(draft.country.map(Country::code))
    .bind(&draft.region_code)
    .bind(draft.is_compound)
    .bind(draft.is_recoverable)
    .bind(draft.is_active)
    .bind(actor)
    .fetch_one(executor)
    .await
    .map_err(|err| as_code_conflict(err, "tax", CODE_INDEX, &draft.code))
}

/// Store a changed tax code. `false` when there is no such row.
pub async fn update_code<'e, E>(
    executor: E,
    id: Uuid,
    draft: &TaxCodeInput,
    actor: Option<UserId>,
) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let affected = sqlx::query(
        "UPDATE master.tax_codes
            SET code           = $2,
                name           = $3,
                kind           = $4,
                country_code   = $5,
                region_code    = $6,
                is_compound    = $7,
                is_recoverable = $8,
                is_active      = $9,
                updated_at     = now(),
                updated_by     = $10
          WHERE id = $1",
    )
    .bind(id)
    .bind(&draft.code)
    .bind(&draft.name)
    .bind(draft.kind.as_str())
    .bind(draft.country.map(Country::code))
    .bind(&draft.region_code)
    .bind(draft.is_compound)
    .bind(draft.is_recoverable)
    .bind(draft.is_active)
    .bind(actor)
    .execute(executor)
    .await
    .map_err(|err| as_code_conflict(err, "tax", CODE_INDEX, &draft.code))?
    .rows_affected();

    Ok(affected > 0)
}

/// Remove a tax code.
///
/// Postgres refuses this the moment the code is in a group, which is the point:
/// retiring a tax is `is_active = false`, and a group that lost a member
/// silently would change what every document using it comes to.
pub async fn delete_code<'e, E>(executor: E, id: Uuid) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let affected = sqlx::query("DELETE FROM master.tax_codes WHERE id = $1")
        .bind(id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?
        .rows_affected();

    Ok(affected > 0)
}

// --- rates --------------------------------------------------------------

/// Every rate on one code, newest window first.
pub async fn rates_of<'e, E>(executor: E, tax_code_id: Uuid) -> Result<Vec<TaxRateRow>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query_as::<_, RateRow>(
        "SELECT id, tax_code_id, rate::text AS rate, valid_from, valid_to
           FROM master.tax_rates
          WHERE tax_code_id = $1
          ORDER BY valid_from DESC",
    )
    .bind(tax_code_id)
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(rows.into_iter().map(|row| row.0).collect())
}

/// The rate in force on a day, for one code.
///
/// `None` when no window covers that date - which is an answer, and one the
/// caller has to refuse rather than treat as zero. An invoice that is silently
/// too small is not noticed until the return is filed.
pub async fn rate_on<'e, E>(
    executor: E,
    tax_code_id: Uuid,
    on: NaiveDate,
) -> Result<Option<TaxRatePeriod>, DbError>
where
    E: PgExecutor<'e>,
{
    let stored = sqlx::query_as::<_, RateRow>(
        "SELECT id, tax_code_id, rate::text AS rate, valid_from, valid_to
           FROM master.tax_rates
          WHERE tax_code_id = $1
            AND valid_from <= $2
            AND (valid_to IS NULL OR valid_to > $2)
          LIMIT 1",
    )
    .bind(tax_code_id)
    .bind(on)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(stored.map(|row| row.0.period))
}

/// Every code's rate on a day, for resolving a whole group in one round trip.
///
/// A group is up to eight codes, and asking eight times to price one line is
/// eight round trips per document. The exclusion constraint guarantees at most
/// one row per code, so this cannot return a code twice.
pub async fn rates_on<'e, E>(
    executor: E,
    on: NaiveDate,
) -> Result<Vec<(Uuid, TaxRatePeriod)>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query_as::<_, RateRow>(
        "SELECT id, tax_code_id, rate::text AS rate, valid_from, valid_to
           FROM master.tax_rates
          WHERE valid_from <= $1
            AND (valid_to IS NULL OR valid_to > $1)",
    )
    .bind(on)
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(rows
        .into_iter()
        .map(|row| (row.0.tax_code_id, row.0.period))
        .collect())
}

/// Add a rate window, or move the one whose id is given.
///
/// An overlap comes back as [`DbError::TaxRateOverlap`] rather than a raw
/// Postgres error, because it is an expected path through a form: two people
/// filing the same rate change on the same afternoon.
pub async fn save_rate<'e, E>(
    executor: E,
    tax_code_id: Uuid,
    id: Option<Uuid>,
    period: &TaxRatePeriod,
    actor: Option<UserId>,
) -> Result<Uuid, DbError>
where
    E: PgExecutor<'e>,
{
    let result = match id {
        Some(id) => {
            sqlx::query_scalar(
                "UPDATE master.tax_rates
                SET rate = $3::numeric, valid_from = $4, valid_to = $5
              WHERE id = $1 AND tax_code_id = $2
              RETURNING id",
            )
            .bind(id)
            .bind(tax_code_id)
            .bind(period.rate.to_storage_string())
            .bind(period.valid_from)
            .bind(period.valid_to)
            .fetch_one(executor)
            .await
        }
        None => {
            sqlx::query_scalar(
                "INSERT INTO master.tax_rates
                 (tax_code_id, rate, valid_from, valid_to, created_by)
             VALUES ($1, $2::numeric, $3, $4, $5)
             RETURNING id",
            )
            .bind(tax_code_id)
            .bind(period.rate.to_storage_string())
            .bind(period.valid_from)
            .bind(period.valid_to)
            .bind(actor)
            .fetch_one(executor)
            .await
        }
    };

    result.map_err(|err| {
        if names_constraint(&err, OVERLAP_CONSTRAINT) {
            DbError::TaxRateOverlap
        } else {
            DbError::Query(err)
        }
    })
}

/// Remove a rate window. `false` when it was not that code's.
pub async fn delete_rate<'e, E>(
    executor: E,
    tax_code_id: Uuid,
    rate_id: Uuid,
) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let affected = sqlx::query("DELETE FROM master.tax_rates WHERE id = $1 AND tax_code_id = $2")
        .bind(rate_id)
        .bind(tax_code_id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?
        .rows_affected();

    Ok(affected > 0)
}

// --- groups -------------------------------------------------------------

/// Every group, with its members in sequence order.
///
/// Two queries and a stitch rather than one join: a join over the members
/// multiplies the group rows and then has to be un-multiplied, and the members
/// carry the fields the arithmetic reads.
pub async fn list_groups(pool: &sqlx::PgPool) -> Result<Vec<TaxGroup>, DbError> {
    let rows = sqlx::query(
        "SELECT id, code, name, country_code, is_active
           FROM master.tax_groups
          ORDER BY lower(code)",
    )
    .fetch_all(pool)
    .await
    .map_err(DbError::Query)?;

    let members = all_members(pool).await?;

    rows.into_iter()
        .map(|row| build_group(&row, &members))
        .collect()
}

/// One group, with its members.
pub async fn find_group(pool: &sqlx::PgPool, id: Uuid) -> Result<Option<TaxGroup>, DbError> {
    let Some(row) = sqlx::query(
        "SELECT id, code, name, country_code, is_active
           FROM master.tax_groups
          WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(DbError::Query)?
    else {
        return Ok(None);
    };

    let members = members_of(pool, id).await?;
    build_group(&row, &members).map(Some)
}

/// A group being written, whole.
#[derive(Debug, Clone, Copy)]
pub struct GroupWrite<'a> {
    /// `None` creates.
    pub id: Option<Uuid>,
    pub code: &'a str,
    pub name: &'a str,
    pub country: Option<Country>,
    pub is_active: bool,
    /// Tax code ids, **in the order they apply**. `sequence` is derived from
    /// the position, so there is no number for a caller to get wrong.
    pub members: &'a [Uuid],
}

/// Create or replace a group and its membership, in one transaction.
///
/// The whole membership, not a diff, for the reason the permission editor
/// submits the whole set: a diff computed in a browser is a diff against
/// whatever that tab loaded, and applying it silently undoes somebody else's
/// change.
pub async fn save_group(
    pool: &sqlx::PgPool,
    write: GroupWrite<'_>,
    actor: Option<UserId>,
) -> Result<Uuid, DbError> {
    // Destructured once, so the body reads the way it did when these were
    // arguments. Carrying them as one value rather than eight is what stops a
    // caller passing the name where the code goes - both are `&str`, so the
    // two would compile and the mistake would be a group with its own name for
    // a code.
    let GroupWrite {
        id,
        code,
        name,
        country,
        is_active,
        members,
    } = write;

    let mut tx = pool.begin().await.map_err(DbError::Query)?;

    let group_id: Uuid = match id {
        Some(id) => {
            sqlx::query(
                "UPDATE master.tax_groups
                    SET code = $2, name = $3, country_code = $4, is_active = $5,
                        updated_at = now(), updated_by = $6
                  WHERE id = $1",
            )
            .bind(id)
            .bind(code)
            .bind(name)
            .bind(country.map(Country::code))
            .bind(is_active)
            .bind(actor)
            .execute(&mut *tx)
            .await
            .map_err(|err| as_code_conflict(err, "tax group", GROUP_CODE_INDEX, code))?;
            id
        }
        None => sqlx::query_scalar(
            "INSERT INTO master.tax_groups
                 (code, name, country_code, is_active, created_by, updated_by)
             VALUES ($1, $2, $3, $4, $5, $5)
             RETURNING id",
        )
        .bind(code)
        .bind(name)
        .bind(country.map(Country::code))
        .bind(is_active)
        .bind(actor)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| as_code_conflict(err, "tax group", GROUP_CODE_INDEX, code))?,
    };

    // Cleared and rewritten rather than reconciled. The membership is small,
    // the order is the whole meaning, and reconciling a reordering in SQL is
    // how a compound chain ends up in the wrong sequence.
    sqlx::query("DELETE FROM master.tax_group_members WHERE tax_group_id = $1")
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(DbError::Query)?;

    for (position, tax_code_id) in members.iter().enumerate() {
        sqlx::query(
            "INSERT INTO master.tax_group_members (tax_group_id, tax_code_id, sequence)
             VALUES ($1, $2, $3)",
        )
        .bind(group_id)
        .bind(tax_code_id)
        // The membership ceiling is eight, checked before this is called, so a
        // position can never reach a SMALLINT's edge.
        .bind(i16::try_from(position).unwrap_or(i16::MAX))
        .execute(&mut *tx)
        .await
        .map_err(DbError::Query)?;
    }

    tx.commit().await.map_err(DbError::Query)?;
    Ok(group_id)
}

/// Remove a group. `false` when there is no such row.
///
/// The members go with it - they are the group - but nothing else does: a party
/// pointing at it is set to no default, and documents keep the snapshot they
/// already resolved.
pub async fn delete_group<'e, E>(executor: E, id: Uuid) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let affected = sqlx::query("DELETE FROM master.tax_groups WHERE id = $1")
        .bind(id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?
        .rows_affected();

    Ok(affected > 0)
}

/// The members of one group, in sequence order.
async fn members_of<'e, E>(executor: E, group_id: Uuid) -> Result<Vec<MemberRow>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, MemberRow>(
        "SELECT m.tax_group_id, m.tax_code_id, m.sequence,
                c.code, c.name, c.kind, c.is_compound, c.is_recoverable
           FROM master.tax_group_members m
           JOIN master.tax_codes c ON c.id = m.tax_code_id
          WHERE m.tax_group_id = $1
          ORDER BY m.sequence",
    )
    .bind(group_id)
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)
}

/// Every group's members at once, for the list screen.
async fn all_members<'e, E>(executor: E) -> Result<Vec<MemberRow>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, MemberRow>(
        "SELECT m.tax_group_id, m.tax_code_id, m.sequence,
                c.code, c.name, c.kind, c.is_compound, c.is_recoverable
           FROM master.tax_group_members m
           JOIN master.tax_codes c ON c.id = m.tax_code_id
          ORDER BY m.tax_group_id, m.sequence",
    )
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)
}

/// A member row, still carrying the group it belongs to.
struct MemberRow {
    tax_group_id: Uuid,
    member: TaxGroupMember,
}

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for MemberRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let stored_kind: String = row.try_get("kind")?;
        let kind = TaxKind::parse(&stored_kind).ok_or_else(|| sqlx::Error::ColumnDecode {
            index: "kind".to_owned(),
            source: format!("unrecognised tax kind '{stored_kind}'").into(),
        })?;

        Ok(Self {
            tax_group_id: row.try_get("tax_group_id")?,
            member: TaxGroupMember {
                tax_code_id: row.try_get("tax_code_id")?,
                code: row.try_get("code")?,
                name: row.try_get("name")?,
                kind,
                is_compound: row.try_get("is_compound")?,
                is_recoverable: row.try_get("is_recoverable")?,
                sequence: row.try_get("sequence")?,
            },
        })
    }
}

/// Attach the members that belong to one group row.
fn build_group(row: &sqlx::postgres::PgRow, members: &[MemberRow]) -> Result<TaxGroup, DbError> {
    let id: Uuid = row.try_get("id").map_err(DbError::Query)?;
    let stored_country: Option<String> = row.try_get("country_code").map_err(DbError::Query)?;

    let country = stored_country
        .map(|code| {
            Country::parse(&code).map_err(|err| {
                DbError::Query(sqlx::Error::ColumnDecode {
                    index: "country_code".to_owned(),
                    source: Box::new(err),
                })
            })
        })
        .transpose()?;

    Ok(TaxGroup {
        id,
        code: row.try_get("code").map_err(DbError::Query)?,
        name: row.try_get("name").map_err(DbError::Query)?,
        country,
        is_active: row.try_get("is_active").map_err(DbError::Query)?,
        members: members
            .iter()
            .filter(|member| member.tax_group_id == id)
            .map(|member| member.member.clone())
            .collect(),
    })
}

/// Whether a failure is the named constraint being violated.
///
/// Matched on the constraint's own name rather than on the message text, which
/// is localised by the server's `lc_messages` and would stop matching the first
/// time somebody deployed to a machine set to French.
fn names_constraint(err: &sqlx::Error, constraint: &str) -> bool {
    matches!(
        err,
        sqlx::Error::Database(db) if db.constraint() == Some(constraint)
    )
}
