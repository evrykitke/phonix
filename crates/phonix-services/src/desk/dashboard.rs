//! The estate at a glance: what Desk knows about the platform as a whole.
//!
//! Every other Desk screen answers a question about one thing - this workspace,
//! this account, this day's audit rows. This one answers questions about the
//! shape of the estate: how many workspaces there are, how many are actually
//! serving, how many are behind the build, and when the licences run out.
//!
//! # It costs one catalog read and one audit read, and nothing else
//!
//! Every workspace number here comes out of `workspace::list`, which the
//! catalog already answers in one query with the licence joined on. Nothing
//! opens a tenant pool - see [`crate::desk::queues`] for why that is a page of
//! its own - so this stays cheap enough to be the page somebody lands on.
//!
//! # It reads no business data
//!
//! ADR 0005 section 6. Everything counted here is a fact about a workspace as
//! an *object*: its status, its schema version, when it was created, whether
//! something authorizes it. Nothing inside one is read, and there is no query
//! here that could be widened into reading it.

use chrono::{DateTime, Datelike, Months, NaiveDate, Utc};
use phonix_core::TenantStatus;
use phonix_db::desk::audit;
use phonix_db::tenancy::catalog::{Catalog, TenantRecord};

use crate::error::ServiceResult;

/// How far ahead "expiring soon" looks.
///
/// A month, because that is the notice period somebody can actually act on:
/// long enough to reach whoever pays and short enough that the number is small
/// and therefore read. A licence with no end date is never counted here - it is
/// not expiring, it is perpetual, and conflating the two would make the most
/// deliberate licences look like the most urgent ones.
pub const EXPIRING_SOON_DAYS: i64 = 30;

/// How many months of history the "created" chart covers.
pub const CREATED_MONTHS: u32 = 12;

/// How many months ahead the licence runway covers.
pub const RUNWAY_MONTHS: u32 = 6;

/// How many days of desk activity the chart covers.
pub const ACTIVITY_DAYS: i64 = 30;

/// One period, and how many things fell in it.
///
/// Carries the period's first day rather than a label: naming a month is a
/// formatting decision, and a use case that made it would be deciding what a
/// screen says. The adapter formats it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bucket {
    pub start: NaiveDate,
    pub value: i64,
}

/// The counts every tile on the dashboard shows.
///
/// One pass over the workspace list fills all of it. They are separate fields
/// rather than a map because each one is a different question, and a caller
/// that asked for `tally.get("serving")` could ask for a key that does not
/// exist.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tally {
    pub total: usize,

    // --- by status ---
    pub active: usize,
    pub provisioning: usize,
    pub suspended: usize,
    pub archived: usize,

    /// Both halves agreeing: an active status *and* something authorizing it.
    /// The only number here that means "a customer can use it right now".
    pub serving: usize,

    // --- schema ---
    pub outdated: usize,

    // --- licences ---
    /// Active, but the licence half refuses. After migration 0005's backfill
    /// this can only be a workspace created since, which is exactly the one
    /// somebody has to look at.
    pub unlicensed: usize,
    /// A current licence that ends inside [`EXPIRING_SOON_DAYS`].
    pub expiring_soon: usize,
    /// A current licence with no end date. Deliberate, and not a problem.
    pub perpetual: usize,
}

/// Everything the dashboard draws.
pub struct Dashboard {
    pub tally: Tally,
    /// Workspaces created per month, oldest first, zero-filled.
    pub created: Vec<Bucket>,
    /// Current licences by the month they end in, this month first. A licence
    /// with no end date is in none of these - it is [`Tally::perpetual`].
    pub runway: Vec<Bucket>,
    /// Desk audit rows per day, oldest first, zero-filled.
    pub activity: Vec<Bucket>,
}

/// Read the catalog once, count everything, and bucket the three series.
pub async fn compose(catalog: &Catalog) -> ServiceResult<Dashboard> {
    let tenants = workspace_list(catalog).await?;
    let now = Utc::now();

    let since = now - chrono::Duration::days(ACTIVITY_DAYS - 1);
    let daily = audit::activity_by_day(catalog.pool(), start_of_day(since)).await?;

    let latest = phonix_db::tenancy::schema_fingerprint();

    Ok(Dashboard {
        tally: tally(&tenants, &latest, now),
        created: created_by_month(&tenants, now),
        runway: licence_runway(&tenants, now),
        activity: activity_by_day(&daily, now),
    })
}

/// Named so this module reads the catalog the way every other desk use case
/// does, rather than reaching for the repository directly.
async fn workspace_list(catalog: &Catalog) -> ServiceResult<Vec<TenantRecord>> {
    super::workspace::list(catalog).await
}

fn tally(tenants: &[TenantRecord], latest: &str, now: DateTime<Utc>) -> Tally {
    let mut tally = Tally {
        total: tenants.len(),
        ..Tally::default()
    };

    let soon = now + chrono::Duration::days(EXPIRING_SOON_DAYS);

    for tenant in tenants {
        match tenant.status {
            TenantStatus::Active => tally.active += 1,
            TenantStatus::Provisioning => tally.provisioning += 1,
            TenantStatus::Suspended => tally.suspended += 1,
            TenantStatus::Archived => tally.archived += 1,
        }

        if tenant.serves_traffic() {
            tally.serving += 1;
        }

        // The same rule the workspace list uses: a workspace still being
        // provisioned has no schema version yet and is not "behind".
        if tenant.status != TenantStatus::Provisioning
            && tenant.schema_version.as_deref() != Some(latest)
        {
            tally.outdated += 1;
        }

        if tenant.status == TenantStatus::Active && tenant.licence_problem().is_some() {
            tally.unlicensed += 1;
        }

        // Only a licence that authorizes right now can be expiring or
        // perpetual. One already lapsed is counted as `unlicensed` above and
        // must not also appear as "expiring soon", which would read as a
        // warning about something that has already happened.
        if let Some(licence) = tenant.licence.as_ref().filter(|l| l.is_current_at(now)) {
            match licence.valid_until {
                None => tally.perpetual += 1,
                Some(end) if end <= soon => tally.expiring_soon += 1,
                Some(_) => {}
            }
        }
    }

    tally
}

/// Workspaces created per month, over the last [`CREATED_MONTHS`] months.
///
/// Zero-filled, and the window is fixed rather than "since the first
/// workspace": a bar chart whose x-axis silently changes length as the estate
/// ages is a chart nobody can compare against last week's screenshot.
fn created_by_month(tenants: &[TenantRecord], now: DateTime<Utc>) -> Vec<Bucket> {
    let first = month_start(now.date_naive())
        .checked_sub_months(Months::new(CREATED_MONTHS - 1))
        .unwrap_or_else(|| month_start(now.date_naive()));

    let mut buckets = months_from(first, CREATED_MONTHS);

    for tenant in tenants {
        let start = month_start(tenant.created_at.date_naive());
        if let Some(bucket) = buckets.iter_mut().find(|b| b.start == start) {
            bucket.value += 1;
        }
    }

    buckets
}

/// Current licences by the month they run out in, starting this month.
///
/// Anything ending further out than [`RUNWAY_MONTHS`] is not counted rather
/// than piled into the last bar: a final column holding "and everything after"
/// is a column whose height means something different from its neighbours'.
fn licence_runway(tenants: &[TenantRecord], now: DateTime<Utc>) -> Vec<Bucket> {
    let mut buckets = months_from(month_start(now.date_naive()), RUNWAY_MONTHS);

    for tenant in tenants {
        let Some(licence) = tenant.licence.as_ref().filter(|l| l.is_current_at(now)) else {
            continue;
        };
        let Some(end) = licence.valid_until else {
            continue;
        };

        let start = month_start(end.date_naive());
        if let Some(bucket) = buckets.iter_mut().find(|b| b.start == start) {
            bucket.value += 1;
        }
    }

    buckets
}

/// Desk audit rows per day, zero-filled across the whole window.
///
/// The zero-fill happens here rather than in SQL because a quiet day is the
/// query's *absence* of a row and the chart's *presence* of a zero, and the
/// window's length is the chart's business - see [`audit::activity_by_day`].
fn activity_by_day(daily: &[audit::DailyCount], now: DateTime<Utc>) -> Vec<Bucket> {
    let today = now.date_naive();
    let first = today - chrono::Duration::days(ACTIVITY_DAYS - 1);

    (0..ACTIVITY_DAYS)
        .filter_map(|offset| first.checked_add_signed(chrono::Duration::days(offset)))
        .map(|day| Bucket {
            start: day,
            value: daily
                .iter()
                .find(|count| count.day == day)
                .map(|count| count.entries)
                .unwrap_or(0),
        })
        .collect()
}

/// `count` consecutive months, starting at `first`, all zero.
fn months_from(first: NaiveDate, count: u32) -> Vec<Bucket> {
    (0..count)
        .filter_map(|offset| first.checked_add_months(Months::new(offset)))
        .map(|start| Bucket { start, value: 0 })
        .collect()
}

/// The first of the month a date falls in.
fn month_start(date: NaiveDate) -> NaiveDate {
    date.with_day(1).unwrap_or(date)
}

/// Midnight UTC on the day an instant falls in.
fn start_of_day(at: DateTime<Utc>) -> DateTime<Utc> {
    at.date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|naive| naive.and_utc())
        .unwrap_or(at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use phonix_core::tenant::licence::{Licence, LicenceState};

    fn at(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap()
    }

    fn on(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn licence(state: LicenceState, until: Option<DateTime<Utc>>) -> Licence {
        Licence {
            state,
            valid_from: at(2020, 1, 1),
            valid_until: until,
            note: None,
            updated_at: at(2020, 1, 1),
            updated_by: None,
        }
    }

    fn workspace(
        status: TenantStatus,
        created: DateTime<Utc>,
        schema: Option<&str>,
        licence: Option<Licence>,
    ) -> TenantRecord {
        TenantRecord {
            id: uuid::Uuid::new_v4(),
            slug: phonix_core::TenantSlug::parse("acme").unwrap(),
            display_name: "Acme".to_owned(),
            database_name: "phonix_tenant_acme".to_owned(),
            status,
            schema_version: schema.map(str::to_owned),
            owner_email: None,
            onboarded_at: None,
            created_at: created,
            licence,
        }
    }

    #[test]
    fn a_workspace_is_counted_once_under_its_status() {
        let now = at(2026, 9, 3);
        let tenants = vec![
            workspace(TenantStatus::Active, at(2026, 1, 1), Some("v1"), Some(licence(LicenceState::Licensed, None))),
            workspace(TenantStatus::Suspended, at(2026, 1, 1), Some("v1"), Some(licence(LicenceState::Licensed, None))),
            workspace(TenantStatus::Provisioning, at(2026, 1, 1), None, None),
        ];

        let tally = tally(&tenants, "v1", now);

        assert_eq!(tally.total, 3);
        assert_eq!(tally.active, 1);
        assert_eq!(tally.suspended, 1);
        assert_eq!(tally.provisioning, 1);
        assert_eq!(tally.archived, 0);
        // Only the active one has both halves agreeing.
        assert_eq!(tally.serving, 1);
    }

    /// A workspace mid-provisioning has no schema version yet, and calling that
    /// "behind the build" would put a permanent number on the dashboard that no
    /// migration could ever clear.
    #[test]
    fn a_provisioning_workspace_is_not_counted_as_behind() {
        let now = at(2026, 9, 3);
        let tenants = vec![
            workspace(TenantStatus::Provisioning, at(2026, 1, 1), None, None),
            workspace(TenantStatus::Active, at(2026, 1, 1), Some("old"), Some(licence(LicenceState::Licensed, None))),
        ];

        assert_eq!(tally(&tenants, "v1", now).outdated, 1);
    }

    /// The distinction the licence tiles exist to draw: a licence that has
    /// already lapsed is a problem, one with no end date is a decision, and
    /// neither is "expiring soon".
    #[test]
    fn a_lapsed_licence_is_never_also_counted_as_expiring() {
        let now = at(2026, 9, 3);
        let tenants = vec![
            // Ended last month.
            workspace(TenantStatus::Active, at(2026, 1, 1), Some("v1"), Some(licence(LicenceState::Licensed, Some(at(2026, 8, 1))))),
            // Ends in a fortnight.
            workspace(TenantStatus::Active, at(2026, 1, 1), Some("v1"), Some(licence(LicenceState::Licensed, Some(at(2026, 9, 17))))),
            // No end at all.
            workspace(TenantStatus::Active, at(2026, 1, 1), Some("v1"), Some(licence(LicenceState::Licensed, None))),
        ];

        let tally = tally(&tenants, "v1", now);

        assert_eq!(tally.unlicensed, 1, "the lapsed one");
        assert_eq!(tally.expiring_soon, 1, "and it is not this one");
        assert_eq!(tally.perpetual, 1);
    }

    #[test]
    fn the_created_series_is_twelve_zero_filled_months_ending_this_month() {
        let now = at(2026, 9, 3);
        let tenants = vec![
            workspace(TenantStatus::Active, at(2026, 9, 2), Some("v1"), None),
            workspace(TenantStatus::Active, at(2026, 9, 30), Some("v1"), None),
            workspace(TenantStatus::Active, at(2026, 7, 15), Some("v1"), None),
            // Older than the window, and therefore in none of the bars.
            workspace(TenantStatus::Active, at(2024, 1, 1), Some("v1"), None),
        ];

        let series = created_by_month(&tenants, now);

        assert_eq!(series.len(), CREATED_MONTHS as usize);
        assert_eq!(series.first().unwrap().start, on(2025, 10, 1));
        assert_eq!(series.last().unwrap().start, on(2026, 9, 1));
        assert_eq!(series.last().unwrap().value, 2);
        assert_eq!(series.iter().find(|b| b.start == on(2026, 7, 1)).unwrap().value, 1);
        assert_eq!(series.iter().map(|b| b.value).sum::<i64>(), 3, "the 2024 one is outside");
    }

    /// A licence ending beyond the window is left out rather than heaped onto
    /// the last bar, where its height would mean something different from its
    /// neighbours'.
    #[test]
    fn the_runway_holds_only_current_licences_that_end_inside_the_window() {
        let now = at(2026, 9, 3);
        let tenants = vec![
            workspace(TenantStatus::Active, at(2026, 1, 1), Some("v1"), Some(licence(LicenceState::Licensed, Some(at(2026, 9, 20))))),
            workspace(TenantStatus::Active, at(2026, 1, 1), Some("v1"), Some(licence(LicenceState::Licensed, Some(at(2026, 11, 2))))),
            // Beyond six months.
            workspace(TenantStatus::Active, at(2026, 1, 1), Some("v1"), Some(licence(LicenceState::Licensed, Some(at(2028, 1, 1))))),
            // No end date: perpetual, so in no bar.
            workspace(TenantStatus::Active, at(2026, 1, 1), Some("v1"), Some(licence(LicenceState::Licensed, None))),
            // Already lapsed: not current, so in no bar either.
            workspace(TenantStatus::Active, at(2026, 1, 1), Some("v1"), Some(licence(LicenceState::Licensed, Some(at(2026, 1, 1))))),
        ];

        let series = licence_runway(&tenants, now);

        assert_eq!(series.len(), RUNWAY_MONTHS as usize);
        assert_eq!(series.first().unwrap().start, on(2026, 9, 1));
        assert_eq!(series.first().unwrap().value, 1);
        assert_eq!(series.iter().find(|b| b.start == on(2026, 11, 1)).unwrap().value, 1);
        assert_eq!(series.iter().map(|b| b.value).sum::<i64>(), 2);
    }

    /// A quiet day is a zero in the chart and an absent row in the query, and
    /// the chart must not close the gap by shifting the days that follow it.
    #[test]
    fn a_quiet_day_becomes_a_zero_and_does_not_shift_the_series() {
        let now = at(2026, 9, 3);
        let daily = vec![
            audit::DailyCount { day: on(2026, 9, 3), entries: 7 },
            audit::DailyCount { day: on(2026, 9, 1), entries: 2 },
        ];

        let series = activity_by_day(&daily, now);

        assert_eq!(series.len(), ACTIVITY_DAYS as usize);
        assert_eq!(series.last().unwrap().start, on(2026, 9, 3));
        assert_eq!(series.last().unwrap().value, 7);
        // The 2nd is quiet, and sits between the two that are not.
        let second = series.iter().find(|b| b.start == on(2026, 9, 2)).unwrap();
        assert_eq!(second.value, 0);
        assert_eq!(series.iter().map(|b| b.value).sum::<i64>(), 9);
    }

    #[test]
    fn a_month_starts_on_its_first_day() {
        assert_eq!(month_start(on(2026, 9, 30)), on(2026, 9, 1));
        assert_eq!(month_start(on(2026, 2, 1)), on(2026, 2, 1));
    }
}
