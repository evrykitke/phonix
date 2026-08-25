//! Document number series, declared per app in `config/numbering/<app_id>.toml`.
//!
//! An app owns the *question* — which documents it issues, and what their
//! numbers should look like out of the box. The tenant owns the *answer*: once
//! the app is installed the rows in `core.number_sequences` are theirs, and a
//! redeploy never puts back a format they changed.
//!
//! That split is why this is a file and not a table. A default is what a
//! workspace starts with, not what it is held to, and a default that lives in a
//! file can be reviewed in a pull request.
//!
//! ```toml
//! # config/numbering/books.toml
//!
//! [[series]]
//! doc_type = "sales_invoice"
//! label    = "books.doc_type.sales_invoice"
//! mask     = "INV-{YYYY}-#####"
//! reset    = "fiscal_year"
//! start_at = 1
//!
//! [[series]]
//! doc_type = "credit_note"
//! mask     = "CN-#-#####-####"
//! ```
//!
//! `mask` is a [`Pattern`], validated here rather than at first use: a format
//! typo should stop a deployment, not an invoice. `label` is an i18n key and is
//! checked against the built-in catalog for the same reason - see
//! [`all user-facing text is a key`](phonix_core::i18n).
//!
//! # Missing is not an error
//!
//! Most apps issue no numbered documents. [`series_for`] returns an empty list
//! for an app with no file, so installing one is never conditional on a file
//! existing.

use std::path::{Path, PathBuf};

use config::{Config, File, FileFormat};
use phonix_core::numbering::{Pattern, ResetPeriod, is_valid_scope};
use serde::Deserialize;

/// The directory under `config/` these files live in.
pub const DIRECTORY: &str = "numbering";

/// Longest `doc_type`, matching the column's CHECK.
pub const MAX_DOC_TYPE_LEN: usize = 40;

/// One app's file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeriesFile {
    #[serde(default)]
    series: Vec<Series>,
}

/// One document type's default numbering.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Series {
    /// What the app calls this document: `sales_invoice`, `credit_note`.
    ///
    /// Part of the sequence's identity, so it is snake_case and stable. Renaming
    /// it in a later release starts a new sequence at 1 rather than continuing
    /// the old one, which is why it is not a display name.
    pub doc_type: String,

    /// An i18n key for what a settings screen calls this. Optional: without one
    /// the screen falls back to the `doc_type`.
    #[serde(default)]
    pub label: Option<String>,

    /// The format. `INV-{YYYY}-#####`, `#-#####-####`.
    pub mask: Pattern,

    /// When the counter goes back to the beginning. Defaults to never.
    #[serde(default)]
    pub reset: ResetPeriod,

    /// The first number this sequence issues, and the first of each new period.
    #[serde(default = "one")]
    pub start_at: i64,

    /// A branch or till code, for an app that numbers per location. Empty means
    /// one sequence for the whole workspace, which is the usual case.
    #[serde(default)]
    pub scope: String,
}

const fn one() -> i64 {
    1
}

/// Read one app's series from the workspace's own `config/numbering`.
pub fn series_for(app_id: &str) -> Result<Vec<Series>, SeriesError> {
    series_from(
        crate::workspace_root().join("config").join(DIRECTORY),
        app_id,
    )
}

/// Read one app's series from an explicit directory.
///
/// Separated from [`series_for`] so tests can point at a fixture directory, the
/// same way [`crate::load_from`] is separated from [`crate::load`].
pub fn series_from(dir: impl AsRef<Path>, app_id: &str) -> Result<Vec<Series>, SeriesError> {
    let path = dir.as_ref().join(format!("{app_id}.toml"));
    if !path.is_file() {
        // Most apps issue no numbered documents.
        return Ok(Vec::new());
    }

    let file: SeriesFile = Config::builder()
        .add_source(File::from(path.clone()).format(FileFormat::Toml))
        .build()
        .map_err(|source| SeriesError::Read {
            path: path.clone(),
            source: Box::new(source),
        })?
        .try_deserialize()
        .map_err(|source| SeriesError::Read {
            path: path.clone(),
            source: Box::new(source),
        })?;

    check(&file.series, &path)?;
    Ok(file.series)
}

/// Everything that has to be true before a file is worth installing.
///
/// Checked at load rather than at first use. A format typo should stop a
/// deployment, where somebody is watching; discovering it when the first
/// invoice is posted is discovering it in front of a customer.
fn check(series: &[Series], path: &Path) -> Result<(), SeriesError> {
    let mut seen: Vec<(&str, &str)> = Vec::new();

    for entry in series {
        let doc_type = entry.doc_type.as_str();

        if !is_snake_case(doc_type) || doc_type.len() > MAX_DOC_TYPE_LEN {
            return Err(SeriesError::DocType {
                path: path.to_path_buf(),
                doc_type: doc_type.to_owned(),
            });
        }

        if !is_valid_scope(&entry.scope) {
            return Err(SeriesError::Scope {
                path: path.to_path_buf(),
                doc_type: doc_type.to_owned(),
                scope: entry.scope.clone(),
            });
        }

        if entry.start_at < 1 {
            return Err(SeriesError::StartAt {
                path: path.to_path_buf(),
                doc_type: doc_type.to_owned(),
                start_at: entry.start_at,
            });
        }

        if let Some(label) = &entry.label
            && !phonix_core::i18n::builtin_contains(label)
        {
            return Err(SeriesError::Label {
                path: path.to_path_buf(),
                doc_type: doc_type.to_owned(),
                label: label.clone(),
            });
        }

        // Two entries for one document type and scope would race to install,
        // and the loser would be silently dropped by `ON CONFLICT DO NOTHING`.
        let key = (doc_type, entry.scope.as_str());
        if seen.contains(&key) {
            return Err(SeriesError::Duplicate {
                path: path.to_path_buf(),
                doc_type: doc_type.to_owned(),
                scope: entry.scope.clone(),
            });
        }
        seen.push(key);
    }

    Ok(())
}

/// The shape `core.number_sequences` accepts for an `app_id` or a `doc_type`.
fn is_snake_case(value: &str) -> bool {
    let mut bytes = value.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Why a numbering file was refused.
///
/// Every one of these stops the process at startup. None of them is worth
/// running with: a sequence installed from a bad definition issues numbers that
/// somebody then has to explain.
#[derive(Debug, thiserror::Error)]
pub enum SeriesError {
    /// The file could not be read, parsed, or deserialised - which includes a
    /// `mask` the pattern parser refused, because `Pattern` validates on the way
    /// in.
    ///
    /// `source` is boxed: `config::ConfigError` is over a hundred bytes and
    /// would otherwise set the size of every `Result` in this module, including
    /// the ones that only ever return a short message.
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: Box<config::ConfigError>,
    },
    #[error("{path}: '{doc_type}' is not a document type name (lower case, digits, underscores)")]
    DocType { path: PathBuf, doc_type: String },
    #[error("{path}: '{doc_type}' has a scope '{scope}' that cannot go into a document number")]
    Scope {
        path: PathBuf,
        doc_type: String,
        scope: String,
    },
    #[error("{path}: '{doc_type}' starts at {start_at}; a sequence starts at 1 or later")]
    StartAt {
        path: PathBuf,
        doc_type: String,
        start_at: i64,
    },
    #[error("{path}: '{doc_type}' has label '{label}', which is not a key in the catalog")]
    Label {
        path: PathBuf,
        doc_type: String,
        label: String,
    },
    #[error("{path}: '{doc_type}' is declared twice for scope '{scope}'")]
    Duplicate {
        path: PathBuf,
        doc_type: String,
        scope: String,
    },
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Write one file into a fresh directory and read it back.
    fn load(name: &str, body: &str) -> Result<Vec<Series>, SeriesError> {
        let dir = std::env::temp_dir().join(format!(
            "phonix-numbering-{}-{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_nanos())
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).expect("scratch directory");
        fs::write(dir.join("books.toml"), body).expect("write fixture");
        let result = series_from(&dir, "books");
        let _ = fs::remove_dir_all(&dir);
        result
    }

    #[test]
    fn an_app_with_no_file_declares_no_series() {
        let series = series_from(
            std::env::temp_dir().join("phonix-numbering-absent"),
            "books",
        )
        .expect("a missing file is not an error");
        assert!(series.is_empty());
    }

    #[test]
    fn the_example_from_the_module_documentation_loads() {
        let series = load(
            "example",
            r#"
[[series]]
doc_type = "sales_invoice"
mask     = "INV-{YYYY}-#####"
reset    = "fiscal_year"
start_at = 1

[[series]]
doc_type = "credit_note"
mask     = "CN-#-#####-####"
"#,
        )
        .expect("the documented example has to load");

        assert_eq!(series.len(), 2);
        assert_eq!(series[0].doc_type, "sales_invoice");
        assert_eq!(series[0].reset, ResetPeriod::FiscalYear);
        assert_eq!(series[0].mask.counter_width(), 5);

        // The defaults are the quiet half of the format.
        assert_eq!(series[1].reset, ResetPeriod::Never);
        assert_eq!(series[1].start_at, 1);
        assert_eq!(series[1].scope, "");
        assert_eq!(series[1].mask.counter_width(), 10);
    }

    #[test]
    fn a_bad_mask_stops_the_deployment_rather_than_the_first_invoice() {
        // No counter: every document in the workspace would share one number.
        let error = load(
            "nocounter",
            "[[series]]\ndoc_type = \"x\"\nmask = \"INV-{YYYY}\"\n",
        )
        .expect_err("a mask with no counter must be refused");
        assert!(matches!(error, SeriesError::Read { .. }), "{error}");

        // And the mixed spelling, which renders a different width than it reads.
        let error = load(
            "mixed",
            "[[series]]\ndoc_type = \"x\"\nmask = \"INV #{NNNNN}\"\n",
        )
        .expect_err("a mixed mask must be refused");
        assert!(matches!(error, SeriesError::Read { .. }), "{error}");
    }

    #[test]
    fn a_document_type_has_to_be_a_name_the_column_accepts() {
        for bad in ["Sales Invoice", "sales-invoice", "1invoice", ""] {
            let body = format!("[[series]]\ndoc_type = \"{bad}\"\nmask = \"#####\"\n");
            let error = load("doctype", &body).expect_err("{bad} must be refused");
            assert!(matches!(error, SeriesError::DocType { .. }), "{error}");
        }
    }

    #[test]
    fn a_label_that_is_not_a_catalog_key_is_refused() {
        let error = load(
            "label",
            "[[series]]\ndoc_type = \"x\"\nmask = \"#####\"\nlabel = \"books.nope\"\n",
        )
        .expect_err("an unknown label key must be refused");
        assert!(matches!(error, SeriesError::Label { .. }), "{error}");

        // A key that does exist is accepted, so the check is not vacuous.
        let series = load(
            "label_ok",
            "[[series]]\ndoc_type = \"x\"\nmask = \"#####\"\nlabel = \"common.save\"\n",
        )
        .expect("a real key is fine");
        assert_eq!(series[0].label.as_deref(), Some("common.save"));
    }

    #[test]
    fn one_document_type_cannot_be_declared_twice_for_one_scope() {
        // `ON CONFLICT DO NOTHING` would drop the second silently.
        let error = load(
            "dupe",
            "[[series]]\ndoc_type = \"x\"\nmask = \"#####\"\n\n[[series]]\ndoc_type = \"x\"\nmask = \"###\"\n",
        )
        .expect_err("a duplicate must be refused");
        assert!(matches!(error, SeriesError::Duplicate { .. }), "{error}");

        // The same type in two scopes is the point of scopes, and is fine.
        let series = load(
            "scoped",
            "[[series]]\ndoc_type = \"x\"\nmask = \"#####\"\nscope = \"NBO\"\n\n[[series]]\ndoc_type = \"x\"\nmask = \"#####\"\nscope = \"MBA\"\n",
        )
        .expect("two scopes are two sequences");
        assert_eq!(series.len(), 2);
    }

    #[test]
    fn a_sequence_cannot_start_before_one() {
        let error = load(
            "start",
            "[[series]]\ndoc_type = \"x\"\nmask = \"#####\"\nstart_at = 0\n",
        )
        .expect_err("start_at = 0 must be refused");
        assert!(matches!(error, SeriesError::StartAt { .. }), "{error}");
    }

    #[test]
    fn a_key_nobody_wrote_is_a_typo_and_not_a_silent_default() {
        let error = load(
            "unknown",
            "[[series]]\ndoc_type = \"x\"\nmask = \"#####\"\nrest = \"yearly\"\n",
        )
        .expect_err("`rest` is a typo for `reset` and must not be ignored");
        assert!(matches!(error, SeriesError::Read { .. }), "{error}");
    }
}
