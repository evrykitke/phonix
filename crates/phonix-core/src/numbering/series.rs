//! A configured number series, as a settings screen sees it.
//!
//! Here rather than in the repository so that it can cross the wire: the screen
//! that edits a series runs in the browser, and it needs the counter to explain
//! why a format change is being refused.
//!
//! # The counter is shown but never edited
//!
//! [`NumberSeries::counter`] and `period_key` are the sequence's own record of
//! what it has handed out. Editing them directly is how a number gets issued
//! twice, so [`SeriesSettings`] - what a form submits - does not carry them.
//! Moving a series on is `start_at`, which the allocation honours the next time
//! it runs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::identity::UserId;
use crate::numbering::{Pattern, ResetPeriod};

/// One series, whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumberSeries {
    pub id: Uuid,
    /// The app that declared this document type. Not a foreign key: a number
    /// already issued has to stay explicable after its app is uninstalled.
    pub app_id: String,
    pub doc_type: String,
    /// Empty for one series across the whole workspace.
    pub scope_key: String,
    pub pattern: Pattern,
    pub reset_period: ResetPeriod,
    /// The period the counter is currently running in. Opaque; compared, never
    /// parsed, and never edited.
    pub period_key: String,
    /// The last number issued in `period_key`. Zero means none yet.
    pub counter: i64,
    pub start_at: i64,
    pub is_active: bool,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<UserId>,
}

impl NumberSeries {
    /// Whether this series has handed out a number in the period it is in.
    ///
    /// What decides whether a format change is safe: reshaping a series that
    /// has already issued can reissue a number in a shape that no longer
    /// distinguishes it from last year's.
    pub const fn has_issued(&self) -> bool {
        self.counter > 0
    }

    /// The key a settings list groups and sorts by.
    pub fn key(&self) -> String {
        if self.scope_key.is_empty() {
            format!("{}.{}", self.app_id, self.doc_type)
        } else {
            format!("{}.{}@{}", self.app_id, self.doc_type, self.scope_key)
        }
    }
}

/// What a settings form submits.
///
/// Deliberately not the counter and not the period key - see the module note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesSettings {
    pub app_id: String,
    pub doc_type: String,
    pub scope_key: String,
    /// As typed, so a mask that does not parse comes back as a message on the
    /// field rather than as a failure to build the form.
    pub pattern: String,
    pub reset_period: ResetPeriod,
    pub start_at: i64,
    pub is_active: bool,
}

impl SeriesSettings {
    /// Open a form on a stored series.
    pub fn of(series: &NumberSeries) -> Self {
        Self {
            app_id: series.app_id.clone(),
            doc_type: series.doc_type.clone(),
            scope_key: series.scope_key.clone(),
            pattern: series.pattern.as_str().to_owned(),
            reset_period: series.reset_period,
            start_at: series.start_at,
            is_active: series.is_active,
        }
    }
}

/// How a settings save turned out, in a shape a screen can render.
///
/// Outcomes rather than errors, the way a wrong password is an outcome: a
/// refused edit is an expected path through a form, and modelling it as a
/// failure would make every caller unwrap something that happens all day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SeriesSaved {
    /// Stored. Carries the series as it now stands.
    Saved(Box<NumberSeries>),
    /// No such series - an app that was never installed, or a document type
    /// spelled wrongly.
    NoSuchSeries,
    /// The format was changed on a series that has already issued numbers, and
    /// the counter was not moved past them.
    ///
    /// `issued` is the last number handed out. Offering to set `start_at` above
    /// it is the fix, which is why the number is here rather than in a sentence.
    WouldReissue { issued: i64 },
    /// The mask does not parse. Carries what is wrong with it.
    BadPattern(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(scope: &str, counter: i64) -> NumberSeries {
        NumberSeries {
            id: Uuid::nil(),
            app_id: "books".to_owned(),
            doc_type: "sales_invoice".to_owned(),
            scope_key: scope.to_owned(),
            pattern: Pattern::parse("INV-#####").expect("a valid mask"),
            reset_period: ResetPeriod::Never,
            period_key: String::new(),
            counter,
            start_at: 1,
            is_active: true,
            updated_at: DateTime::<Utc>::from_timestamp(0, 0).expect("the epoch"),
            updated_by: None,
        }
    }

    #[test]
    fn an_unscoped_series_is_named_by_its_app_and_document_type() {
        assert_eq!(series("", 0).key(), "books.sales_invoice");
    }

    #[test]
    fn a_scoped_series_says_which_scope() {
        // Otherwise a workspace numbering per branch has four identical rows.
        assert_eq!(series("NBO", 0).key(), "books.sales_invoice@NBO");
    }

    #[test]
    fn a_series_that_has_never_issued_can_be_reshaped_freely() {
        // The common case, right after installing an app.
        assert!(!series("", 0).has_issued());
        assert!(series("", 1).has_issued());
    }

    #[test]
    fn a_form_opens_on_the_stored_values_and_not_on_the_counter() {
        // The counter is the sequence's own record of what it handed out.
        // Editing it directly is how a number gets issued twice.
        let settings = SeriesSettings::of(&series("", 42));

        assert_eq!(settings.pattern, "INV-#####");
        assert_eq!(settings.start_at, 1);
    }
}
