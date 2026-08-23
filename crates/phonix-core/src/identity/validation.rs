//! Field rules shared by the browser and the server.
//!
//! Compiled into both, so the signup form applies exactly what the server will
//! enforce and a green field cannot turn into a rejection. The server still
//! re-runs every check - a client is never trusted - but the two cannot drift.

use serde::{Deserialize, Serialize};

use crate::i18n::Message;
use crate::msg;
use crate::tenant::{InvalidTenantSlug, TenantSlug};

/// Longest a person's name may be. Generous on purpose.
pub const MAX_NAME_LEN: usize = 80;

/// Longest an organization's display name may be.
pub const MAX_ORGANIZATION_NAME_LEN: usize = 120;

/// One problem with one field, addressed to the person filling in the form.
///
/// The message is a [`Message`], not a sentence. A validator runs on the
/// server and knows nothing about who is reading, so it names what is wrong and
/// leaves the wording to the view - see [`crate::i18n`]. `Display` renders the
/// built-in English, which is what a log line and a test want.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{field}: {message}")]
pub struct FieldError {
    /// Matches the form field's `name`, so the UI can place the message.
    pub field: String,
    pub message: Message,
}

impl FieldError {
    /// Build one. `message` is deliberately not `impl Into<Message>`: an
    /// English string must not be able to slip in unnoticed, because it would
    /// reach the browser untranslatable and nothing would report it.
    pub fn new(field: impl Into<String>, message: Message) -> Self {
        Self {
            field: field.into(),
            message,
        }
    }
}

/// Push an error into `sink` and yield `None`, or yield the value.
///
/// Lets a validator collect every failure in one pass instead of returning at
/// the first, which is what makes "fix one thing, resubmit, find the next"
/// avoidable.
pub(crate) fn collect<T>(result: Result<T, FieldError>, sink: &mut Vec<FieldError>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(err) => {
            sink.push(err);
            None
        }
    }
}

/// A given or family name. Returns the trimmed value.
pub fn validate_person_name(field: &str, raw: &str) -> Result<String, FieldError> {
    let trimmed = raw.trim();

    // The two fields have separate keys rather than one key with the label
    // interpolated. "First name is required" and "Last name is required" are
    // one sentence in English and need not be in another language - a language
    // with grammatical gender agrees the verb with the noun - and a translator
    // handed `{label} is required.` cannot fix that.
    let (required, too_long, charset) = if field == "first_name" {
        (
            msg!("validation.first_name.required"),
            msg!("validation.first_name.too_long", max = MAX_NAME_LEN),
            msg!("validation.first_name.charset"),
        )
    } else {
        (
            msg!("validation.last_name.required"),
            msg!("validation.last_name.too_long", max = MAX_NAME_LEN),
            msg!("validation.last_name.charset"),
        )
    };

    if trimmed.is_empty() {
        return Err(FieldError::new(field, required));
    }
    // Counted in chars, not bytes: "José" is four characters, five bytes.
    if trimmed.chars().count() > MAX_NAME_LEN {
        return Err(FieldError::new(field, too_long));
    }
    // No allow-list of letters: names contain apostrophes, hyphens, spaces and
    // every script there is. Only control characters are refused, because those
    // are never part of a name and do turn up in injection attempts.
    if trimmed.chars().any(char::is_control) {
        return Err(FieldError::new(field, charset));
    }

    Ok(trimmed.to_owned())
}

/// Returns the trimmed, lowercased address.
///
/// Deliberately permissive. A regex that tries to implement RFC 5322 will
/// reject valid addresses; the only real proof that an address works is a
/// delivered message, which is what email verification is for.
pub fn validate_email(raw: &str) -> Result<String, FieldError> {
    const FIELD: &str = "email";
    let trimmed = raw.trim();

    if trimmed.is_empty() {
        return Err(FieldError::new(FIELD, msg!("validation.email.required")));
    }
    if trimmed.len() > 254 {
        // RFC 5321 caps a forward path at 254 characters.
        return Err(FieldError::new(FIELD, msg!("validation.email.too_long")));
    }
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(FieldError::new(FIELD, msg!("validation.email.whitespace")));
    }

    let Some((local, domain)) = trimmed.split_once('@') else {
        return Err(FieldError::new(FIELD, msg!("validation.email.no_at")));
    };

    if local.is_empty() {
        return Err(FieldError::new(FIELD, msg!("validation.email.no_local")));
    }
    // A second @ means the domain half contains one, which is never valid in
    // the addresses this application will ever see.
    if domain.contains('@') {
        return Err(FieldError::new(FIELD, msg!("validation.email.two_at")));
    }
    if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') {
        return Err(FieldError::new(
            FIELD,
            msg!("validation.email.domain_incomplete"),
        ));
    }
    if domain.contains("..") {
        return Err(FieldError::new(
            FIELD,
            msg!("validation.email.domain_invalid"),
        ));
    }

    // Lowercased so that Ada@Example.com and ada@example.com are one account.
    // Strictly the local part is case-sensitive per RFC 5321, but no mail
    // provider in practice treats it that way, and honouring it would let one
    // person register twice.
    Ok(trimmed.to_ascii_lowercase())
}

/// Returns the trimmed organization name.
pub fn validate_organization_name(raw: &str) -> Result<String, FieldError> {
    const FIELD: &str = "organization_name";
    let trimmed = raw.trim();

    if trimmed.is_empty() {
        return Err(FieldError::new(
            FIELD,
            msg!("validation.organization_name.required"),
        ));
    }
    if trimmed.chars().count() < 2 {
        return Err(FieldError::new(
            FIELD,
            msg!("validation.organization_name.too_short"),
        ));
    }
    if trimmed.chars().count() > MAX_ORGANIZATION_NAME_LEN {
        return Err(FieldError::new(
            FIELD,
            msg!(
                "validation.organization_name.too_long",
                max = MAX_ORGANIZATION_NAME_LEN
            ),
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(FieldError::new(
            FIELD,
            msg!("validation.organization_name.charset"),
        ));
    }

    Ok(trimmed.to_owned())
}

/// Validate the chosen subdomain, restating [`TenantSlug`]'s rules in the
/// second person.
///
/// [`TenantSlug::parse`] is the actual authority - this only turns its errors
/// into something worth reading under a form field.
pub fn validate_workspace_slug(raw: &str) -> Result<TenantSlug, FieldError> {
    const FIELD: &str = "workspace_slug";

    if raw.trim().is_empty() {
        return Err(FieldError::new(
            FIELD,
            msg!("validation.workspace_slug.required"),
        ));
    }

    TenantSlug::parse(raw).map_err(|err| {
        let message = match err {
            InvalidTenantSlug::Length { min, max, .. } => {
                msg!("validation.workspace_slug.length", min = min, max = max)
            }
            InvalidTenantSlug::Charset => msg!("validation.workspace_slug.charset"),
            InvalidTenantSlug::Boundary => msg!("validation.workspace_slug.boundary"),
            InvalidTenantSlug::DoubleHyphen => msg!("validation.workspace_slug.double_hyphen"),
            InvalidTenantSlug::LeadingDigit => msg!("validation.workspace_slug.leading_digit"),
        };
        FieldError::new(FIELD, message)
    })
}

/// Derive a workspace address from an organization name.
///
/// `"Acme Widgets, Inc."` becomes `"acme-widgets-inc"`. Best-effort: the result
/// is a *suggestion* the user can overwrite, so an unusable one (an all-emoji
/// name, a name starting with a digit) returns `None` rather than inventing
/// something. The field then simply starts empty.
pub fn slug_from_organization_name(name: &str) -> Option<String> {
    // Matches `TenantSlug`'s own ceiling, so a suggestion is never rejected
    // for being too long.
    const SLUG_SUGGESTION_MAX: usize = 40;

    let mut out = String::with_capacity(name.len());
    let mut pending_hyphen = false;

    for ch in name.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_hyphen && !out.is_empty() {
                out.push('-');
            }
            pending_hyphen = false;
            out.push(ch);
        } else {
            // Any run of separators or non-ASCII collapses to one hyphen, and
            // trailing runs are dropped because the flag is only ever flushed
            // before the next kept character.
            pending_hyphen = true;
        }

        if out.len() >= SLUG_SUGGESTION_MAX {
            break;
        }
    }

    // The loop can push a hyphen and a character in one pass, so the cap is
    // enforced again here rather than trusted from the break above. `out` is
    // ASCII by construction, so truncating by bytes cannot split a character.
    out.truncate(SLUG_SUGGESTION_MAX);

    // A leading digit is legal in DNS but would need quoting as a Postgres
    // identifier, so `TenantSlug` refuses it; drop the digits rather than
    // proposing something that cannot be accepted.
    let trimmed = out.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-');
    let trimmed = trimmed.trim_end_matches('-');

    (trimmed.len() >= 2).then(|| trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_addresses() {
        for good in [
            "ada@example.com",
            "ada+tag@example.co.uk",
            "a.b_c-d@sub.example.io",
            "ada@münchen.example",
        ] {
            assert!(validate_email(good).is_ok(), "{good} should be accepted");
        }
    }

    #[test]
    fn rejects_addresses_that_cannot_work() {
        for bad in [
            "",
            "ada",
            "@example.com",
            "ada@",
            "ada@example",
            "ada@@example.com",
            "ada@.example.com",
            "ada@example..com",
            "ada lovelace@example.com",
        ] {
            assert!(validate_email(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn email_is_normalised() {
        assert_eq!(
            validate_email("  Ada@Example.COM  ").unwrap(),
            "ada@example.com"
        );
    }

    #[test]
    fn names_accept_every_script_but_no_control_characters() {
        for good in ["Ada", "O'Brien", "van der Berg", "李", "Þórunn"] {
            assert!(validate_person_name("first_name", good).is_ok(), "{good}");
        }
        assert!(validate_person_name("first_name", "").is_err());
        assert!(validate_person_name("first_name", "Ada\u{0}").is_err());
        assert!(validate_person_name("last_name", &"n".repeat(MAX_NAME_LEN + 1)).is_err());
    }

    #[test]
    fn slugs_are_derived_from_names() {
        for (name, expected) in [
            ("Acme", "acme"),
            ("Acme Widgets", "acme-widgets"),
            ("Acme Widgets, Inc.", "acme-widgets-inc"),
            ("  North   Wind  ", "north-wind"),
            ("Café Zürich", "caf-z-rich"),
            ("3M Company", "m-company"),
        ] {
            assert_eq!(
                slug_from_organization_name(name).as_deref(),
                Some(expected),
                "for {name:?}"
            );
        }
    }

    #[test]
    fn every_derived_slug_is_a_valid_tenant_slug() {
        for name in [
            "Acme Widgets, Inc.",
            "北京公司 Ltd",
            "A & B",
            "  spaced  out  ",
            "Very Long Organization Name That Goes On And On Forever And Ever",
        ] {
            if let Some(slug) = slug_from_organization_name(name) {
                assert!(
                    TenantSlug::parse(&slug).is_ok(),
                    "derived {slug:?} from {name:?} must be a valid slug"
                );
            }
        }
    }

    #[test]
    fn undecidable_names_yield_no_suggestion() {
        for name in ["", "   ", "!!!", "北京", "3"] {
            assert_eq!(slug_from_organization_name(name), None, "for {name:?}");
        }
    }

    #[test]
    fn slug_errors_name_the_rule_that_was_broken() {
        // Asserted on the key rather than on the English, which is the point of
        // the exercise: rewording a sentence in `i18n/en.json` is now a copy
        // change, not a test failure.
        let err = validate_workspace_slug("Acme Corp").unwrap_err();
        assert_eq!(err.field, "workspace_slug");
        assert_eq!(err.message.key, "validation.workspace_slug.charset");

        assert_eq!(
            validate_workspace_slug("3m").unwrap_err().message.key,
            "validation.workspace_slug.leading_digit"
        );

        // The sentence is still reachable, and still reads for a person.
        assert!(err.message.to_string().contains("lowercase"));
    }

    #[test]
    fn a_length_rejection_carries_the_bounds_it_was_measured_against() {
        // The numbers travel as arguments, so a translation can put them
        // wherever its own grammar needs them.
        let err = validate_workspace_slug("a").unwrap_err();
        assert_eq!(err.message.key, "validation.workspace_slug.length");
        assert_eq!(err.message.args.len(), 2);
        assert!(err.message.to_string().contains("between"));
    }
}
