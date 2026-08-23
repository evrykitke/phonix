//! Password policy and the strength meter.
//!
//! Policy lives here rather than in `config/*.toml` on purpose: the browser has
//! to apply the same rules to draw the meter and enable the submit button, and
//! it cannot read a server-side config file. Only *cost* - the Argon2id work
//! factors - is configurable server-side, because that depends on the hardware.
//!
//! # Three layers
//!
//! ```text
//! ABSOLUTE_MIN_LENGTH ..  the floor no one can go under, compiled in
//! PasswordPolicy::system_default()  ..  what a new workspace starts with
//! workspace_settings.password_*    ..  what this organization decided
//! ```
//!
//! An organization may tighten the policy, and may loosen it only as far as the
//! absolute floor. [`PasswordPolicy::validate`] is what enforces that, and it
//! runs on the settings form - not on the sign-up form, which has no
//! organization yet and therefore uses the system default.
//!
//! The hashing itself is in `phonix_db::identity::password`; nothing in this
//! module ever sees a hash.

use serde::{Deserialize, Serialize};

use super::validation::FieldError;
use crate::{Message, msg};

/// Minimum length the system default asks for.
///
/// NIST SP 800-63B: length is the control that matters. Composition rules
/// ("one uppercase, one symbol") mostly push people toward `Password1!`, so the
/// default floor is high and composition is off - available to organizations
/// that have a compliance regime demanding it, not imposed on those that don't.
pub const DEFAULT_MIN_LENGTH: usize = 12;

/// The shortest password any organization may configure.
///
/// Below this the Argon2 cost stops mattering: an eight-character password is
/// already the weakest link, and a policy that permits six would make the
/// hashing parameters theatre.
pub const ABSOLUTE_MIN_LENGTH: usize = 8;

/// Upper bound on password length.
///
/// Not a security limit - Argon2 accepts any length - but a denial-of-service
/// one: each hash costs ~19 MiB and ~50 ms, and nothing is gained by letting an
/// anonymous caller submit a megabyte of it.
pub const MAX_PASSWORD_LEN: usize = 256;

/// How many previous passwords a workspace may insist on remembering.
///
/// Beyond this the check costs more than it buys: every stored hash has to be
/// verified against the new password, at ~50 ms each.
pub const MAX_HISTORY_DEPTH: u8 = 24;

/// The longest expiry an organization may configure, in days.
pub const MAX_EXPIRY_DAYS: u32 = 3650;

/// What one organization requires of its users' passwords.
///
/// Serialised to the browser so the sign-in and change-password forms show the
/// same rules the server will apply. Every field is a *requirement*, so the
/// `Default` (which is [`Self::system_default`]) is the loosest policy the
/// system ships with, not an empty one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PasswordPolicy {
    pub min_length: usize,
    pub max_length: usize,

    // Composition. Off by default; see the note on `DEFAULT_MIN_LENGTH`.
    pub require_lowercase: bool,
    pub require_uppercase: bool,
    pub require_digit: bool,
    pub require_symbol: bool,

    /// Refuse passwords on the built-in list of guessed-first strings.
    pub forbid_common: bool,

    /// Refuse passwords built from the user's own name, address or the
    /// organization's name - the first things an attacker who knows the target
    /// will try.
    pub forbid_personal_information: bool,

    /// Force a change after this many days. `None` is the default and the
    /// recommendation: NIST withdrew routine expiry because it produces
    /// `Summer2024!` then `Autumn2024!`. Available because some auditors still
    /// require it.
    pub expiry_days: Option<u32>,

    /// How many previous passwords may not be reused. `0` disables the check.
    pub history_depth: u8,
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self::system_default()
    }
}

impl PasswordPolicy {
    /// What a workspace gets on the day it is created.
    pub const fn system_default() -> Self {
        Self {
            min_length: DEFAULT_MIN_LENGTH,
            max_length: MAX_PASSWORD_LEN,
            require_lowercase: false,
            require_uppercase: false,
            require_digit: false,
            require_symbol: false,
            forbid_common: true,
            forbid_personal_information: true,
            expiry_days: None,
            history_depth: 0,
        }
    }

    /// Check a policy an administrator submitted.
    ///
    /// Returns every problem at once, because the settings form shows them all
    /// at once. The field names match the form's inputs.
    pub fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut errors = Vec::new();

        if self.min_length < ABSOLUTE_MIN_LENGTH {
            errors.push(FieldError::new(
                "min_length",
                msg!(
                    "validation.password_policy.min_too_low",
                    min = ABSOLUTE_MIN_LENGTH
                ),
            ));
        }
        if self.max_length > MAX_PASSWORD_LEN {
            errors.push(FieldError::new(
                "max_length",
                msg!(
                    "validation.password_policy.max_too_high",
                    max = MAX_PASSWORD_LEN
                ),
            ));
        }
        if self.min_length > self.max_length {
            errors.push(FieldError::new(
                "min_length",
                msg!("validation.password_policy.min_above_max"),
            ));
        }
        if self.history_depth > MAX_HISTORY_DEPTH {
            errors.push(FieldError::new(
                "history_depth",
                msg!(
                    "validation.password_policy.history_too_deep",
                    max = MAX_HISTORY_DEPTH
                ),
            ));
        }
        match self.expiry_days {
            // Zero would mean "expire immediately", which locks the whole
            // workspace out the moment it is saved. It is always a mistake for
            // "never", which is `None`.
            Some(0) => errors.push(FieldError::new(
                "expiry_days",
                msg!("validation.password_policy.expiry_zero"),
            )),
            Some(days) if days > MAX_EXPIRY_DAYS => errors.push(FieldError::new(
                "expiry_days",
                msg!(
                    "validation.password_policy.expiry_too_long",
                    max = MAX_EXPIRY_DAYS
                ),
            )),
            _ => {}
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// The rules, as the lines of the checklist under the password field.
    ///
    /// Rendered by the client, so it has to be exactly what [`Self::check`]
    /// enforces - a checklist that promises something the server does not
    /// check, or omits something it does, is worse than none.
    pub fn requirements(&self) -> Vec<String> {
        let mut lines = vec![format!("At least {} characters", self.min_length)];

        if self.require_lowercase {
            lines.push("A lowercase letter".to_owned());
        }
        if self.require_uppercase {
            lines.push("An uppercase letter".to_owned());
        }
        if self.require_digit {
            lines.push("A number".to_owned());
        }
        if self.require_symbol {
            lines.push("A symbol".to_owned());
        }
        if self.history_depth > 0 {
            lines.push(format!(
                "Not one of your last {} passwords",
                self.history_depth
            ));
        }

        lines
    }

    /// Apply the policy to a candidate password.
    ///
    /// Everything that can be judged from the password alone. Reuse against
    /// history needs stored hashes and happens server-side; the personal
    /// information check needs the user's own details and is
    /// [`super::signup::password_echoes_identity`].
    pub fn check(&self, raw: &str) -> Result<(), FieldError> {
        const FIELD: &str = "password";

        if raw.is_empty() {
            return Err(FieldError::new(FIELD, msg!("validation.password.required")));
        }
        if raw.chars().count() < self.min_length {
            return Err(FieldError::new(
                FIELD,
                msg!("validation.password.too_short", min = self.min_length),
            ));
        }
        // Bytes, not characters: the ceiling exists to bound hashing work, and
        // Argon2 hashes bytes.
        if raw.len() > self.max_length {
            return Err(FieldError::new(
                FIELD,
                msg!("validation.password.too_long", max = self.max_length),
            ));
        }
        if self.require_lowercase && !raw.chars().any(char::is_lowercase) {
            return Err(FieldError::new(
                FIELD,
                msg!("validation.password.needs_lowercase"),
            ));
        }
        if self.require_uppercase && !raw.chars().any(char::is_uppercase) {
            return Err(FieldError::new(
                FIELD,
                msg!("validation.password.needs_uppercase"),
            ));
        }
        if self.require_digit && !raw.chars().any(|c| c.is_ascii_digit()) {
            return Err(FieldError::new(
                FIELD,
                msg!("validation.password.needs_digit"),
            ));
        }
        // Whitespace does not count. A passphrase with spaces would otherwise
        // satisfy "include a symbol" without the person typing one, which makes
        // the checklist tick itself and means nothing.
        if self.require_symbol
            && !raw
                .chars()
                .any(|c| !c.is_alphanumeric() && !c.is_whitespace())
        {
            return Err(FieldError::new(
                FIELD,
                msg!("validation.password.needs_symbol"),
            ));
        }
        if self.forbid_common
            && COMMON_PASSWORDS
                .iter()
                .any(|common| raw.eq_ignore_ascii_case(common))
        {
            return Err(FieldError::new(FIELD, msg!("validation.password.breached")));
        }

        Ok(())
    }

    /// Whether a password set at `changed_at` has aged out.
    ///
    /// `None` expiry means never, which is the default.
    pub fn is_expired(
        &self,
        changed_at: chrono::DateTime<chrono::Utc>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> bool {
        let Some(days) = self.expiry_days else {
            return false;
        };
        now - changed_at > chrono::Duration::days(i64::from(days))
    }
}

/// Length and breach rules for a new password, under the system default policy.
///
/// The sign-up form's check: at that point no organization exists yet, so there
/// is no organization policy to apply. Everything after sign-up goes through
/// [`PasswordPolicy::check`] with the workspace's own policy.
pub fn validate_password(raw: &str) -> Result<(), FieldError> {
    PasswordPolicy::system_default().check(raw)
}

/// How strong a password looks, for the meter under the field.
///
/// Advisory only: [`PasswordPolicy::check`] decides what is accepted. The meter
/// exists to nudge, not to gate, so a `Fair` password still submits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PasswordStrength {
    Empty,
    TooShort,
    Weak,
    Fair,
    Good,
    Strong,
}

impl PasswordStrength {
    /// 0-4, for the width of the meter.
    pub fn filled_bars(self) -> u8 {
        match self {
            Self::Empty => 0,
            Self::TooShort | Self::Weak => 1,
            Self::Fair => 2,
            Self::Good => 3,
            Self::Strong => 4,
        }
    }

    /// The word under the meter, or `None` while the box is empty.
    ///
    /// `None` rather than a blank message: an empty box has nothing to say
    /// about it, and the caller shows its hint instead. A key that renders to
    /// the empty string would put that decision in the catalog, where a
    /// translator would eventually and reasonably fill it in.
    pub fn message(self) -> Option<Message> {
        match self {
            Self::Empty => None,
            Self::TooShort => Some(msg!("password.strength.too_short")),
            Self::Weak => Some(msg!("password.strength.weak")),
            Self::Fair => Some(msg!("password.strength.fair")),
            Self::Good => Some(msg!("password.strength.good")),
            Self::Strong => Some(msg!("password.strength.strong")),
        }
    }

    /// Whether a password of this strength is allowed through.
    pub fn is_acceptable(self) -> bool {
        self >= Self::Weak && self != Self::TooShort
    }
}

/// Score a password on length and character variety, under a given policy.
///
/// Length dominates, because it should: a 20-character passphrase of lowercase
/// words beats `P@ss1!` by orders of magnitude, and a meter that says otherwise
/// teaches people the wrong lesson. The policy is passed in only so the
/// "too short" threshold matches the one that will actually reject the form.
pub fn password_strength_for(password: &str, policy: &PasswordPolicy) -> PasswordStrength {
    let length = password.chars().count();

    if length == 0 {
        return PasswordStrength::Empty;
    }
    if length < policy.min_length {
        return PasswordStrength::TooShort;
    }
    if COMMON_PASSWORDS
        .iter()
        .any(|common| password.eq_ignore_ascii_case(common))
    {
        return PasswordStrength::Weak;
    }

    // A password built from a handful of characters reaches any length without
    // gaining entropy, so it is scored before length is considered at all.
    let distinct = {
        let mut chars: Vec<char> = password.chars().collect();
        chars.sort_unstable();
        chars.dedup();
        chars.len()
    };
    if distinct <= 4 {
        return PasswordStrength::Weak;
    }

    let mut score: u32 = match length {
        0..=11 => 0,
        12..=15 => 1,
        16..=19 => 2,
        20..=27 => 3,
        _ => 4,
    };

    let classes = [
        password.chars().any(|c| c.is_lowercase()),
        password.chars().any(|c| c.is_uppercase()),
        password.chars().any(|c| c.is_numeric()),
        password.chars().any(|c| !c.is_alphanumeric()),
    ]
    .iter()
    .filter(|present| **present)
    .count();

    if classes >= 3 {
        score += 1;
    }

    match score {
        0 => PasswordStrength::Weak,
        1 => PasswordStrength::Fair,
        2 | 3 => PasswordStrength::Good,
        _ => PasswordStrength::Strong,
    }
}

/// Score a password under the system default policy.
pub fn password_strength(password: &str) -> PasswordStrength {
    password_strength_for(password, &PasswordPolicy::system_default())
}

/// The handful of passwords that clear a 12-character floor and are still
/// guessed first. Not a breach corpus - a full check belongs server-side
/// against a k-anonymity API, which is a later job.
const COMMON_PASSWORDS: &[&str] = &[
    "password1234",
    "password123!",
    "qwertyuiop123",
    "123456789012",
    "1234567890123",
    "administrator",
    "letmein12345",
    "welcome12345",
    "iloveyou1234",
    "trustno1trustno1",
    "passwordpassword",
    "qwertyqwerty",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_floor_is_length_not_composition() {
        assert!(validate_password("z".repeat(DEFAULT_MIN_LENGTH).as_str()).is_ok());
        assert!(validate_password("z".repeat(DEFAULT_MIN_LENGTH - 1).as_str()).is_err());
        assert!(validate_password(&"z".repeat(MAX_PASSWORD_LEN + 1)).is_err());
        // A long all-lowercase passphrase passes; no symbol is demanded.
        assert!(validate_password("correct horse battery staple").is_ok());
    }

    #[test]
    fn breached_passwords_are_refused_even_when_long_enough() {
        assert!(validate_password("password1234").is_err());
        assert!(
            validate_password("PassWord1234").is_err(),
            "case-insensitive"
        );
    }

    #[test]
    fn a_space_is_not_a_symbol() {
        let strict = PasswordPolicy {
            require_symbol: true,
            ..PasswordPolicy::system_default()
        };

        // Otherwise the requirement ticks itself for anyone using a passphrase.
        assert!(strict.check("correct horse battery st").is_err());
        assert!(strict.check("correct horse battery s!").is_ok());
    }

    #[test]
    fn an_organization_can_demand_composition() {
        let strict = PasswordPolicy {
            require_uppercase: true,
            require_digit: true,
            require_symbol: true,
            ..PasswordPolicy::system_default()
        };

        // Accepted by the system default, refused here.
        assert!(validate_password("correct horse battery staple").is_ok());
        assert!(strict.check("correct horse battery staple").is_err());

        assert!(strict.check("Correct horse 9 battery!").is_ok());
    }

    #[test]
    fn an_organization_can_raise_but_not_drop_the_floor() {
        let stricter = PasswordPolicy {
            min_length: 16,
            ..PasswordPolicy::system_default()
        };
        assert!(stricter.validate().is_ok());
        assert!(stricter.check("z".repeat(12).as_str()).is_err());

        let too_loose = PasswordPolicy {
            min_length: ABSOLUTE_MIN_LENGTH - 1,
            ..PasswordPolicy::system_default()
        };
        let errors = too_loose.validate().unwrap_err();
        assert_eq!(errors[0].field, "min_length");
    }

    #[test]
    fn a_policy_that_would_lock_everyone_out_is_refused() {
        // "expires after zero days" expires every password the instant it is
        // saved, including the administrator's own.
        let suicide = PasswordPolicy {
            expiry_days: Some(0),
            ..PasswordPolicy::system_default()
        };
        assert!(suicide.validate().is_err());

        let inverted = PasswordPolicy {
            min_length: 40,
            max_length: 20,
            ..PasswordPolicy::system_default()
        };
        assert!(inverted.validate().is_err());

        let hoarder = PasswordPolicy {
            history_depth: MAX_HISTORY_DEPTH + 1,
            ..PasswordPolicy::system_default()
        };
        assert!(hoarder.validate().is_err());
    }

    #[test]
    fn expiry_is_off_unless_asked_for() {
        let now = chrono::Utc::now();
        let ancient = now - chrono::Duration::days(4000);

        assert!(!PasswordPolicy::system_default().is_expired(ancient, now));

        let expiring = PasswordPolicy {
            expiry_days: Some(90),
            ..PasswordPolicy::system_default()
        };
        assert!(expiring.is_expired(ancient, now));
        assert!(!expiring.is_expired(now - chrono::Duration::days(89), now));
    }

    #[test]
    fn the_checklist_matches_what_is_enforced() {
        let policy = PasswordPolicy {
            min_length: 14,
            require_digit: true,
            history_depth: 5,
            ..PasswordPolicy::system_default()
        };
        let lines = policy.requirements();

        assert!(lines[0].contains("14"));
        assert!(lines.iter().any(|line| line.contains("number")));
        assert!(lines.iter().any(|line| line.contains("last 5")));
        // Nothing is promised that `check` does not enforce.
        assert!(!lines.iter().any(|line| line.contains("symbol")));
    }

    #[test]
    fn strength_rewards_length_over_symbols() {
        let long_simple = password_strength("correct horse battery staple");
        let short_complex = password_strength("P@ssw0rd!23x");
        assert!(
            long_simple > short_complex,
            "{long_simple:?} should beat {short_complex:?}"
        );
    }

    #[test]
    fn strength_punishes_repetition() {
        assert_eq!(
            password_strength("aaaaaaaaaaaaaaaaaaaa"),
            PasswordStrength::Weak
        );
        assert_eq!(password_strength(""), PasswordStrength::Empty);
        assert_eq!(password_strength("short"), PasswordStrength::TooShort);
    }

    #[test]
    fn the_meter_uses_the_workspaces_own_minimum() {
        let strict = PasswordPolicy {
            min_length: 20,
            ..PasswordPolicy::system_default()
        };
        // Long enough for the system default, short for this workspace - and
        // the meter has to say so, or the form promises a submit that fails.
        assert_eq!(
            password_strength_for("correct horse", &strict),
            PasswordStrength::TooShort
        );
    }

    #[test]
    fn the_meter_has_a_bar_for_every_level() {
        assert_eq!(PasswordStrength::Empty.filled_bars(), 0);
        assert_eq!(PasswordStrength::Strong.filled_bars(), 4);
        assert!(!PasswordStrength::TooShort.is_acceptable());
        assert!(PasswordStrength::Fair.is_acceptable());
    }

    #[test]
    fn a_policy_round_trips_through_json() {
        let policy = PasswordPolicy {
            min_length: 16,
            require_symbol: true,
            expiry_days: Some(90),
            history_depth: 3,
            ..PasswordPolicy::system_default()
        };
        let json = serde_json::to_string(&policy).unwrap();
        assert_eq!(
            serde_json::from_str::<PasswordPolicy>(&json).unwrap(),
            policy
        );

        // Every field has a default, so a policy stored by an older release
        // still loads when a new field is added.
        assert_eq!(
            serde_json::from_str::<PasswordPolicy>("{}").unwrap(),
            PasswordPolicy::system_default()
        );
    }
}
