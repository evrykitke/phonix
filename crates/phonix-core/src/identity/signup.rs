//! Onboarding: what the signup wizard sends, and what comes back.

use serde::{Deserialize, Serialize};

use crate::tenant::TenantSlug;

use super::password::validate_password;
use crate::msg;

use super::validation::{
    FieldError, collect, validate_email, validate_organization_name, validate_person_name,
    validate_workspace_slug,
};

/// Everything the signup wizard collects, submitted in one call.
///
/// The wizard is three screens but one request: a partially created workspace -
/// a tenant row with no owner, or an owner with no database - is a state nobody
/// wants to reason about later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignupInput {
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub password: String,
    pub password_confirmation: String,
    pub organization_name: String,
    /// The subdomain. Derived from `organization_name` by the UI, but sent
    /// explicitly because the user is allowed to change it.
    pub workspace_slug: String,
}

impl SignupInput {
    /// Run every field rule.
    ///
    /// Returns all failures at once rather than the first: a form that reveals
    /// one problem per submission is the single most irritating thing a signup
    /// flow can do.
    pub fn validate(&self) -> Result<ValidSignup, Vec<FieldError>> {
        let mut errors = Vec::new();

        let first_name = collect(
            validate_person_name("first_name", &self.first_name),
            &mut errors,
        );
        let last_name = collect(
            validate_person_name("last_name", &self.last_name),
            &mut errors,
        );
        let email = collect(validate_email(&self.email), &mut errors);
        let organization_name = collect(
            validate_organization_name(&self.organization_name),
            &mut errors,
        );
        let workspace_slug = collect(validate_workspace_slug(&self.workspace_slug), &mut errors);

        // Password rules run even when the confirmation does not match, so the
        // user sees "too short" and "does not match" together.
        if let Err(err) = validate_password(&self.password) {
            errors.push(err);
        }
        if self.password != self.password_confirmation {
            errors.push(FieldError::new(
                "password_confirmation",
                msg!("validation.password.mismatch"),
            ));
        }

        // A password built out of something else on this very form is the most
        // common way a "strong-looking" password turns out to be guessable.
        if password_echoes_identity(self) {
            errors.push(FieldError::new(
                "password",
                msg!("validation.password.contains_personal"),
            ));
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        // Every validator that failed pushed an error, so an empty list means
        // all five of these are `Some`. Destructured rather than unwrapped:
        // this runs in the browser as well as on the server, and in a wasm
        // bundle a panic is not a stack trace but a page that stops responding
        // altogether. A field added here whose failure is not collected now
        // shows the form an error instead, which is recoverable and visible.
        let (
            Some(first_name),
            Some(last_name),
            Some(email),
            Some(organization_name),
            Some(workspace_slug),
        ) = (
            first_name,
            last_name,
            email,
            organization_name,
            workspace_slug,
        )
        else {
            return Err(vec![FieldError::new(
                "form",
                msg!("validation.form.uncheckable"),
            )]);
        };

        Ok(ValidSignup {
            first_name,
            last_name,
            email,
            password: self.password.clone(),
            organization_name,
            workspace_slug,
        })
    }
}

/// A [`SignupInput`] that has passed every rule, with fields normalised.
///
/// Constructible only through [`SignupInput::validate`], so a function taking
/// this type cannot be handed unvalidated input by mistake.
#[derive(Debug, Clone)]
pub struct ValidSignup {
    pub first_name: String,
    pub last_name: String,
    /// Lowercased and trimmed.
    pub email: String,
    pub password: String,
    pub organization_name: String,
    pub workspace_slug: TenantSlug,
}

impl ValidSignup {
    pub fn display_name(&self) -> String {
        format!("{} {}", self.first_name, self.last_name)
    }
}

/// What the client needs after a workspace is created.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignupOutcome {
    pub workspace_slug: TenantSlug,
    pub organization_name: String,
    /// Absolute URL of the new workspace, including scheme and port.
    pub workspace_url: String,
    /// Where to send the browser to trade the handoff token for a session
    /// cookie on the workspace's own host. See `phonix_server::auth::handoff`.
    pub handoff_url: String,
}

/// The outcome of a signup attempt.
///
/// Validation failures arrive as `Ok(Rejected(..))`, not as an `Err`. A person
/// mistyping their email is the expected path through this code, not an
/// exception - and modelling it as one would mean squeezing structured,
/// per-field messages through a stringly-typed error channel.
///
/// `Err` from the server function therefore means only one thing: something
/// broke.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SignupResult {
    Created(Box<SignupOutcome>),
    /// One or more fields need fixing. Never empty.
    Rejected(Vec<FieldError>),
    /// Self-service signup is switched off for this deployment.
    Closed,
}

/// Whether a slug is free, and why not when it is taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlugAvailability {
    pub slug: String,
    pub available: bool,
    /// Shown under the field. `None` when available.
    pub reason: Option<String>,
    /// A free alternative, when the requested one is taken.
    pub suggestion: Option<String>,
}

/// Whether the password is a thin disguise of something already on the form.
///
/// Only the mailbox half of the address is checked, never the domain: at a
/// company where everyone is `@acme.com`, treating "acme" as forbidden would
/// reject a great many perfectly good passphrases for no gain.
fn password_echoes_identity(input: &SignupInput) -> bool {
    let lowered = input.password.to_lowercase();
    if lowered.chars().count() < 4 {
        return false;
    }

    let local_part = input.email.split('@').next().unwrap_or("");

    // Three characters for names, which are short and are what people actually
    // reuse; four for the free-text organization name, where a shorter run is
    // likely to be an accidental substring.
    [
        (input.first_name.as_str(), 3usize),
        (input.last_name.as_str(), 3),
        (local_part, 3),
        (input.organization_name.as_str(), 4),
    ]
    .into_iter()
    .any(|(candidate, min_len)| {
        let candidate = candidate.trim().to_lowercase();
        candidate.chars().count() >= min_len && lowered.contains(&candidate)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signup() -> SignupInput {
        SignupInput {
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            email: "ada@example.com".into(),
            password: "correct horse battery".into(),
            password_confirmation: "correct horse battery".into(),
            organization_name: "Analytical Engines".into(),
            workspace_slug: "analytical-engines".into(),
        }
    }

    #[test]
    fn a_complete_form_validates() {
        let valid = signup().validate().expect("should be valid");
        assert_eq!(valid.email, "ada@example.com");
        assert_eq!(valid.workspace_slug.as_str(), "analytical-engines");
        assert_eq!(valid.display_name(), "Ada Lovelace");
    }

    #[test]
    fn fields_are_normalised_on_the_way_through() {
        let mut input = signup();
        input.email = "  Ada@Example.COM  ".into();
        input.first_name = "  Ada  ".into();
        input.workspace_slug = "  Analytical-Engines ".into();

        let valid = input.validate().unwrap();
        assert_eq!(valid.email, "ada@example.com");
        assert_eq!(valid.first_name, "Ada");
        assert_eq!(valid.workspace_slug.as_str(), "analytical-engines");
    }

    #[test]
    fn every_problem_is_reported_at_once() {
        let input = SignupInput {
            first_name: "".into(),
            last_name: "".into(),
            email: "not-an-email".into(),
            password: "short".into(),
            password_confirmation: "different".into(),
            organization_name: "".into(),
            workspace_slug: "".into(),
        };

        let errors = input.validate().unwrap_err();
        let fields: Vec<&str> = errors.iter().map(|e| e.field.as_str()).collect();

        // One round-trip must surface all of them, not just the first.
        for expected in [
            "first_name",
            "last_name",
            "email",
            "password",
            "password_confirmation",
            "organization_name",
            "workspace_slug",
        ] {
            assert!(fields.contains(&expected), "missing error for {expected}");
        }
    }

    #[test]
    fn mismatched_confirmation_is_caught() {
        let mut input = signup();
        input.password_confirmation = "something else entirely".into();
        let errors = input.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.field == "password_confirmation"));
    }

    #[test]
    fn password_may_not_echo_the_form() {
        for password in [
            "ada-ada-ada-ada",         // first name
            "lovelace-lovelace",       // last name
            "analytical engines rock", // organization
        ] {
            let mut input = signup();
            input.password = password.into();
            input.password_confirmation = password.into();

            let errors = input.validate().unwrap_err();
            assert!(
                errors.iter().any(|e| e.field == "password"),
                "{password:?} should be refused"
            );
        }
    }

    #[test]
    fn the_email_domain_is_not_treated_as_a_forbidden_word() {
        // Everyone at a company shares a domain; banning it would reject a lot
        // of perfectly good passphrases.
        let mut input = signup();
        input.password = "example horse battery".into();
        input.password_confirmation = input.password.clone();
        assert!(input.validate().is_ok());
    }
}
