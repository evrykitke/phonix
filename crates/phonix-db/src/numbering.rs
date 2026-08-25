//! `number_sequences`: handing out the next document number.
//!
//! # The guarantee, and what it costs
//!
//! [`allocate`] is one `UPDATE ... RETURNING`. That single statement takes a row
//! lock which Postgres holds **until the surrounding transaction ends**, which
//! buys two things at once:
//!
//! * a rollback *returns* the number rather than burning it, so a failed save
//!   leaves no gap;
//! * concurrent allocators queue on the row instead of racing, so no two
//!   documents get the same number.
//!
//! The cost is real and worth stating: every document of one type serialises
//! through one row. For an invoice that is the correct trade — the law wants an
//! unbroken sequence and an unbroken sequence is inherently serial. For a job
//! number or an internal reference it is overkill, and the design allows for a
//! cheaper mode that is not built yet (see migration 0016).
//!
//! # Why `&mut PgConnection` and not an executor
//!
//! Everything else in this crate is generic over [`sqlx::PgExecutor`], which
//! accepts `&PgPool`. Here that would be a trap: a pool runs each statement in
//! its own implicit transaction, so the lock would be released the moment the
//! `UPDATE` returned and the number would be burned by a later rollback —
//! silently, and only under load.
//!
//! Taking a connection makes the caller hold one, and the doc comment on
//! [`allocate`] says the rest: it belongs in the same transaction as the
//! `INSERT` of the document it numbers.
//!
//! # Never number a draft
//!
//! Allocate at confirm or post, never at create. A discarded draft would
//! otherwise leave a permanent gap, which is precisely what an auditor asks
//! about. This module cannot enforce that; it is written here because it is the
//! rule most easily lost.

use chrono::NaiveDate;
use phonix_core::identity::UserId;
use phonix_core::numbering::{NumberContext, NumberSeries, Pattern, ResetPeriod};
use sqlx::{FromRow, PgConnection, PgExecutor, Row};

use crate::error::DbError;

/// Which sequence: an app's document type, optionally per branch or till.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SequenceKey<'a> {
    pub app_id: &'a str,
    pub doc_type: &'a str,
    /// Empty for one sequence across the whole workspace.
    pub scope_key: &'a str,
}

impl<'a> SequenceKey<'a> {
    /// One sequence for the workspace, unscoped.
    pub const fn new(app_id: &'a str, doc_type: &'a str) -> Self {
        Self {
            app_id,
            doc_type,
            scope_key: "",
        }
    }

    #[must_use]
    pub const fn scoped(mut self, scope_key: &'a str) -> Self {
        self.scope_key = scope_key;
        self
    }
}

/// A configured sequence, as a settings screen sees it.
///
/// The shape under the name this crate has always used. It moved to
/// `phonix-core` so that it can cross the wire: the screen that edits a series
/// runs in the browser, and it needs the counter to explain why a format
/// change is being refused.
pub type SequenceRow = NumberSeries;

/// One sequence row.
///
/// A local wrapper, because `FromRow` belongs to sqlx and [`NumberSeries`]
/// belongs to `phonix-core` - the orphan rule, and the reason every read of a
/// shared type in this crate is a newtype.
struct SequenceRowDecode(SequenceRow);

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for SequenceRowDecode {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let raw_pattern: String = row.try_get("pattern")?;
        let raw_period: String = row.try_get("reset_period")?;

        // Both refused rather than defaulted. A pattern this build cannot parse
        // would render as something nobody chose, and a reset period falling
        // back to `Never` would silently stop a sequence resetting - the kind of
        // error found a year later, in the numbers already issued.
        let pattern = Pattern::parse(&raw_pattern).map_err(|err| sqlx::Error::ColumnDecode {
            index: "pattern".to_owned(),
            source: Box::new(err),
        })?;
        let reset_period =
            ResetPeriod::parse(&raw_period).ok_or_else(|| sqlx::Error::ColumnDecode {
                index: "reset_period".to_owned(),
                source: format!("unrecognised reset period '{raw_period}'").into(),
            })?;

        Ok(Self(NumberSeries {
            id: row.try_get("id")?,
            app_id: row.try_get("app_id")?,
            doc_type: row.try_get("doc_type")?,
            scope_key: row.try_get("scope_key")?,
            pattern,
            reset_period,
            period_key: row.try_get("period_key")?,
            counter: row.try_get("counter")?,
            start_at: row.try_get("start_at")?,
            is_active: row.try_get("is_active")?,
            updated_at: row.try_get("updated_at")?,
            updated_by: row.try_get("updated_by")?,
        }))
    }
}

/// A number, and the counter behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allocation {
    /// What goes on the document.
    pub number: String,
    /// The raw counter, for the app's own unique index and for support
    /// questions that start "which one was the fortieth".
    pub counter: i64,
    /// The period it was issued into.
    pub period_key: String,
}

/// Take the next number for a document.
///
/// **Call this inside the transaction that inserts the document**, and take the
/// `&mut PgConnection` from that transaction. Anywhere else and the gap-free
/// guarantee is gone — see the module documentation for why the signature is
/// what it is.
///
/// `on` is the **document's own date**, not today. A document backdated into
/// last month belongs in last month's period, which is what makes a monthly
/// reset and the number it prints agree with each other.
///
/// `fiscal_year_start_month` comes from `organization_profile`. It matters only
/// for a fiscal-year reset and for a `{FY}` token, and passing it in rather than
/// reading it here keeps this one statement.
pub async fn allocate(
    conn: &mut PgConnection,
    key: SequenceKey<'_>,
    on: NaiveDate,
    fiscal_year_start_month: u8,
) -> Result<Allocation, DbError> {
    // The period is computed here rather than in SQL so there is one
    // implementation of "which period is this", shared with the wasm bundle
    // that previews a pattern. Postgres would need its own, and two of them
    // would agree until a fiscal year did not start in January.
    let reset_period = current_reset_period(&mut *conn, key).await?;
    let period_key = reset_period.key_for(on, fiscal_year_start_month);

    // The whole allocation, including the period reset, in one statement. The
    // CASE is what makes a year boundary unable to interleave with a document:
    // there is no window between deciding to reset and issuing the number.
    //
    // `counter >= start_at` is the third case the two-branch version misses. On
    // a fresh row the counter is 0, so a sequence configured to begin at 5000
    // would otherwise issue 1 - and raising `start_at` past numbers already
    // issued, which is exactly how an administrator is meant to move a sequence
    // on, would do nothing at all.
    let row = sqlx::query(
        "UPDATE number_sequences
            SET counter    = CASE
                                 WHEN period_key = $4 AND counter >= start_at
                                 THEN counter + 1
                                 ELSE start_at
                             END,
                period_key = $4
          WHERE app_id = $1 AND doc_type = $2 AND scope_key = $3 AND is_active
      RETURNING counter, pattern",
    )
    .bind(key.app_id)
    .bind(key.doc_type)
    .bind(key.scope_key)
    .bind(&period_key)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::Query)?;

    // No row means no sequence, or one switched off. Both are refusals rather
    // than a number invented on the spot: a document that numbers itself
    // outside the sequence is the gap the sequence exists to prevent.
    let row = row.ok_or_else(|| DbError::UnusableSequence {
        app_id: key.app_id.to_owned(),
        doc_type: key.doc_type.to_owned(),
        scope_key: key.scope_key.to_owned(),
    })?;

    let counter: i64 = row.try_get("counter").map_err(DbError::Query)?;
    let raw_pattern: String = row.try_get("pattern").map_err(DbError::Query)?;
    let pattern = Pattern::parse(&raw_pattern)
        .map_err(|err| DbError::CorruptRow(format!("number pattern '{raw_pattern}': {err}")))?;

    let number = pattern.render(NumberContext {
        counter,
        on,
        scope: key.scope_key,
        fiscal_year_start_month,
    });

    Ok(Allocation {
        number,
        counter,
        period_key,
    })
}

/// The reset period of one sequence, read before the allocating `UPDATE`.
///
/// A second statement, which looks like it wants folding into the first. It
/// cannot be: the period key is an *input* to that `UPDATE` and computing it
/// needs the reset period, so something has to read it first. Reading it in the
/// same transaction is enough - an administrator changing the reset period
/// concurrently blocks on the row this is about to lock.
async fn current_reset_period(
    conn: &mut PgConnection,
    key: SequenceKey<'_>,
) -> Result<ResetPeriod, DbError> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT reset_period FROM number_sequences
          WHERE app_id = $1 AND doc_type = $2 AND scope_key = $3 AND is_active",
    )
    .bind(key.app_id)
    .bind(key.doc_type)
    .bind(key.scope_key)
    .fetch_optional(conn)
    .await
    .map_err(DbError::Query)?;

    let raw = raw.ok_or_else(|| DbError::UnusableSequence {
        app_id: key.app_id.to_owned(),
        doc_type: key.doc_type.to_owned(),
        scope_key: key.scope_key.to_owned(),
    })?;

    ResetPeriod::parse(&raw)
        .ok_or_else(|| DbError::CorruptRow(format!("unrecognised reset period '{raw}'")))
}

/// Every sequence, or every sequence one app owns.
///
/// The column list is spelled out here and again in [`find`] rather than shared
/// through a `format!`: sqlx 0.9 takes only `&'static str` unless the SQL is
/// explicitly asserted safe, and a repository has no business asserting that
/// over a string it assembled.
pub async fn list<'e, E>(executor: E, app_id: Option<&str>) -> Result<Vec<SequenceRow>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query_as::<_, SequenceRowDecode>(
        "SELECT id, app_id, doc_type, scope_key, pattern, reset_period,
                period_key, counter, start_at, is_active, updated_at, updated_by
           FROM number_sequences
          WHERE ($1::text IS NULL OR app_id = $1)
          ORDER BY app_id, doc_type, scope_key",
    )
    .bind(app_id)
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(rows.into_iter().map(|row| row.0).collect())
}

/// One sequence, by key.
pub async fn find<'e, E>(executor: E, key: SequenceKey<'_>) -> Result<Option<SequenceRow>, DbError>
where
    E: PgExecutor<'e>,
{
    let row = sqlx::query_as::<_, SequenceRowDecode>(
        "SELECT id, app_id, doc_type, scope_key, pattern, reset_period,
                period_key, counter, start_at, is_active, updated_at, updated_by
           FROM number_sequences
          WHERE app_id = $1 AND doc_type = $2 AND scope_key = $3",
    )
    .bind(key.app_id)
    .bind(key.doc_type)
    .bind(key.scope_key)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(row.map(|row| row.0))
}

/// What an app's manifest declares for one document type.
#[derive(Debug, Clone)]
pub struct SequenceDefinition<'a> {
    pub key: SequenceKey<'a>,
    pub pattern: &'a Pattern,
    pub reset_period: ResetPeriod,
    pub start_at: i64,
}

/// Create the sequences an app declares, leaving any that already exist alone.
///
/// `DO NOTHING` and not `DO UPDATE`, which is the whole point: installing an app
/// a second time, or re-running the installer after an upgrade, must not put
/// back the pattern the tenant edited or reset a counter that has already issued
/// numbers. A default is what a workspace starts with, not what it is held to.
pub async fn install_defaults<'e, E>(
    executor: E,
    definitions: &[SequenceDefinition<'_>],
) -> Result<u64, DbError>
where
    E: PgExecutor<'e>,
{
    if definitions.is_empty() {
        return Ok(0);
    }

    // Unnested arrays rather than a statement per definition: one round trip,
    // and one place for the conflict rule to be written.
    let app_ids: Vec<&str> = definitions.iter().map(|def| def.key.app_id).collect();
    let doc_types: Vec<&str> = definitions.iter().map(|def| def.key.doc_type).collect();
    let scopes: Vec<&str> = definitions.iter().map(|def| def.key.scope_key).collect();
    let patterns: Vec<&str> = definitions.iter().map(|def| def.pattern.as_str()).collect();
    let periods: Vec<&str> = definitions
        .iter()
        .map(|def| def.reset_period.as_str())
        .collect();
    let starts: Vec<i64> = definitions.iter().map(|def| def.start_at).collect();

    // The one statement in this module that qualifies its table. Everything
    // else runs on a request connection, whose search path is rooted at `core`;
    // this runs from the *installer*, on a pool rooted at the app being
    // installed - where `number_sequences` would not resolve at all.
    let affected = sqlx::query(
        "INSERT INTO core.number_sequences
             (app_id, doc_type, scope_key, pattern, reset_period, start_at)
         SELECT * FROM unnest($1::text[], $2::text[], $3::text[], $4::text[], $5::text[], $6::bigint[])
         ON CONFLICT (app_id, doc_type, scope_key) DO NOTHING",
    )
    .bind(&app_ids)
    .bind(&doc_types)
    .bind(&scopes)
    .bind(&patterns)
    .bind(&periods)
    .bind(&starts)
    .execute(executor)
    .await
    .map_err(DbError::Query)?
    .rows_affected();

    Ok(affected)
}

/// Create the sequences an app's configuration file declares.
///
/// The bridge between `config/numbering/<app_id>.toml` and the table, and the
/// only place the two shapes meet. Both callers reach it: the installer, which
/// runs as part of migrating an app, and
/// [`phonix_services::numbering::install`], which is the same act performed
/// deliberately. One implementation, because a second would be a second place
/// for the `DO NOTHING` rule to be forgotten.
pub async fn install_from_config<'e, E>(
    executor: E,
    app_id: &str,
    series: &[phonix_config::numbering::Series],
) -> Result<u64, DbError>
where
    E: PgExecutor<'e>,
{
    // Borrowed rather than cloned, so the definitions cannot drift from the
    // file they came from.
    let definitions: Vec<SequenceDefinition<'_>> = series
        .iter()
        .map(|entry| SequenceDefinition {
            key: SequenceKey {
                app_id,
                doc_type: &entry.doc_type,
                scope_key: &entry.scope,
            },
            pattern: &entry.mask,
            reset_period: entry.reset,
            start_at: entry.start_at,
        })
        .collect();

    install_defaults(executor, &definitions).await
}

/// What a settings screen may change.
///
/// Not the counter and not the period key. Those are the sequence's own record
/// of what it has already handed out, and editing them directly is how a number
/// gets issued twice. Moving a sequence on is [`SequenceUpdate::start_at`],
/// which the allocation honours the next time it runs.
#[derive(Debug, Clone)]
pub struct SequenceUpdate<'a> {
    pub pattern: &'a Pattern,
    pub reset_period: ResetPeriod,
    pub start_at: i64,
    pub is_active: bool,
    pub updated_by: Option<UserId>,
}

/// Change one sequence's settings. `false` when there is no such sequence.
///
/// **The caller has to guard this.** Narrowing a pattern or changing the reset
/// period mid-period can collide with numbers already issued: an invoice numbered
/// `INV-2026-000041` and a pattern changed to `INV-{NNNN}` will reissue `0042` in
/// a shape that no longer distinguishes it from last year's. The service layer
/// either raises `start_at` past the highest number already issued or refuses
/// the edit; this function does as it is told.
///
/// Note which direction is the dangerous one. **Raising** `start_at` moves the
/// sequence on, and is the supported way to do it: the allocation takes
/// `start_at` whenever the counter is behind it. **Lowering** it changes nothing
/// while a period is running, because the counter is already ahead — it only
/// decides where the *next* period opens. So there is nothing to refuse here;
/// the risk is in the pattern, and that is the caller's to weigh.
pub async fn update<'e, E>(
    executor: E,
    key: SequenceKey<'_>,
    update: SequenceUpdate<'_>,
) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let affected = sqlx::query(
        "UPDATE number_sequences
            SET pattern      = $4,
                reset_period = $5,
                start_at     = $6,
                is_active    = $7,
                updated_at   = now(),
                updated_by   = $8
          WHERE app_id = $1 AND doc_type = $2 AND scope_key = $3",
    )
    .bind(key.app_id)
    .bind(key.doc_type)
    .bind(key.scope_key)
    .bind(update.pattern.as_str())
    .bind(update.reset_period.as_str())
    .bind(update.start_at)
    .bind(update.is_active)
    .bind(update.updated_by)
    .execute(executor)
    .await
    .map_err(DbError::Query)?
    .rows_affected();

    Ok(affected > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_defaults_to_one_sequence_for_the_workspace() {
        let key = SequenceKey::new("books", "invoice");
        assert_eq!(key.scope_key, "");
        assert_eq!(key.scoped("NBO").scope_key, "NBO");
        // Scoping does not change which document type it is.
        assert_eq!(key.scoped("NBO").doc_type, "invoice");
    }
}
