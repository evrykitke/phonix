//! When a counter goes back to the beginning.

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

/// Longest scope key, matching the column's CHECK.
///
/// A scope is a branch, a till, a warehouse - a short code that goes into a
/// document number. Forty characters is generous for that and short enough that
/// `{SCOPE}` cannot turn a number into a paragraph.
pub const MAX_SCOPE_LEN: usize = 40;

/// How often the counter restarts.
///
/// The reset is not a scheduled job. It happens as part of the allocation
/// itself, by comparing the period the row last issued into against the period
/// the new document falls in - so a year boundary cannot interleave with an
/// allocation, and nothing has to run at midnight for the first invoice of
/// January to be number one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetPeriod {
    /// One unbroken run for the life of the workspace.
    #[default]
    Never,
    Daily,
    Monthly,
    /// Calendar year. January to December regardless of the organization's
    /// books.
    Yearly,
    /// The organization's own financial year, which opens in
    /// `organization_profile.fiscal_year_start_month`.
    FiscalYear,
}

impl ResetPeriod {
    /// Every period, in the order a settings screen should offer them.
    pub const ALL: &'static [Self] = &[
        Self::Never,
        Self::Daily,
        Self::Monthly,
        Self::Yearly,
        Self::FiscalYear,
    ];

    /// The stored value, matching the column's CHECK constraint.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Daily => "daily",
            Self::Monthly => "monthly",
            Self::Yearly => "yearly",
            Self::FiscalYear => "fiscal_year",
        }
    }

    /// What a settings screen calls it.
    ///
    /// A key rather than the word, so the screen resolves it against whichever
    /// catalog the reader is in. `as_str` is the *stored* value and is never
    /// shown: a settings box printing `fiscal_year` is a box that reads like a
    /// column name.
    pub fn label(self) -> crate::Message {
        match self {
            Self::Never => crate::msg!("numbering.reset.never"),
            Self::Daily => crate::msg!("numbering.reset.daily"),
            Self::Monthly => crate::msg!("numbering.reset.monthly"),
            Self::Yearly => crate::msg!("numbering.reset.yearly"),
            Self::FiscalYear => crate::msg!("numbering.reset.fiscal_year"),
        }
    }

    /// Read a stored value. `None` for anything else - a row holding a period
    /// this build does not know is refused rather than defaulted, because
    /// falling back to `Never` would silently stop a sequence resetting.
    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|period| period.as_str() == raw)
    }

    /// The key identifying the period `on` falls in.
    ///
    /// Compared for equality and nothing else - it is never parsed back - so
    /// the only thing that matters is that two dates in the same period produce
    /// the same string and two dates in different periods do not.
    ///
    /// The `FY` prefix on a fiscal year is deliberate: without it a January
    /// fiscal year would produce the same key as [`Yearly`](Self::Yearly), and
    /// an administrator switching between the two mid-year would leave a stored
    /// key that accidentally matches - so the counter would carry on instead of
    /// resetting, which is the failure nobody notices until the numbers are
    /// already out.
    pub fn key_for(self, on: NaiveDate, fiscal_year_start_month: u8) -> String {
        match self {
            Self::Never => String::new(),
            Self::Daily => on.format("%Y-%m-%d").to_string(),
            Self::Monthly => on.format("%Y-%m").to_string(),
            Self::Yearly => on.format("%Y").to_string(),
            Self::FiscalYear => format!("FY{}", fiscal_year(on, fiscal_year_start_month)),
        }
    }
}

/// The financial year `on` falls in, named by the calendar year it **opens**.
///
/// A choice, not a fact: the year April 2026 to March 2027 is "2026/27" in the
/// UK, "FY2027" to the US federal government and "FY 2026-27" in India. Naming
/// it by the opening year is the one convention that degrades gracefully -
/// `fiscal_year_start_month` defaults to January, and for a January year this
/// returns exactly `{YYYY}`. Naming it by the closing year would hand a
/// January-start organization next year's number on every document, which looks
/// like a bug because it is indistinguishable from one.
///
/// An organization that names its years the other way writes `FY{YY}` against a
/// year it thinks of as the second half; that is a labelling preference, and it
/// is one line in a pattern rather than a different reset boundary.
pub fn fiscal_year(on: NaiveDate, fiscal_year_start_month: u8) -> i32 {
    // Clamped rather than trusted. The column has a CHECK and the domain type
    // validates, so a value outside 1-12 means something is already wrong -
    // and this crate cannot panic to say so.
    let start = i32::from(fiscal_year_start_month.clamp(1, 12));

    // `month()` is 1-12, so this compares like with like.
    let month = on.month() as i32;
    if month >= start {
        on.year()
    } else {
        on.year() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(year: i32, month: u32, of: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, of).unwrap()
    }

    #[test]
    fn every_period_round_trips_through_its_stored_value() {
        for period in ResetPeriod::ALL {
            assert_eq!(ResetPeriod::parse(period.as_str()), Some(*period));
        }
        assert_eq!(ResetPeriod::parse("quarterly"), None);
        assert_eq!(ResetPeriod::parse(""), None);
    }

    #[test]
    fn two_dates_in_one_period_share_a_key_and_two_in_different_ones_do_not() {
        let cases: &[(ResetPeriod, NaiveDate, NaiveDate, NaiveDate)] = &[
            // (period, a, same period as a, a different period)
            (
                ResetPeriod::Daily,
                day(2026, 8, 24),
                day(2026, 8, 24),
                day(2026, 8, 25),
            ),
            (
                ResetPeriod::Monthly,
                day(2026, 8, 1),
                day(2026, 8, 31),
                day(2026, 9, 1),
            ),
            (
                ResetPeriod::Yearly,
                day(2026, 1, 1),
                day(2026, 12, 31),
                day(2027, 1, 1),
            ),
        ];

        for (period, first, same, different) in cases {
            assert_eq!(
                period.key_for(*first, 1),
                period.key_for(*same, 1),
                "{period:?} split a period"
            );
            assert_ne!(
                period.key_for(*first, 1),
                period.key_for(*different, 1),
                "{period:?} ran two periods together"
            );
        }
    }

    #[test]
    fn never_has_one_period_for_ever() {
        assert_eq!(ResetPeriod::Never.key_for(day(2026, 8, 24), 1), "");
        assert_eq!(ResetPeriod::Never.key_for(day(2099, 1, 1), 4), "");
    }

    #[test]
    fn a_january_fiscal_year_is_the_calendar_year() {
        // The argument for naming a fiscal year by the year it opens: with the
        // default profile the two agree, so `{FY}` cannot look like a bug.
        for date in [day(2026, 1, 1), day(2026, 6, 30), day(2026, 12, 31)] {
            assert_eq!(fiscal_year(date, 1), date.year());
        }
    }

    #[test]
    fn an_april_fiscal_year_opens_in_april() {
        assert_eq!(fiscal_year(day(2026, 3, 31), 4), 2025);
        assert_eq!(fiscal_year(day(2026, 4, 1), 4), 2026);
        assert_eq!(fiscal_year(day(2027, 3, 31), 4), 2026);
        assert_eq!(fiscal_year(day(2027, 4, 1), 4), 2027);
    }

    #[test]
    fn a_fiscal_year_key_cannot_be_mistaken_for_a_calendar_one() {
        // Switching an existing sequence between the two must reset it. Equal
        // keys would mean it carried on instead.
        let date = day(2026, 6, 1);
        assert_ne!(
            ResetPeriod::FiscalYear.key_for(date, 1),
            ResetPeriod::Yearly.key_for(date, 1),
        );
        assert_eq!(ResetPeriod::FiscalYear.key_for(date, 1), "FY2026");
    }

    #[test]
    fn a_fiscal_month_outside_the_calendar_falls_back_rather_than_panicking() {
        // Only reachable through a corrupt row - and this crate compiles to
        // wasm, where a panic freezes the whole tab.
        assert_eq!(fiscal_year(day(2026, 6, 1), 0), 2026);
        assert_eq!(fiscal_year(day(2026, 6, 1), 200), 2025);
    }
}
