//! Issuing document numbers.
//!
//! Three acts, and they are deliberately three:
//!
//! * [`install`] creates the sequences an app declares in
//!   `config/numbering/<app_id>.toml`, once, at install time.
//! * [`NumberGenerator::next`] hands out the next number, inside the caller's
//!   own transaction.
//! * [`save_settings`] is the settings screen, and it is the only one that
//!   needs a permission.
//!
//! # Why a generator and not a function
//!
//! Every number needs the organization's fiscal year opening, which lives in
//! `organization_profile` and changes about once. Reading it per document would
//! be a query per document to answer a question whose answer is the same all
//! day. [`NumberGenerator::open`] reads it once; hold the generator for the
//! duration of a request, a batch, or a worker's lifetime.
//!
//! It is also where the *preview* lives, and keeping the two on one type is
//! what makes the settings screen and the posting path provably agree: they
//! render through the same [`Pattern`] with the same fiscal year.
//!
//! # The rule that is not in any type
//!
//! **Allocate at confirm or post, never at create, and never for a draft.**
//! [`NumberGenerator::next`] takes a `&mut PgConnection` so that it cannot be
//! called outside a transaction, but nothing can make it refuse to be called
//! too early. A discarded draft that took a number leaves a permanent gap, and
//! a permanent gap is what an auditor asks about.

use chrono::NaiveDate;
use phonix_config::numbering::Series;
use phonix_core::numbering::{Pattern, ResetPeriod, SAMPLE_COUNTER};
use phonix_core::permissions as names;
use phonix_db::numbering::{Allocation, SequenceKey, SequenceRow, SequenceUpdate};
use phonix_db::sqlx::{PgConnection, PgExecutor};
use phonix_db::{numbering as repo, organization};

use crate::audit::{self, Target, kinds};
use crate::caller::Caller;
use crate::error::{ServiceError, ServiceResult};

/// Issues numbers against one workspace's sequences.
///
/// Cheap to hold and cheap to copy: it is the fiscal year opening and nothing
/// else. Everything that changes lives in the database row.
#[derive(Debug, Clone, Copy)]
pub struct NumberGenerator {
    fiscal_year_start_month: u8,
}

impl NumberGenerator {
    /// Read the organization's fiscal year opening and keep it.
    pub async fn open<'e, E>(executor: E) -> ServiceResult<Self>
    where
        E: PgExecutor<'e>,
    {
        let profile = organization::load(executor).await?;
        Ok(Self {
            fiscal_year_start_month: profile.profile.fiscal_year_start_month,
        })
    }

    /// Build one from a month already in hand.
    ///
    /// For a caller that has just loaded the profile for its own reasons, and
    /// for tests.
    pub const fn with_fiscal_year_start(month: u8) -> Self {
        Self {
            fiscal_year_start_month: month,
        }
    }

    /// The month the organization's financial year opens in, 1-12.
    pub const fn fiscal_year_start_month(self) -> u8 {
        self.fiscal_year_start_month
    }

    /// Take the next number for a document.
    ///
    /// `on` is the **document's own date**, not today. A credit note backdated
    /// into last month carries last month's tokens, which is what makes a
    /// monthly reset and the number it prints agree.
    ///
    /// # This must be the document's transaction
    ///
    /// The `UPDATE` behind this takes a row lock that Postgres holds until the
    /// surrounding transaction ends. Called in the same transaction as the
    /// `INSERT`, a failed save *returns* the number and a retry cannot burn
    /// one. Called in its own, the lock is released immediately and the
    /// guarantee is gone - silently, and only under load. The `&mut
    /// PgConnection` is what stops a pool being passed by accident.
    pub async fn next(
        &self,
        conn: &mut PgConnection,
        key: SequenceKey<'_>,
        on: NaiveDate,
    ) -> ServiceResult<Allocation> {
        repo::allocate(conn, key, on, self.fiscal_year_start_month)
            .await
            .map_err(ServiceError::from)
    }

    /// What a format would produce, for a settings screen.
    ///
    /// A different act from [`next`](Self::next), and a separate function for
    /// that reason: this one is safe to show at any time, because it renders a
    /// sample counter rather than a real one. Showing a document the number it
    /// is *going* to get promises something that may not be kept.
    pub fn preview(&self, pattern: &Pattern, on: NaiveDate, scope: &str) -> String {
        pattern.preview(on, scope, self.fiscal_year_start_month)
    }

    /// The counter [`preview`](Self::preview) renders, so a screen can label it
    /// as an example.
    pub const fn sample_counter() -> i64 {
        SAMPLE_COUNTER
    }
}

/// Create the sequences an app declares, leaving any that already exist alone.
///
/// Called by the installer, with what [`phonix_config::numbering::series_for`]
/// read from the app's file. Returns how many rows were new.
///
/// Running it again is safe and is the point: a redeploy, or an upgrade that
/// adds a document type, must add the new sequence without putting back a
/// format the tenant changed or resetting a counter that has already issued
/// numbers. A default is what a workspace starts with, not what it is held to.
pub async fn install<'e, E>(executor: E, app_id: &str, series: &[Series]) -> ServiceResult<u64>
where
    E: PgExecutor<'e>,
{
    // Straight through to the repository, which is also what the installer in
    // `phonix_db::tenancy::provision` calls. One implementation of the mapping
    // from a configuration file to a row, because a second would be a second
    // place for the `DO NOTHING` rule to be forgotten.
    repo::install_from_config(executor, app_id, series)
        .await
        .map_err(ServiceError::from)
}

/// What a settings screen is asking to change.
#[derive(Debug, Clone)]
pub struct SettingsChange<'a> {
    pub pattern: &'a Pattern,
    pub reset_period: ResetPeriod,
    pub start_at: i64,
    pub is_active: bool,
}

/// How a settings save turned out.
///
/// Outcomes rather than errors, the way a wrong password is an outcome: a
/// refused edit is an expected path through a form, and modelling it as a
/// failure would make every caller unwrap something that happens all day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsOutcome {
    Saved,
    /// There is no such sequence. An app that was never installed, or a
    /// document type spelled wrongly.
    NoSuchSequence,
    /// The format was changed on a sequence that has already issued numbers,
    /// and the counter was not moved past them.
    ///
    /// `issued` is the last counter handed out. Offering to set `start_at`
    /// above it is the fix, and it is the caller's to offer - see
    /// [`save_settings`].
    WouldReissue {
        issued: i64,
    },
}

/// Change one sequence's settings.
///
/// # The guard, and why it is here rather than in the repository
///
/// Changing a format mid-period can collide with numbers already issued. A
/// workspace that has posted `INV-2026-000041` and then narrows its pattern to
/// `INV-####` will reissue `0042` in a shape that no longer distinguishes it
/// from last year's - two documents, one number, and no constraint in `core`
/// that can see it, because `core` holds no documents.
///
/// So this refuses a format change on a sequence that has already issued,
/// unless `start_at` is raised past the last counter handed out. That is the
/// supported way to move a sequence on: the allocation takes `start_at`
/// whenever the counter is behind it.
///
/// Note which direction is dangerous. **Raising** `start_at` moves the sequence
/// on. **Lowering** it changes nothing while a period is running, because the
/// counter is already ahead - it only decides where the next period opens.
///
/// The repository does as it is told; this is the judgement, and it needs a
/// caller with [`SETTINGS`](phonix_core::permissions::SETTINGS).
pub async fn save_settings(
    pool: &phonix_db::sqlx::PgPool,
    caller: &Caller,
    key: SequenceKey<'_>,
    change: SettingsChange<'_>,
) -> ServiceResult<SettingsOutcome> {
    caller.require(names::SETTINGS)?;

    let Some(current) = repo::find(pool, key).await? else {
        return Ok(SettingsOutcome::NoSuchSequence);
    };

    let reshaped = current.pattern.as_str() != change.pattern.as_str()
        || current.reset_period != change.reset_period;

    // `counter` is 0 on a sequence that has never issued, so an untouched
    // sequence can be reshaped freely - which is the common case, right after
    // installing an app.
    if reshaped && current.counter > 0 && change.start_at <= current.counter {
        return Ok(SettingsOutcome::WouldReissue {
            issued: current.counter,
        });
    }

    let saved = repo::update(
        pool,
        key,
        SequenceUpdate {
            pattern: change.pattern,
            reset_period: change.reset_period,
            start_at: change.start_at,
            is_active: change.is_active,
            updated_by: caller.user_id(),
        },
    )
    .await?;

    if !saved {
        return Ok(SettingsOutcome::NoSuchSequence);
    }

    // The one settings change that can make two documents share a number, so it
    // is recorded per sequence rather than folded into a settings blob. The
    // counter is a *fact* rather than part of the diff: it moves on its own,
    // every time a document is issued, and a diff that included it would report
    // a change nobody made.
    audit::updated(
        pool,
        caller,
        Target::new(kinds::NUMBER_SEQUENCE, current.id)
            .named(format!("{}.{}", key.app_id, key.doc_type))
            .fact("issued", current.counter),
        &Settings::of(&current),
        &Settings::from_change(&change),
    )
    .await;

    Ok(SettingsOutcome::Saved)
}

/// A sequence's settings, in the shape the audit diff records them.
///
/// The counter and the period key are deliberately absent: they are the
/// sequence's own record of what it has handed out, not settings, and
/// including them would make every allocation look like an edit.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct Settings {
    pattern: String,
    reset_period: &'static str,
    start_at: i64,
    is_active: bool,
}

impl Settings {
    fn of(row: &SequenceRow) -> Self {
        Self {
            pattern: row.pattern.as_str().to_owned(),
            reset_period: row.reset_period.as_str(),
            start_at: row.start_at,
            is_active: row.is_active,
        }
    }

    fn from_change(change: &SettingsChange<'_>) -> Self {
        Self {
            pattern: change.pattern.as_str().to_owned(),
            reset_period: change.reset_period.as_str(),
            start_at: change.start_at,
            is_active: change.is_active,
        }
    }
}

/// Save what a settings form submitted.
///
/// The screen's door into [`save_settings`], and the one that turns a mask
/// somebody typed into a [`Pattern`]. Kept apart from `save_settings` so the
/// parsing happens once, here, rather than at every call site - and so that a
/// mask that does not parse comes back as an outcome the form can render
/// beside the box rather than as an error at the top of the page.
pub async fn apply_settings(
    pool: &phonix_db::sqlx::PgPool,
    caller: &Caller,
    settings: &phonix_core::numbering::SeriesSettings,
) -> ServiceResult<phonix_core::numbering::SeriesSaved> {
    use phonix_core::numbering::SeriesSaved;

    let pattern = match Pattern::parse(&settings.pattern) {
        Ok(pattern) => pattern,
        // The parser's own words: it knows whether the mask was unbalanced, had
        // no counter in it, or mixed the two spellings, and each of those is a
        // different thing to fix.
        Err(err) => return Ok(SeriesSaved::BadPattern(err.to_string())),
    };

    let key = SequenceKey {
        app_id: &settings.app_id,
        doc_type: &settings.doc_type,
        scope_key: &settings.scope_key,
    };

    let outcome = save_settings(
        pool,
        caller,
        key,
        SettingsChange {
            pattern: &pattern,
            reset_period: settings.reset_period,
            start_at: settings.start_at,
            is_active: settings.is_active,
        },
    )
    .await?;

    Ok(match outcome {
        SettingsOutcome::NoSuchSequence => SeriesSaved::NoSuchSeries,
        SettingsOutcome::WouldReissue { issued } => SeriesSaved::WouldReissue { issued },
        // Re-read rather than the draft echoed back, for the reason every other
        // save in this application re-reads: the row carries a counter and a
        // period key the form never sent, and a screen showing the draft would
        // show a series that had lost them.
        SettingsOutcome::Saved => match repo::find(pool, key).await? {
            Some(series) => SeriesSaved::Saved(Box::new(series)),
            None => SeriesSaved::NoSuchSeries,
        },
    })
}

/// Every sequence a settings screen should list, or one app's.
pub async fn list<'e, E>(
    executor: E,
    caller: &Caller,
    app_id: Option<&str>,
) -> ServiceResult<Vec<SequenceRow>>
where
    E: PgExecutor<'e>,
{
    caller.require(names::SETTINGS)?;
    repo::list(executor, app_id)
        .await
        .map_err(ServiceError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(year: i32, month: u32, of: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, of).expect("a real date")
    }

    #[test]
    fn a_preview_and_a_real_number_share_one_fiscal_year() {
        // The reason both live on this type. A settings screen that resolved
        // the fiscal year differently from the posting path would show a
        // format that is right and a number that is wrong.
        let generator = NumberGenerator::with_fiscal_year_start(4);
        let pattern = Pattern::parse("INV-{FY}-#####").expect("a valid mask");

        assert_eq!(
            generator.preview(&pattern, day(2026, 3, 31), ""),
            "INV-2025-00042"
        );
        assert_eq!(
            generator.preview(&pattern, day(2026, 4, 1), ""),
            "INV-2026-00042"
        );
    }

    #[test]
    fn a_preview_is_obviously_an_example() {
        let generator = NumberGenerator::with_fiscal_year_start(1);
        let pattern = Pattern::parse("#-#####-####").expect("a valid mask");

        let preview = generator.preview(&pattern, day(2026, 8, 25), "");
        assert_eq!(preview, "0-00000-0042");
        assert!(preview.contains(&NumberGenerator::sample_counter().to_string()));
    }
}
