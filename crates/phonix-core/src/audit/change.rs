//! What changed, as a screen draws it.
//!
//! One vocabulary, used by both trails. The security trail records a policy
//! being relaxed; the change trail records a record being edited; and a reader
//! looking at either wants the same three things - which field, what it was,
//! what it is now.
//!
//! These types were once part of [`crate::identity::audit`], which is where the
//! first diff happened to be needed. They are not about identity, and leaving
//! them there would have meant `phonix_core::audit` depending on
//! `phonix_core::identity` to describe a change to an invoice.
//!
//! Nothing here is stored. A diff is presentation: freezing one into a table
//! would mean migrating history every time the wording changed. What is stored
//! is `{from, to}`, and this is what that becomes on the way out.

use serde::{Deserialize, Serialize};

/// Which way a field went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeKind {
    /// It had no value before.
    Added,
    /// It has none now.
    Removed,
    Modified,
}

/// What happened to one field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change {
    /// One value replaced another. `None` on either side means "not set",
    /// which is not the same as an empty string and must not draw as one.
    Value {
        before: Option<String>,
        after: Option<String>,
    },
    /// A collection gained and lost members.
    ///
    /// Distinct from `Value` because a permission set that gained one name out
    /// of forty is unreadable as two lists of forty, and obvious as one line.
    Members {
        added: Vec<String>,
        removed: Vec<String>,
    },
}

/// One line of a diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldChange {
    /// A dotted path into the entity: `password.min_length`. Stable, and the
    /// heading is derived from it rather than stored next to it.
    pub field: String,
    pub change: Change,
}

impl FieldChange {
    /// The path as a heading: `password.min_length` -> "Password / min length".
    pub fn label(&self) -> String {
        let opened = self
            .field
            .split('.')
            .map(|segment| segment.replace('_', " "))
            .collect::<Vec<_>>()
            .join(" / ");

        let mut label = opened;
        if let Some(first) = label.get_mut(0..1) {
            first.make_ascii_uppercase();
        }

        label
    }

    pub fn kind(&self) -> ChangeKind {
        match &self.change {
            Change::Value {
                before: None,
                after: Some(_),
            } => ChangeKind::Added,
            Change::Value {
                before: Some(_),
                after: None,
            } => ChangeKind::Removed,
            Change::Value { .. } => ChangeKind::Modified,
            Change::Members { added, removed } if removed.is_empty() && !added.is_empty() => {
                ChangeKind::Added
            }
            Change::Members { added, removed } if added.is_empty() && !removed.is_empty() => {
                ChangeKind::Removed
            }
            Change::Members { .. } => ChangeKind::Modified,
        }
    }
}

/// Something recorded beside an event that is not part of a diff: who it was
/// done to, which kind of factor, why it was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    pub label: String,
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dotted_path_reads_as_a_heading() {
        let change = FieldChange {
            field: "password.min_length".to_owned(),
            change: Change::Value {
                before: Some("8".into()),
                after: Some("12".into()),
            },
        };

        assert_eq!(change.label(), "Password / min length");
        assert_eq!(change.kind(), ChangeKind::Modified);
    }

    #[test]
    fn a_field_that_gained_or_lost_its_value_says_which() {
        let gained = FieldChange {
            field: "note".to_owned(),
            change: Change::Value {
                before: None,
                after: Some("x".into()),
            },
        };
        let lost = FieldChange {
            field: "note".to_owned(),
            change: Change::Value {
                before: Some("x".into()),
                after: None,
            },
        };

        assert_eq!(gained.kind(), ChangeKind::Added);
        assert_eq!(lost.kind(), ChangeKind::Removed);
    }

    #[test]
    fn a_set_that_only_grew_is_an_addition() {
        let grew = FieldChange {
            field: "granted".to_owned(),
            change: Change::Members {
                added: vec!["Pages.Users".into()],
                removed: Vec::new(),
            },
        };

        assert_eq!(grew.kind(), ChangeKind::Added);
    }
}
