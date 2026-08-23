//! Multi-factor authentication: the policy, the factors, and the challenge.
//!
//! What lives here is only the vocabulary the browser and the server share. The
//! TOTP arithmetic, the encrypted secrets and the challenge bookkeeping are in
//! `phonix_db::identity::mfa` and `phonix_db::identity::totp`, which never
//! compile to wasm - the client has no business being able to produce a code.
//!
//! # Who decides
//!
//! ```text
//! [security.mfa] in config    ..  the parameters (issuer, digits, step, skew)
//! MfaPolicy on the workspace  ..  whether users must enrol at all
//! user_mfa_factors rows       ..  what a given user actually holds
//! ```
//!
//! The organization decides enforcement; the deployment decides the arithmetic.
//! An organization cannot weaken the code length or widen the acceptance
//! window, because those are not preferences - a six-digit code with a
//! ten-step window is a different security claim, not a different taste.

use serde::{Deserialize, Serialize};

use super::user::UserId;
use crate::{Message, msg};

/// How hard a workspace pushes its users onto a second factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MfaEnforcement {
    /// Nobody may enrol. Existing factors stop being asked for.
    ///
    /// Deliberately available: an organization that has decided its users will
    /// authenticate through an upstream identity provider does not want a
    /// second, unmanaged factor in the way.
    Disabled,
    /// Users may enrol if they want to. The default.
    #[default]
    Optional,
    /// Every user must hold a confirmed factor. Those who do not are sent to
    /// enrolment on their next sign-in and can reach nothing else until they
    /// finish - see [`MfaPolicy::grace_period_days`] for the escape hatch.
    Required,
}

impl MfaEnforcement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Optional => "optional",
            Self::Required => "required",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "disabled" => Some(Self::Disabled),
            "optional" => Some(Self::Optional),
            "required" => Some(Self::Required),
            _ => None,
        }
    }

    /// What the settings screen shows next to each choice.
    /// The level's name, for a person choosing between them.
    ///
    /// Not [`as_str`](Self::as_str), which is the value in the column and in
    /// the JSON on the wire. Those two must never be the same string: the day
    /// somebody translates the enum, every stored row becomes unreadable.
    pub fn name(self) -> Message {
        match self {
            Self::Disabled => msg!("mfa.enforcement.disabled.name"),
            Self::Optional => msg!("mfa.enforcement.optional.name"),
            Self::Required => msg!("mfa.enforcement.required.name"),
        }
    }

    pub fn description(self) -> Message {
        match self {
            Self::Disabled => msg!("mfa.enforcement.disabled.description"),
            Self::Optional => msg!("mfa.enforcement.optional.description"),
            Self::Required => msg!("mfa.enforcement.required.description"),
        }
    }
}

impl std::fmt::Display for MfaEnforcement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What one organization requires of its users' second factor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MfaPolicy {
    pub enforcement: MfaEnforcement,

    /// Authenticator apps. The only method implemented today; the flag exists
    /// so an organization standardising on hardware keys can turn it off once
    /// WebAuthn lands.
    pub allow_totp: bool,

    /// One-time recovery codes, for the phone that fell in the sea.
    ///
    /// Turning this off is a real choice with a real consequence: an
    /// administrator then has to reset a locked-out user by hand, which is
    /// exactly what some organizations want and others cannot staff.
    pub allow_recovery_codes: bool,

    /// Days a user may keep signing in without enrolling after `Required` is
    /// turned on. `0` means no grace at all.
    ///
    /// Without this, switching to `Required` locks out everybody who is not at
    /// their desk with their phone, including whoever flipped the switch.
    pub grace_period_days: u32,

    /// Days a browser may skip the challenge after passing it once. `0`
    /// disables the feature, and is the default: "remember this device" is a
    /// convenience that quietly extends the trust window.
    pub remember_device_days: u32,
}

impl Default for MfaPolicy {
    fn default() -> Self {
        Self::system_default()
    }
}

impl MfaPolicy {
    /// What a workspace gets on the day it is created.
    pub const fn system_default() -> Self {
        Self {
            enforcement: MfaEnforcement::Optional,
            allow_totp: true,
            allow_recovery_codes: true,
            grace_period_days: 7,
            remember_device_days: 0,
        }
    }

    /// Check a policy an administrator submitted.
    pub fn validate(&self) -> Result<(), Vec<super::validation::FieldError>> {
        use super::validation::FieldError;
        let mut errors = Vec::new();

        // A policy that demands a factor while permitting no method to hold one
        // locks every user out of their own workspace on their next sign-in.
        if self.enforcement == MfaEnforcement::Required && !self.allow_totp {
            errors.push(FieldError::new(
                "allow_totp",
                msg!("validation.mfa.no_method"),
            ));
        }
        if self.grace_period_days > MAX_GRACE_PERIOD_DAYS {
            errors.push(FieldError::new(
                "grace_period_days",
                msg!("validation.mfa.grace_too_long", max = MAX_GRACE_PERIOD_DAYS),
            ));
        }
        if self.remember_device_days > MAX_REMEMBER_DEVICE_DAYS {
            errors.push(FieldError::new(
                "remember_device_days",
                msg!(
                    "validation.mfa.remember_too_long",
                    max = MAX_REMEMBER_DEVICE_DAYS
                ),
            ));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Whether a user holding no confirmed factor may still sign in.
    ///
    /// `enrolled_since` is how long the account has existed, in days. Under
    /// `Required` the answer is yes only while the grace period runs.
    pub fn allows_sign_in_without_factor(&self, account_age_days: i64) -> bool {
        match self.enforcement {
            MfaEnforcement::Disabled | MfaEnforcement::Optional => true,
            MfaEnforcement::Required => account_age_days < i64::from(self.grace_period_days),
        }
    }

    /// Whether a confirmed factor should be challenged at sign-in.
    ///
    /// `Disabled` means existing factors are ignored rather than deleted: an
    /// organization that turns enforcement off and on again should not have
    /// destroyed everybody's enrolment in between.
    pub fn challenges_existing_factors(&self) -> bool {
        self.enforcement != MfaEnforcement::Disabled
    }

    /// Whether a user may enrol a new factor right now.
    pub fn allows_enrolment(&self) -> bool {
        self.enforcement != MfaEnforcement::Disabled && self.allow_totp
    }
}

/// The longest grace period an organization may configure.
pub const MAX_GRACE_PERIOD_DAYS: u32 = 90;

/// The longest a device may be remembered.
pub const MAX_REMEMBER_DEVICE_DAYS: u32 = 90;

/// A kind of second factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MfaFactorKind {
    /// An authenticator app holding a shared secret (RFC 6238).
    Totp,
    /// A hardware or platform authenticator. Reserved; the storage exists but
    /// nothing issues these yet.
    WebAuthn,
    /// A printed one-time code.
    RecoveryCode,
}

impl MfaFactorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Totp => "totp",
            Self::WebAuthn => "webauthn",
            Self::RecoveryCode => "recovery_code",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "totp" => Some(Self::Totp),
            "webauthn" => Some(Self::WebAuthn),
            "recovery_code" => Some(Self::RecoveryCode),
            _ => None,
        }
    }

    pub fn label(self) -> Message {
        match self {
            Self::Totp => msg!("mfa.method.totp"),
            Self::WebAuthn => msg!("mfa.method.webauthn"),
            Self::RecoveryCode => msg!("mfa.method.recovery_code"),
        }
    }
}

impl std::fmt::Display for MfaFactorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One of a user's factors, as the security screen lists it.
///
/// Carries no secret, no digest and no credential id - only what a person needs
/// to recognise the thing they set up and decide whether to remove it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MfaFactorSummary {
    pub id: uuid::Uuid,
    pub kind: MfaFactorKind,
    pub label: String,
    pub confirmed: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// A started, not yet confirmed, TOTP enrolment.
///
/// Everything here is shown once and never again. The factor is not usable
/// until the user proves they can produce a code from it, which is what stops
/// somebody enrolling a secret they mistyped and locking themselves out.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpEnrolment {
    pub factor_id: uuid::Uuid,
    /// The shared secret, RFC 4648 base32, for typing in by hand.
    pub secret_base32: String,
    /// `otpauth://totp/...`, for the QR code.
    pub provisioning_uri: String,
    pub digits: u8,
    pub period_secs: u64,
}

/// The state of a user's second factor, for the account screen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MfaStatus {
    pub policy: MfaPolicy,
    pub enabled: bool,
    /// Set when the workspace requires a factor this user does not hold yet.
    pub enrolment_required: bool,
    /// Days left of the grace period, when one is running.
    pub grace_days_remaining: Option<i64>,
    pub factors: Vec<MfaFactorSummary>,
    pub recovery_codes_remaining: usize,
}

/// The outcome of answering a challenge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MfaChallengeResult {
    /// The session is now fully authenticated.
    Accepted(Box<super::user::AuthUser>),
    /// Wrong code, or a recovery code already spent.
    Rejected {
        /// Attempts left before the session is thrown away. Stated because the
        /// caller has already proved they know the password - the count is not
        /// a secret from them, and a silent lockout is worse than a warning.
        attempts_remaining: u32,
    },
    /// Too many wrong codes. The half-authenticated session is gone and the
    /// password has to be entered again.
    Exhausted,
    /// The half-authenticated session expired, was revoked, or never existed.
    NoChallenge,
}

impl MfaChallengeResult {
    /// Message to show under the code field.
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Accepted(_) => None,
            Self::Rejected { attempts_remaining } => Some(format!(
                "That code is not right. {attempts_remaining} attempt{} left.",
                if *attempts_remaining == 1 { "" } else { "s" }
            )),
            Self::Exhausted => {
                Some("Too many incorrect codes. Sign in with your password again.".to_owned())
            }
            Self::NoChallenge => {
                Some("This sign-in has expired. Enter your password again.".to_owned())
            }
        }
    }
}

/// A submitted challenge answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MfaChallenge {
    pub user_id: UserId,
    /// Digits from an authenticator app, or a recovery code. Which one it is
    /// is decided by shape server-side rather than by a radio button the
    /// caller controls.
    pub code: String,
}

/// A freshly generated set of recovery codes.
///
/// Returned once, at generation. The server keeps only digests, so a user who
/// loses these has to generate a new set, not recover the old one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryCodes {
    pub codes: Vec<String>,
    pub generated_at: chrono::DateTime<chrono::Utc>,
}

impl RecoveryCodes {
    /// The codes as a plain text file, for the "download" button.
    pub fn as_text(&self, workspace: &str) -> String {
        let mut out = format!(
            "Recovery codes for {workspace}\nGenerated {}\n\n\
             Each code works once. Keep them somewhere you can reach without your phone.\n\n",
            self.generated_at.format("%Y-%m-%d %H:%M UTC")
        );
        for code in &self.codes {
            out.push_str(code);
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_is_what_a_new_workspace_gets() {
        let policy = MfaPolicy::system_default();
        assert_eq!(policy.enforcement, MfaEnforcement::Optional);
        assert!(policy.allows_enrolment());
        assert!(policy.challenges_existing_factors());
        // Nobody is blocked from signing in on day one.
        assert!(policy.allows_sign_in_without_factor(0));
        assert!(policy.allows_sign_in_without_factor(10_000));
    }

    #[test]
    fn required_blocks_only_after_the_grace_period() {
        let policy = MfaPolicy {
            enforcement: MfaEnforcement::Required,
            grace_period_days: 7,
            ..MfaPolicy::system_default()
        };

        assert!(policy.allows_sign_in_without_factor(0));
        assert!(policy.allows_sign_in_without_factor(6));
        assert!(!policy.allows_sign_in_without_factor(7));
        assert!(!policy.allows_sign_in_without_factor(400));
    }

    #[test]
    fn required_with_no_grace_blocks_immediately() {
        let policy = MfaPolicy {
            enforcement: MfaEnforcement::Required,
            grace_period_days: 0,
            ..MfaPolicy::system_default()
        };
        assert!(!policy.allows_sign_in_without_factor(0));
    }

    #[test]
    fn a_policy_nobody_could_satisfy_is_refused() {
        // Required, but no method allowed to satisfy it.
        let impossible = MfaPolicy {
            enforcement: MfaEnforcement::Required,
            allow_totp: false,
            ..MfaPolicy::system_default()
        };
        let errors = impossible.validate().unwrap_err();
        assert_eq!(errors[0].field, "allow_totp");

        // The same combination is fine when nothing is required.
        let fine = MfaPolicy {
            enforcement: MfaEnforcement::Optional,
            allow_totp: false,
            ..MfaPolicy::system_default()
        };
        assert!(fine.validate().is_ok());
    }

    #[test]
    fn turning_enforcement_off_does_not_destroy_enrolments() {
        let off = MfaPolicy {
            enforcement: MfaEnforcement::Disabled,
            ..MfaPolicy::system_default()
        };
        // Existing factors stop being challenged, and no new ones are taken -
        // but nothing here deletes anything.
        assert!(!off.challenges_existing_factors());
        assert!(!off.allows_enrolment());
        assert!(off.allows_sign_in_without_factor(10_000));
    }

    #[test]
    fn enforcement_survives_the_round_trip_through_the_database() {
        for enforcement in [
            MfaEnforcement::Disabled,
            MfaEnforcement::Optional,
            MfaEnforcement::Required,
        ] {
            assert_eq!(
                MfaEnforcement::parse(enforcement.as_str()),
                Some(enforcement)
            );
        }
        assert_eq!(MfaEnforcement::parse("mandatory"), None);
    }

    #[test]
    fn factor_kinds_match_the_check_constraint() {
        // These strings are written into `user_mfa_factors.kind`, which has a
        // CHECK constraint listing exactly these three.
        for kind in [
            MfaFactorKind::Totp,
            MfaFactorKind::WebAuthn,
            MfaFactorKind::RecoveryCode,
        ] {
            assert_eq!(MfaFactorKind::parse(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn a_rejection_says_how_many_tries_are_left() {
        let message = MfaChallengeResult::Rejected {
            attempts_remaining: 1,
        }
        .message()
        .unwrap();
        assert!(message.contains("1 attempt left"), "{message}");

        let plural = MfaChallengeResult::Rejected {
            attempts_remaining: 3,
        }
        .message()
        .unwrap();
        assert!(plural.contains("3 attempts left"), "{plural}");
    }

    #[test]
    fn recovery_codes_print_with_their_workspace() {
        let codes = RecoveryCodes {
            codes: vec!["abcd-efgh".to_owned(), "ijkl-mnop".to_owned()],
            generated_at: chrono::Utc::now(),
        };
        let text = codes.as_text("acme");
        assert!(text.contains("acme"));
        assert!(text.contains("abcd-efgh"));
        assert!(text.contains("ijkl-mnop"));
    }

    #[test]
    fn a_policy_round_trips_through_json() {
        let policy = MfaPolicy {
            enforcement: MfaEnforcement::Required,
            grace_period_days: 14,
            ..MfaPolicy::system_default()
        };
        let json = serde_json::to_string(&policy).unwrap();
        assert_eq!(serde_json::from_str::<MfaPolicy>(&json).unwrap(), policy);

        // A policy stored before a field existed still loads.
        assert_eq!(
            serde_json::from_str::<MfaPolicy>("{}").unwrap(),
            MfaPolicy::system_default()
        );
    }
}
