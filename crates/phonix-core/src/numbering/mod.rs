//! Document numbers: the format, the period, and what a counter renders as.
//!
//! # What lives here and what does not
//!
//! This module is the *format*. Allocating the next counter is a database
//! statement — one row-locked `UPDATE ... RETURNING` in the document's own
//! transaction — and it lives in `phonix_db::numbering`. Rendering is pure, so
//! it compiles to wasm and a settings screen can preview a pattern as somebody
//! types it, with no round trip and no possibility of the preview disagreeing
//! with what the server will issue.
//!
//! # Gap-free is the whole point
//!
//! Italy, Spain, Portugal, Poland, India under GST and most of Latin America
//! require an unbroken sequence on a tax document. A missing number is an audit
//! finding, and "the save failed" is not a defence.
//!
//! That requirement is why a Postgres `SEQUENCE` is the wrong tool no matter how
//! convenient it looks: `nextval()` is deliberately non-transactional, so a
//! rollback burns the number. Correct for a surrogate key, unlawful for an
//! invoice. See `phonix_db::numbering` for what replaces it.
//!
//! # Three rules the format cannot enforce
//!
//! Worth writing here anyway, because they are what actually keeps a sequence
//! gap-free, and none of them is visible from the schema:
//!
//! 1. **Allocate inside the document's own transaction.** A failed save then
//!    returns the number automatically, and a retry cannot burn one.
//! 2. **Never number a draft.** Allocate at confirm or post. A discarded draft
//!    would otherwise leave a permanent gap — precisely the thing an auditor
//!    asks about. Drafts show their id, or the word Draft.
//! 3. **Never show a document the number it is *going* to get.** Displaying it
//!    before commit promises something that may not be kept. Previewing a
//!    *pattern* against a sample counter is a different act and an entirely
//!    safe one — see [`Pattern::preview`].

mod pattern;
mod period;
mod series;

pub use pattern::{
    MAX_COUNTER_WIDTH, MAX_PATTERN_LEN, NumberContext, Pattern, PatternError, SAMPLE_COUNTER,
    is_valid_scope,
};
pub use period::{MAX_SCOPE_LEN, ResetPeriod, fiscal_year};
pub use series::{NumberSeries, SeriesSaved, SeriesSettings};
