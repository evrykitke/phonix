//! The `tenant_licences` table.
//!
//! One row per workspace, read on every request as part of the catalog row
//! (see [`super::catalog`]) and written only by Desk. The vocabulary -
//! [`phonix_core::Licence`], the three states, and what "current" means - is in
//! `phonix-core`, because both the thing that decides and the thing that
//! displays need it and neither should own it.
//!
//! # There is no `expire` here, and there never will be
//!
//! A licence ends by a date passing, which needs no statement and no job. The
//! moment something wrote `state = 'expired'` on a schedule, the reason a
//! licence ended would be gone - and a workspace that was deliberately
//! withdrawn and then also lapsed would come back on when payment cleared.
//! See ADR 0005 section 7.

use chrono::{DateTime, Utc};
use phonix_core::{Licence, LicenceState, TenantId};
use sqlx::{PgExecutor, Row};

use crate::error::DbError;

/// A licence to write. Everything a desk user decided, and nothing derived.
#[derive(Debug, Clone)]
pub struct LicenceInput<'a> {
    pub state: LicenceState,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
    pub note: Option<&'a str>,
    /// The desk user's address. Text, so deleting an account does not erase
    /// who authorized a workspace.
    pub updated_by: &'a str,
}

const SELECT_ONE: &str = "SELECT state, valid_from, valid_until, note, updated_at, updated_by \
     FROM tenant_licences WHERE tenant_id = $1";

/// Insert or replace this workspace's licence.
///
/// An upsert rather than an insert-and-supersede: the row *is* the current
/// answer, and what it used to be is a `desk_audit` row written by the service
/// above this - in a database the workspace cannot edit. Two places holding
/// licence history would eventually disagree about which one is in force.
const INSERT_IF_ABSENT: &str = "INSERT INTO tenant_licences \
     (tenant_id, state, valid_from, valid_until, note, updated_by) \
     VALUES ($1, $2, $3, $4, $5, $6) \
     ON CONFLICT (tenant_id) DO NOTHING";

const UPSERT: &str = "INSERT INTO tenant_licences \
     (tenant_id, state, valid_from, valid_until, note, updated_at, updated_by) \
     VALUES ($1, $2, $3, $4, $5, now(), $6) \
     ON CONFLICT (tenant_id) DO UPDATE SET \
       state = EXCLUDED.state, valid_from = EXCLUDED.valid_from, \
       valid_until = EXCLUDED.valid_until, note = EXCLUDED.note, \
       updated_at = now(), updated_by = EXCLUDED.updated_by \
     RETURNING state, valid_from, valid_until, note, updated_at, updated_by";

/// Issue a licence only if the workspace has none.
///
/// What provisioning calls, and the `DO NOTHING` is the point: provisioning is
/// retryable by design, and a retry must not quietly reinstate a licence
/// somebody has since withdrawn. Returns whether a row was written, because a
/// caller writing an audit trail has to say which of the two happened.
pub async fn issue_if_absent<'e, E>(
    executor: E,
    tenant_id: TenantId,
    input: LicenceInput<'_>,
) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query(INSERT_IF_ABSENT)
        .bind(tenant_id)
        .bind(input.state.as_str())
        .bind(input.valid_from)
        .bind(input.valid_until)
        .bind(input.note)
        .bind(input.updated_by)
        .execute(executor)
        .await
        .map_err(DbError::Query)?;

    Ok(result.rows_affected() > 0)
}

/// Read one workspace's licence. `Ok(None)` means it has none, which is a
/// refusal to serve rather than an error - see `TenantStatus::serves_traffic`.
pub async fn find<'e, E>(executor: E, tenant_id: TenantId) -> Result<Option<Licence>, DbError>
where
    E: PgExecutor<'e>,
{
    let row = sqlx::query(SELECT_ONE)
        .bind(tenant_id)
        .fetch_optional(executor)
        .await
        .map_err(DbError::Query)?;

    row.as_ref().map(from_row).transpose()
}

/// Issue, extend or withdraw. All three are this one statement, because they
/// are the same act with different arguments - which is also why the audit row
/// carries the before and after rather than a verb.
pub async fn set<'e, E>(
    executor: E,
    tenant_id: TenantId,
    input: LicenceInput<'_>,
) -> Result<Licence, DbError>
where
    E: PgExecutor<'e>,
{
    let row = sqlx::query(UPSERT)
        .bind(tenant_id)
        .bind(input.state.as_str())
        .bind(input.valid_from)
        .bind(input.valid_until)
        .bind(input.note)
        .bind(input.updated_by)
        .fetch_one(executor)
        .await
        .map_err(|err| match &err {
            // 23514 = check_violation. The only one a caller can trip is an
            // end date at or before the start, which is a form error and not a
            // broken deployment.
            sqlx::Error::Database(db) if db.code().as_deref() == Some("23514") => {
                DbError::InvalidPolicy(
                    "a licence must end after it starts, and a note is at most 500 characters"
                        .to_owned(),
                )
            }
            _ => DbError::Query(err),
        })?;

    from_row(&row)
}

/// Build a [`Licence`] from a row carrying the six licence columns.
///
/// Shared with `catalog`'s joined select through [`from_prefixed_row`], so the
/// two cannot come to decode the same table differently.
fn from_row(row: &sqlx::postgres::PgRow) -> Result<Licence, DbError> {
    read(
        row,
        "state",
        "valid_from",
        "valid_until",
        "note",
        "updated_at",
        "updated_by",
    )
}

/// The same, for a row where the licence arrived under `licence_`-prefixed
/// aliases because it was joined onto a tenant that has its own `updated_at`.
pub(crate) fn from_prefixed_row(row: &sqlx::postgres::PgRow) -> Result<Option<Licence>, DbError> {
    let state: Option<String> = row.try_get("licence_state").map_err(DbError::Query)?;
    if state.is_none() {
        return Ok(None);
    }

    read(
        row,
        "licence_state",
        "licence_valid_from",
        "licence_valid_until",
        "licence_note",
        "licence_updated_at",
        "licence_updated_by",
    )
    .map(Some)
}

fn read(
    row: &sqlx::postgres::PgRow,
    state: &str,
    valid_from: &str,
    valid_until: &str,
    note: &str,
    updated_at: &str,
    updated_by: &str,
) -> Result<Licence, DbError> {
    let raw_state: String = row.try_get(state).map_err(DbError::Query)?;

    Ok(Licence {
        // A stored state no variant matches is a migration that did not run,
        // not a transient failure - so it is `CorruptRow` and it names the
        // value, because the next question is always "what is in that column".
        state: LicenceState::parse(&raw_state).ok_or_else(|| {
            DbError::CorruptRow(format!("unrecognised licence state '{raw_state}'"))
        })?,
        valid_from: row.try_get(valid_from).map_err(DbError::Query)?,
        valid_until: row.try_get(valid_until).map_err(DbError::Query)?,
        note: row.try_get(note).map_err(DbError::Query)?,
        updated_at: row.try_get(updated_at).map_err(DbError::Query)?,
        updated_by: row.try_get(updated_by).map_err(DbError::Query)?,
    })
}
