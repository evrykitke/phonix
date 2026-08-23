//! Turning a stored `detail` object into a diff a screen may read.
//!
//! # Why a translation exists at all
//!
//! Both trails store a free-form JSONB object. Whatever use case wrote the row
//! put in whatever it thought useful - a failure reason, the previous password
//! policy, two lists of permission names - and the shape is internal,
//! unversioned, and different for almost every event.
//!
//! Shipping that object to the browser would make the screen depend on it, and
//! from then on the shape could not be changed without breaking a page that is
//! also deployed. So it is translated here, on the way out, into the two things
//! a reader actually wants:
//!
//! * a **diff**, when the row recorded a before and an after
//! * a few labelled **facts**, for everything else in the object
//!
//! # One differ, two trails
//!
//! The security trail records a policy being relaxed; the change trail records
//! a record being edited. Neither is a special case of the other, and a second
//! copy of this file would be two answers to "did this field move".
//!
//! # How a diff is recognised
//!
//! Not by the event name. Two shapes are understood, and any row written in
//! either gets a diff whether or not this file has heard of what wrote it:
//!
//! ```text
//! { "from": {..}, "to": {..} }                        the entity shape
//! { "granted": [..], "denied": [..],                  the override shape
//!   "previously_granted": [..], "previously_denied": [..] }
//! ```
//!
//! That is deliberate. A new module that records a change in the `from`/`to`
//! shape gets a working detail page without touching this file, and one that
//! invents a third shape gets a list of facts rather than a wrong diff.
//! `from`/`to` is the shape to write - see [`crate::audit::updated`], which
//! writes it for you.
//!
//! # Nested objects are flattened, lists are compared as sets
//!
//! `{"password": {"min_length": 8}}` becomes the field `password.min_length`,
//! so a policy with six fields produces six lines rather than one line holding
//! two blobs of JSON. Lists become what joined and what left, because a
//! permission set that gained one name out of forty is unreadable as two lists
//! of forty and obvious as one added name.

use std::collections::{BTreeMap, BTreeSet};

use phonix_core::audit::{Change, Fact, FieldChange};
use serde_json::Value as Json;

/// The keys that carry a diff rather than a fact, and so are not listed twice.
const CHANGE_KEYS: &[&str] = &[
    "from",
    "to",
    "granted",
    "denied",
    "previously_granted",
    "previously_denied",
];

/// Render the free-form `detail` object as one short line.
///
/// The stored object is whatever the use case that wrote it thought useful, and
/// its shape is internal. Rendering it here rather than shipping it keeps the
/// browser from growing a dependency on a structure nothing versions.
pub fn summarise(detail: &Json) -> Option<String> {
    let object = detail.as_object()?;
    if object.is_empty() {
        return None;
    }

    // `reason` is what a failure carries and is the only thing worth reading on
    // one, so it wins outright.
    if let Some(reason) = object.get("reason").and_then(Json::as_str) {
        return Some(reason.to_owned());
    }

    // A row that carries a diff is summarised by the diff, so that the list
    // says "min length 8 -> 12" rather than "from: changed, to: changed".
    let changes = changes(detail);
    if !changes.is_empty() {
        let mut parts: Vec<String> = changes.iter().take(3).map(summarise_change).collect();

        if changes.len() > 3 {
            parts.push(format!("and {} more", changes.len() - 3));
        }

        return Some(parts.join(", "));
    }

    let mut parts: Vec<String> = object
        .iter()
        // Values that are themselves objects or arrays - a whole previous
        // policy, a list of permission names - do not fit on a line and are
        // summarised by their size instead.
        .map(|(key, value)| match value {
            Json::String(text) => format!("{key}: {text}"),
            Json::Array(items) => format!("{key}: {} item(s)", items.len()),
            Json::Object(_) => format!("{key}: changed"),
            other => format!("{key}: {other}"),
        })
        .collect();

    parts.sort();
    parts.truncate(4);

    Some(parts.join(", "))
}

/// One change, short enough for a list cell.
fn summarise_change(change: &FieldChange) -> String {
    let label = change.label();

    match &change.change {
        Change::Value { before, after } => {
            let before = before.as_deref().unwrap_or("not set");
            let after = after.as_deref().unwrap_or("not set");

            format!("{label} {before} -> {after}")
        }
        Change::Members { added, removed } => {
            format!("{label} +{} -{}", added.len(), removed.len())
        }
    }
}

/// The before and after this row recorded, field by field.
///
/// Empty when the row recorded no change, which is what tells a screen to
/// narrate the event instead of drawing a diff.
pub fn changes(detail: &Json) -> Vec<FieldChange> {
    let Some(object) = detail.as_object() else {
        return Vec::new();
    };

    // The override shape, rearranged into the same before-and-after the policy
    // shape already has, so there is one differ rather than two.
    if object.contains_key("granted") || object.contains_key("previously_granted") {
        let pick = |key: &str| object.get(key).cloned().unwrap_or(Json::Array(Vec::new()));

        let before = serde_json::json!({
            "granted": pick("previously_granted"),
            "denied": pick("previously_denied"),
        });
        let after = serde_json::json!({
            "granted": pick("granted"),
            "denied": pick("denied"),
        });

        return diff(&before, &after);
    }

    match (object.get("from"), object.get("to")) {
        (Some(before), Some(after)) => diff(before, after),
        // One side alone is not a diff. It is a fact, and falls through.
        _ => Vec::new(),
    }
}

/// Everything in the object that was not part of the diff.
pub fn facts(detail: &Json) -> Vec<Fact> {
    let Some(object) = detail.as_object() else {
        return Vec::new();
    };

    object
        .iter()
        .filter(|(key, _)| !CHANGE_KEYS.contains(&key.as_str()))
        // `reason` is already the summary and already in the narration; a
        // third copy under a heading is noise.
        .filter(|(key, _)| key.as_str() != "reason")
        .filter_map(|(key, value)| {
            render(value).map(|value| Fact {
                label: humanise(key),
                value,
            })
        })
        .collect()
}

/// What changed between two values, field by field.
fn diff(before: &Json, after: &Json) -> Vec<FieldChange> {
    let mut was = BTreeMap::new();
    let mut now = BTreeMap::new();

    flatten("", before, &mut was);
    flatten("", after, &mut now);

    let fields: BTreeSet<&String> = was.keys().chain(now.keys()).collect();

    fields
        .into_iter()
        .filter(|field| was.get(*field) != now.get(*field))
        .filter_map(|field| {
            let was = was.get(field);
            let now = now.get(field);

            let change = if is_list(was) || is_list(now) {
                let before = members(was);
                let after = members(now);

                Change::Members {
                    added: after
                        .iter()
                        .filter(|item| !before.contains(*item))
                        .cloned()
                        .collect(),
                    removed: before
                        .iter()
                        .filter(|item| !after.contains(*item))
                        .cloned()
                        .collect(),
                }
            } else {
                Change::Value {
                    before: was.and_then(render),
                    after: now.and_then(render),
                }
            };

            // Two lists holding the same names in a different order are not a
            // change, however unequal the JSON is.
            match &change {
                Change::Members { added, removed } if added.is_empty() && removed.is_empty() => {
                    None
                }
                // Nothing on either side is not a change either. A creation's
                // `from` is null and a deletion's `to` is, and a null walked as
                // a value is one nameless leaf at the root - which would put a
                // blank row above every creation on the trail saying that an
                // unnamed field went from nothing to nothing.
                Change::Value { before, after } if before.is_none() && after.is_none() => None,
                _ => Some(FieldChange {
                    field: field.clone(),
                    change,
                }),
            }
        })
        .collect()
}

/// Every leaf of `value`, keyed by its dotted path.
fn flatten(path: &str, value: &Json, out: &mut BTreeMap<String, Json>) {
    match value {
        Json::Object(fields) if !fields.is_empty() => {
            for (key, value) in fields {
                let path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };

                flatten(&path, value, out);
            }
        }
        // An array is a leaf: its members are compared as a set, not walked.
        _ => {
            out.insert(path.to_owned(), value.clone());
        }
    }
}

fn is_list(value: Option<&Json>) -> bool {
    matches!(value, Some(Json::Array(_)))
}

/// A list value as the strings it holds. Anything that is not a list is the
/// one member it is, so that a field which used to hold a single value and now
/// holds a list still diffs sensibly.
fn members(value: Option<&Json>) -> Vec<String> {
    match value {
        Some(Json::Array(items)) => items.iter().filter_map(render).collect(),
        other => other.and_then(render).into_iter().collect(),
    }
}

/// One JSON value as a line of text, or `None` when there is nothing there.
///
/// `None` rather than an empty string: "not set" and "set to nothing" read the
/// same on screen otherwise, and on a security page they are different facts.
fn render(value: &Json) -> Option<String> {
    match value {
        Json::Null => None,
        Json::Bool(true) => Some("Yes".to_owned()),
        Json::Bool(false) => Some("No".to_owned()),
        Json::String(text) if text.is_empty() => None,
        Json::String(text) => Some(text.clone()),
        Json::Number(number) => Some(number.to_string()),
        Json::Array(items) => {
            let rendered: Vec<String> = items.iter().filter_map(render).collect();

            (!rendered.is_empty()).then(|| rendered.join(", "))
        }
        Json::Object(fields) => (!fields.is_empty()).then(|| format!("{} field(s)", fields.len())),
    }
}

/// A stored key as a heading: `role_id` -> "Role id".
fn humanise(key: &str) -> String {
    let mut label = key.replace('_', " ");

    if let Some(first) = label.get_mut(0..1) {
        first.make_ascii_uppercase();
    }

    label
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(changes: &[FieldChange]) -> Vec<&str> {
        changes.iter().map(|change| change.field.as_str()).collect()
    }

    #[test]
    fn a_creation_lists_its_fields_and_nothing_else() {
        // The change trail records a creation as a diff against null. Walking
        // that null as a value puts one nameless leaf at the root, which used
        // to draw a blank row above every creation saying that an unnamed
        // field went from nothing to nothing.
        let detail = serde_json::json!({
            "from": serde_json::Value::Null,
            "to": { "display_name": "Auditor", "is_default": false },
        });

        assert_eq!(fields(&changes(&detail)), ["display_name", "is_default"]);
    }

    #[test]
    fn a_deletion_lists_what_was_lost_and_nothing_else() {
        let detail = serde_json::json!({
            "from": { "display_name": "Auditor" },
            "to": serde_json::Value::Null,
        });

        assert_eq!(fields(&changes(&detail)), ["display_name"]);
    }

    #[test]
    fn a_policy_change_diffs_field_by_field_rather_than_blob_by_blob() {
        let detail = serde_json::json!({
            "from": { "min_length": 8, "require_symbol": false },
            "to": { "min_length": 12, "require_symbol": false },
        });

        let changes = changes(&detail);

        // Only the field that moved. A diff that lists every field of the
        // policy is a diff nobody reads twice.
        assert_eq!(fields(&changes), ["min_length"]);
        assert_eq!(
            changes[0].change,
            Change::Value {
                before: Some("8".into()),
                after: Some("12".into())
            }
        );
    }

    #[test]
    fn a_nested_policy_is_flattened_into_paths() {
        let detail = serde_json::json!({
            "from": { "password": { "min_length": 8 }, "mfa": { "enforcement": "optional" } },
            "to": { "password": { "min_length": 8 }, "mfa": { "enforcement": "required" } },
        });

        let changes = changes(&detail);

        assert_eq!(fields(&changes), ["mfa.enforcement"]);
        assert_eq!(changes[0].label(), "Mfa / enforcement");
    }

    #[test]
    fn a_permission_edit_reads_as_what_joined_and_what_left() {
        let detail = serde_json::json!({
            "subject": "0d1a",
            "previously_granted": ["Pages.Users", "Pages.Roles"],
            "previously_denied": [],
            "granted": ["Pages.Users", "Pages.Settings"],
            "denied": [],
        });

        let changes = changes(&detail);

        assert_eq!(fields(&changes), ["granted"]);
        assert_eq!(
            changes[0].change,
            Change::Members {
                added: vec!["Pages.Settings".into()],
                removed: vec!["Pages.Roles".into()],
            }
        );
    }

    #[test]
    fn a_list_that_only_changed_order_did_not_change() {
        let detail = serde_json::json!({
            "from": { "roles": ["a", "b"] },
            "to": { "roles": ["b", "a"] },
        });

        assert!(changes(&detail).is_empty());
    }

    #[test]
    fn a_field_that_appeared_or_vanished_says_so_rather_than_showing_nothing() {
        let detail = serde_json::json!({
            "from": { "note": null },
            "to": { "note": "locked by an administrator" },
        });

        let changes = changes(&detail);

        assert_eq!(
            changes[0].change,
            Change::Value {
                before: None,
                after: Some("locked by an administrator".into())
            }
        );
    }

    #[test]
    fn an_event_that_changed_nothing_has_no_diff_to_show() {
        // The sign-in case: a reason and nothing else. This is what makes the
        // detail page choose a sentence over a table.
        let detail = serde_json::json!({ "reason": "no such account" });

        assert!(changes(&detail).is_empty());
        assert!(facts(&detail).is_empty());
        assert_eq!(summarise(&detail).as_deref(), Some("no such account"));
    }

    #[test]
    fn a_half_written_shape_is_a_fact_rather_than_a_wrong_diff() {
        // `to` without `from`: something recorded the new value and not the old
        // one. Inventing an empty before would claim every field was added.
        let detail = serde_json::json!({ "to": { "min_length": 12 } });

        assert!(changes(&detail).is_empty());
    }

    #[test]
    fn the_facts_are_what_the_diff_did_not_take() {
        let detail = serde_json::json!({
            "role": "Auditor",
            "role_id": "8f2c",
            "from": { "a": 1 },
            "to": { "a": 2 },
        });

        let facts = facts(&detail);
        let labels: Vec<&str> = facts.iter().map(|fact| fact.label.as_str()).collect();

        // `from` and `to` are the diff and must not be listed again beside it.
        assert_eq!(labels, ["Role", "Role id"]);
        assert_eq!(facts[0].value, "Auditor");
    }

    #[test]
    fn a_row_with_a_diff_is_summarised_by_the_diff() {
        let detail = serde_json::json!({
            "from": { "min_length": 8 },
            "to": { "min_length": 12 },
        });

        // Not "from: changed, to: changed", which is what listing the keys
        // would have produced.
        assert_eq!(summarise(&detail).as_deref(), Some("Min length 8 -> 12"));
    }

    #[test]
    fn an_empty_detail_says_nothing_at_all() {
        assert_eq!(summarise(&serde_json::json!({})), None);
        assert!(changes(&serde_json::json!({})).is_empty());
    }
}
