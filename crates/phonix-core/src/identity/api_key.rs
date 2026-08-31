//! What a screen says about an API key, and what it sends to make one.
//!
//! The credential itself lives in `phonix-services`; nothing here can produce
//! or verify a token. These are the three shapes that cross the wire:
//!
//! * [`ApiKeyDraft`] - what a form fills in.
//! * [`ApiKeySummary`] - what a list draws. Carries the *hint*, never the key.
//! * [`ApiKeyIssued`] - the one moment the token exists outside the client that
//!   will hold it.
//!
//! # Scopes are permission names, and this type does not check them
//!
//! A draft carries names from the compiled tree -
//! `Pages.Administration.Settings` - and [`ApiKeyDraft::validate`] deliberately
//! does not ask whether they are real or whether the issuer holds them. The
//! first is a question for `authorization::is_defined` and the second only the
//! service can answer, because it is about the person asking rather than about
//! the draft. What is checked here is what a form can check without a server:
//! that the key has a name, and that an expiry is in the future.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::i18n::Message;
use crate::identity::FieldError;
use crate::msg;

/// Longest name the column will take. Matches `api_keys_name_length`.
pub const MAX_NAME_LEN: usize = 80;

/// What a form sends to have a key minted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKeyDraft {
    /// What it will be called on the screen that eventually revokes it.
    pub name: String,
    /// Permission names the key is narrowed to. Empty is meaningful: such a
    /// key reaches only what is ungated.
    pub scopes: Vec<String>,
    /// Days from now, or `None` for a key that lives until it is revoked.
    ///
    /// Days rather than an instant because that is the decision being made -
    /// "this contractor is here for three months" - and because a date picker
    /// invites somebody to choose yesterday.
    pub expires_in_days: Option<i64>,
}

impl ApiKeyDraft {
    /// A blank draft, for a form to open on.
    pub fn blank() -> Self {
        Self {
            name: String::new(),
            scopes: Vec::new(),
            expires_in_days: None,
        }
    }

    /// Check what can be checked without touching the database.
    ///
    /// Every problem rather than the first, so a form is not sent round the
    /// loop once per field.
    pub fn validate(&self) -> Vec<FieldError> {
        let mut errors = Vec::new();
        let name = self.name.trim();

        if name.is_empty() {
            errors.push(FieldError::new("name", msg!("error.api_key.name_required")));
        } else if name.chars().count() > MAX_NAME_LEN {
            errors.push(FieldError::new(
                "name",
                msg!("error.api_key.name_too_long", max = MAX_NAME_LEN),
            ));
        }

        if self.expires_in_days.is_some_and(|days| days <= 0) {
            errors.push(FieldError::new(
                "expires_in_days",
                msg!("error.api_key.expiry_in_past"),
            ));
        }

        errors
    }
}

/// Whether a key would be accepted, and if not, why not.
///
/// Three states rather than a boolean, because "somebody stopped this" and
/// "this ran out" are different facts about how a workspace is being looked
/// after, and a list that shows both as "inactive" hides the difference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    Live,
    Expired,
    Revoked,
}

impl KeyState {
    /// What a badge says.
    pub fn label(self) -> Message {
        match self {
            Self::Live => msg!("api_keys.state.live"),
            Self::Expired => msg!("api_keys.state.expired"),
            Self::Revoked => msg!("api_keys.state.revoked"),
        }
    }

    /// The stable value a filter carries, matching what the reader answers.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
        }
    }
}

/// A key as a list shows it. Contains nothing anyone could present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKeySummary {
    pub id: Uuid,
    pub name: String,
    /// The last four characters of the token. Enough to answer "is this the
    /// one in the config file", useless to anybody reading over a shoulder.
    pub hint: String,
    pub scopes: Vec<String>,
    /// The account the key acts as.
    pub owner_name: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl ApiKeySummary {
    /// What state this key is in, as of `now`.
    ///
    /// Revocation wins over expiry: a key somebody stopped is stopped, whatever
    /// its dates say.
    pub fn state(&self, now: DateTime<Utc>) -> KeyState {
        if self.revoked_at.is_some() {
            KeyState::Revoked
        } else if self.expires_at.is_some_and(|expiry| expiry <= now) {
            KeyState::Expired
        } else {
            KeyState::Live
        }
    }

    /// Whether this key would be accepted right now.
    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        matches!(self.state(now), KeyState::Live)
    }

    /// Whether revoking it would do anything.
    ///
    /// An expired key is still worth revoking - it stops being a credential
    /// that a clock change could revive - so only an already-revoked one is
    /// past being stopped.
    pub const fn can_be_revoked(&self) -> bool {
        self.revoked_at.is_none()
    }
}

/// A key, the once.
///
/// The token is in this value on its way to one screen and is then gone: it is
/// not stored, cannot be recovered, and is deliberately not part of
/// [`ApiKeySummary`], so no list can ever draw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKeyIssued {
    pub key: ApiKeySummary,
    /// `phx_...`. Shown once.
    pub secret: String,
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    fn summary() -> ApiKeySummary {
        ApiKeySummary {
            id: Uuid::nil(),
            name: "nightly export".to_owned(),
            hint: "wxyz".to_owned(),
            scopes: vec!["Pages.Administration.Settings".to_owned()],
            owner_name: "Ada Lovelace".to_owned(),
            created_at: Utc::now(),
            expires_at: None,
            last_used_at: None,
            revoked_at: None,
        }
    }

    #[test]
    fn a_key_needs_a_name_worth_reading_later() {
        let mut draft = ApiKeyDraft::blank();
        assert_eq!(draft.validate().len(), 1);

        draft.name = "  ".to_owned();
        assert_eq!(
            draft.validate().first().map(|err| err.field.as_str()),
            Some("name"),
            "whitespace is not a name"
        );

        draft.name = "a".repeat(MAX_NAME_LEN + 1);
        assert_eq!(draft.validate().len(), 1);

        draft.name = "nightly export".to_owned();
        assert!(draft.validate().is_empty());
    }

    #[test]
    fn an_expiry_that_has_already_passed_is_refused_before_the_server_sees_it() {
        let draft = ApiKeyDraft {
            name: "contractor".to_owned(),
            scopes: Vec::new(),
            expires_in_days: Some(0),
        };

        assert_eq!(
            draft.validate().first().map(|err| err.field.as_str()),
            Some("expires_in_days")
        );
    }

    #[test]
    fn no_scopes_is_a_shape_rather_than_a_mistake() {
        // A key that reads only what is ungated is the useful minimum, not an
        // unfinished form.
        let draft = ApiKeyDraft {
            name: "read only".to_owned(),
            ..ApiKeyDraft::blank()
        };

        assert!(draft.validate().is_empty());
    }

    #[test]
    fn revocation_wins_over_expiry() {
        let now = Utc::now();
        let mut key = summary();

        assert_eq!(key.state(now), KeyState::Live);
        assert!(key.is_live(now));

        key.expires_at = Some(now - Duration::hours(1));
        assert_eq!(key.state(now), KeyState::Expired);
        assert!(key.can_be_revoked(), "an expired key is still worth stopping");

        key.revoked_at = Some(now - Duration::days(1));
        assert_eq!(key.state(now), KeyState::Revoked);
        assert!(!key.can_be_revoked());
    }
}
