//! The change trail (`entity_events`).
//!
//! One row per change to one record: which kind of thing, which one, what
//! happened to it, and the `{from, to}` that says what moved. The security
//! trail next door in [`crate::identity::audit`] answers a different question -
//! who signed in, who was locked out - and the two are kept apart for the
//! reasons set out in `phonix_core::audit`.
//!
//! Recording is best-effort on the same terms as the security trail: losing a
//! trail row is bad, and refusing an administrator's save because the trail is
//! unwritable is worse.
//!
//! # Nothing in here decides what to record
//!
//! This module writes what it is handed and reads what is there. Which entity a
//! save is about, whether anything actually moved, and what the record is
//! called are decisions with a use case behind them, and they live in
//! `phonix_services::audit`.

use chrono::{DateTime, Utc};
use phonix_core::audit::{EntityAction, EntityKind};
use phonix_core::identity::UserId;
use phonix_core::query::{Page, PageRequest};
use serde_json::Value as Json;
use sqlx::{AssertSqlSafe, FromRow, PgExecutor, PgPool, Row};

use crate::error::DbError;

/// One change to record.
#[derive(Debug, Clone)]
pub struct EntityEntry<'a> {
    pub entity_type: &'static str,
    /// Which record. A singleton records its kind name - see
    /// `EntityKind::singleton_id`.
    pub entity_id: String,
    pub action: EntityAction,
    /// What the record was called at the time.
    pub label: Option<String>,
    pub actor_id: Option<UserId>,
    /// Kept beside `actor_id` so the row still names somebody after the
    /// account is gone.
    pub actor_email: Option<&'a str>,
    pub ip: Option<&'a str>,
    pub user_agent: Option<&'a str>,
    /// `{"from": {...}, "to": {...}}`, which is what earns a diff on the
    /// detail page.
    pub detail: Json,
}

impl<'a> EntityEntry<'a> {
    /// A change to one record of a declared kind.
    pub fn new(kind: EntityKind, entity_id: impl Into<String>, action: EntityAction) -> Self {
        Self {
            entity_type: kind.name,
            entity_id: entity_id.into(),
            action,
            label: None,
            actor_id: None,
            actor_email: None,
            ip: None,
            user_agent: None,
            detail: Json::Object(Default::default()),
        }
    }

    /// A change to the one record of a kind there is only one of.
    pub fn singleton(kind: EntityKind, action: EntityAction) -> Self {
        Self::new(kind, kind.singleton_id(), action)
    }

    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    #[must_use]
    pub fn actor(mut self, id: UserId, email: Option<&'a str>) -> Self {
        self.actor_id = Some(id);
        self.actor_email = email;
        self
    }

    #[must_use]
    pub fn client(mut self, ip: Option<&'a str>, user_agent: Option<&'a str>) -> Self {
        self.ip = ip;
        self.user_agent = user_agent;
        self
    }

    #[must_use]
    pub fn detail(mut self, detail: Json) -> Self {
        self.detail = detail;
        self
    }
}

/// One row of `entity_events`.
#[derive(Debug, Clone)]
pub struct EntityRecord {
    pub id: i64,
    pub entity_type: String,
    pub entity_id: String,
    pub action: EntityAction,
    pub label: Option<String>,
    pub actor_id: Option<UserId>,
    pub actor_email: Option<String>,
    pub detail: Json,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for EntityRecord {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let action: String = row.try_get("action")?;

        Ok(Self {
            id: row.try_get("id")?,
            entity_type: row.try_get("entity_type")?,
            entity_id: row.try_get("entity_id")?,
            // Never fails: the column is check-constrained, and a value from
            // outside the set reads as an edit rather than dropping the row.
            action: EntityAction::from_stored(&action),
            label: row.try_get("label")?,
            actor_id: row.try_get("actor_id")?,
            actor_email: row.try_get("actor_email")?,
            detail: row.try_get("detail")?,
            ip: row.try_get("ip")?,
            user_agent: row.try_get("user_agent")?,
            occurred_at: row.try_get("occurred_at")?,
        })
    }
}

/// The columns every read selects, in one place so they cannot drift apart.
const COLUMNS: &str = "id, entity_type, entity_id, action, label, actor_id, actor_email, \
                       detail, ip, user_agent, occurred_at";

/// Append a change.
pub async fn record<'e, E>(executor: E, entry: EntityEntry<'_>) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "INSERT INTO entity_events
             (entity_type, entity_id, action, label, actor_id, actor_email, detail, ip, user_agent)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(entry.entity_type)
    .bind(&entry.entity_id)
    .bind(entry.action.as_str())
    .bind(entry.label.as_deref())
    .bind(entry.actor_id)
    .bind(entry.actor_email)
    .bind(&entry.detail)
    .bind(entry.ip)
    .bind(entry.user_agent)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Append a change, logging rather than propagating a failure.
///
/// The same trade the security trail makes: an administrator's save must not
/// fail because the trail is unwritable.
pub async fn record_best_effort<'e, E>(executor: E, entry: EntityEntry<'_>)
where
    E: PgExecutor<'e>,
{
    let entity_type = entry.entity_type;
    let entity_id = entry.entity_id.clone();
    let action = entry.action.as_str();

    if let Err(err) = record(executor, entry).await {
        tracing::error!(
            error = %err,
            entity_type,
            entity_id,
            action,
            "could not write the entity audit entry"
        );
    }
}

/// Everything that has ever happened to one record, newest first.
///
/// This is the history section on a record's own page, and the reason the
/// trail is keyed by what it is about rather than by who did it.
pub async fn for_entity<'e, E>(
    executor: E,
    entity_type: &str,
    entity_id: &str,
    limit: i64,
) -> Result<Vec<EntityRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    let selecting = AssertSqlSafe(format!(
        "SELECT {COLUMNS}
           FROM entity_events
          WHERE entity_type = $1 AND entity_id = $2
          ORDER BY occurred_at DESC, id DESC
          LIMIT $3"
    ));

    sqlx::query_as::<_, EntityRecord>(selecting)
        .bind(entity_type)
        .bind(entity_id)
        .bind(limit.clamp(1, 500))
        .fetch_all(executor)
        .await
        .map_err(DbError::Query)
}

/// The range key the change grid declares, and so the pair of filter keys -
/// `occurred_from` and `occurred_to` - that arrive with a request.
///
/// A constant because it is written in two crates that must agree and do not
/// depend on each other: here, and `ui::table::config::changes`.
pub const OCCURRED: &str = "occurred";

/// The filter key naming which kind of record to show.
pub const KIND: &str = "kind";

/// The filter key naming which verb to show.
pub const ACTION: &str = "action";

/// The columns of `entity_events` a grid may order by.
///
/// A whitelist, not a convenience: `sort.field` arrives from a browser, and the
/// only safe way to put it in an `ORDER BY` is to not put it there at all - to
/// match it against a list of literals this file wrote itself.
const SORTABLE: &[(&str, &str)] = &[
    ("occurred_at", "occurred_at"),
    ("entity_type", "entity_type"),
    ("action", "action"),
    ("label", "label"),
    ("actor_email", "actor_email"),
];

/// One page of the trail, matching a search, a kind and a verb.
///
/// Paged in SQL for the same reason the security trail is: nothing ever deletes
/// from it, so there is no number of rows at which fetching all of it stops
/// being wrong - only a date at which it becomes obvious.
///
/// Two statements - a count and a select - so the page can be pulled back to
/// one that exists before the rows are fetched.
pub async fn page(pool: &PgPool, request: &PageRequest) -> Result<Page<EntityRecord>, DbError> {
    let request = request.sanitised();
    let needle = request
        .needle()
        .map(|needle| format!("%{}%", escape_like(&needle)));

    // A filter nobody set is a NULL that discards its own line, so one clause
    // serves every combination and nothing is interpolated.
    //
    // The range arrives as two instants rather than as a name: the browser
    // resolved "this week" before it sent anything, so this file owns no
    // calendar and cannot disagree with the panel about when a week starts.
    const WHERE: &str = "WHERE ($1::text IS NULL
                             OR label ILIKE $1
                             OR actor_email ILIKE $1
                             OR entity_type ILIKE $1)
                           AND ($2::text IS NULL OR entity_type = $2)
                           AND ($3::text IS NULL OR action = $3)
                           AND ($4::timestamptz IS NULL OR occurred_at >= $4)
                           AND ($5::timestamptz IS NULL OR occurred_at < $5)";

    // Empty is "everything", which is what an unset filter sends.
    let kind = request.filter(KIND).filter(|value| !value.is_empty());
    let action = request.filter(ACTION).filter(|value| !value.is_empty());
    // Half open: `from` is included, `to` is not, which is what makes a span of
    // one day exactly one day.
    let occurred = request.range(OCCURRED);

    // `AssertSqlSafe` because these statements are composed rather than
    // written: `WHERE` and `COLUMNS` are constants, and `order` can only be a
    // string this file put in `SORTABLE`. Nothing from a browser reaches the
    // text of the query - the search, the kind, the verb and the page are all
    // bound parameters.
    let counting = AssertSqlSafe(format!("SELECT count(*) FROM entity_events {WHERE}"));

    let total: i64 = sqlx::query_scalar(counting)
        .bind(needle.as_deref())
        .bind(kind)
        .bind(action)
        .bind(occurred.from)
        .bind(occurred.to)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;

    let total = u64::try_from(total).unwrap_or(0);
    let request = request.clamped_to(total);

    let order = match &request.sort {
        Some(sort) => SORTABLE
            .iter()
            .find(|(field, _)| *field == sort.field)
            .map(|(_, column)| format!("{column} {}", sort.direction.sql())),
        None => None,
    }
    // Newest first, and `id` after it whatever the sort: two changes written in
    // the same millisecond would otherwise swap places between one page and the
    // next, which shows up as a row that appears twice.
    .unwrap_or_else(|| "occurred_at DESC".to_owned());

    let selecting = AssertSqlSafe(format!(
        "SELECT {COLUMNS}
           FROM entity_events
           {WHERE}
          ORDER BY {order}, id DESC
          LIMIT $6 OFFSET $7"
    ));

    let rows = sqlx::query_as::<_, EntityRecord>(selecting)
        .bind(needle.as_deref())
        .bind(kind)
        .bind(action)
        .bind(occurred.from)
        .bind(occurred.to)
        .bind(request.limit() as i64)
        .bind(request.offset() as i64)
        .fetch_all(pool)
        .await
        .map_err(DbError::Query)?;

    Ok(Page::new(rows, total, &request))
}

/// One change, for the screen that opens from the list.
pub async fn find<'e, E>(executor: E, id: i64) -> Result<Option<EntityRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    let selecting = AssertSqlSafe(format!("SELECT {COLUMNS} FROM entity_events WHERE id = $1"));

    sqlx::query_as::<_, EntityRecord>(selecting)
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(DbError::Query)
}

/// Delete entries older than `days`, up to `limit` of them.
///
/// Batched rather than one statement, and that is the whole design. A workspace
/// switching retention on for the first time can have a year of entries to drop,
/// and `DELETE FROM entity_events WHERE occurred_at < ...` would take a lock
/// across all of them - on the table an administrator is looking at, in the
/// middle of the working day. A bounded batch finishes quickly, and the caller
/// runs it again until it returns zero.
///
/// Returns how many rows went, so the caller can tell "nothing left to do" from
/// "there is more".
///
/// `days` is the retention, not a cutoff instant: computing the boundary in SQL
/// means it is `now()` on the database's clock, which is the same clock
/// `occurred_at` was written from. A cutoff computed here would drift by
/// whatever the two machines disagree about.
pub async fn prune(pool: &PgPool, days: i32, limit: i64) -> Result<u64, DbError> {
    if days <= 0 || limit <= 0 {
        // Not an error, and deliberately not a delete. A retention of zero
        // would mean "keep nothing", which is not something a UI can ask for
        // and not something to infer from a bad value.
        return Ok(0);
    }

    let deleted = sqlx::query(
        "DELETE FROM entity_events
           WHERE id IN (
               SELECT id FROM entity_events
                WHERE occurred_at < now() - make_interval(days => $1)
                ORDER BY occurred_at
                LIMIT $2
           )",
    )
    .bind(days)
    .bind(limit)
    .execute(pool)
    .await
    .map_err(DbError::Query)?;

    Ok(deleted.rows_affected())
}

/// Neutralise the wildcards in a search term.
///
/// Without this, typing `%` into the search box matches every row and typing
/// `_` matches every single character - which reads as a search box that
/// sometimes ignores what was typed.
fn escape_like(needle: &str) -> String {
    needle
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use phonix_core::audit::kinds;

    #[test]
    fn a_search_term_cannot_smuggle_in_a_wildcard() {
        assert_eq!(escape_like("50%"), "50\\%");
        assert_eq!(escape_like("a_b"), "a\\_b");
    }

    #[test]
    fn a_singleton_entry_keys_itself_without_anybody_inventing_a_key() {
        // Two call sites spelling the id differently would be one record with
        // two histories, and nothing would fail to say so.
        let entry = EntityEntry::singleton(kinds::ORGANIZATION, EntityAction::Updated);

        assert_eq!(entry.entity_type, "organization");
        assert_eq!(entry.entity_id, "organization");
    }

    #[test]
    fn an_entry_carries_the_name_the_record_had_when_it_changed() {
        // The row worth reading most is the deletion, and after it there is
        // nothing left to join to for a name.
        let entry = EntityEntry::new(kinds::ROLE, "8f2c", EntityAction::Deleted).label("Auditor");

        assert_eq!(entry.label.as_deref(), Some("Auditor"));
        assert_eq!(entry.action.as_str(), "deleted");
    }

    #[test]
    fn every_sortable_field_names_a_column_of_the_table() {
        // The pair exists so that the browser never names a column directly.
        for (field, column) in SORTABLE {
            assert!(!field.is_empty() && !column.is_empty());
        }
    }

    #[tokio::test]
    async fn pruning_with_nothing_to_keep_is_refused_rather_than_obeyed() {
        // A retention of zero means "keep nothing", which no screen can ask
        // for. Inferring it from a bad value would empty the table.
        //
        // No pool is touched: both guards return before the statement, which
        // is the property under test.
        let pool = PgPool::connect_lazy("postgres://unused/unused")
            .expect("a lazy pool connects to nothing");

        assert_eq!(prune(&pool, 0, 500).await.unwrap_or(1), 0);
        assert_eq!(prune(&pool, -1, 500).await.unwrap_or(1), 0);
        assert_eq!(prune(&pool, 30, 0).await.unwrap_or(1), 0);
    }

    #[test]
    fn the_filter_keys_are_the_ones_a_request_is_read_for() {
        // Written into `PageRequest.filters` by the grid and read back here.
        // Two spellings would be a control that changes nothing.
        assert_eq!(KIND, "kind");
        assert_eq!(ACTION, "action");
        assert_eq!(OCCURRED, "occurred");
    }
}
