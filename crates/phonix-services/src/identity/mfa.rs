//! Enrolling a second factor, and answering a challenge with one.
//!
//! # Two rules
//!
//! **An unconfirmed factor satisfies nothing.** Enrolment writes a row and then
//! makes the user produce a code from it. Skipping that step is how somebody
//! ends up with a secret in an app they mistyped and no way back in.
//!
//! **A spent recovery code is deleted, not flagged.** There is then nothing
//! left to compare against, so no code path can accept it twice by forgetting
//! to check a column.
//!
//! # Where the secrecy lives
//!
//! A TOTP secret is sealed here and stored sealed; a recovery code is digested
//! here and stored as a digest. `phonix_db::identity::mfa` moves those bytes
//! and never interprets them - which is what stops a repository from being able
//! to produce a working code.

use phonix_config::MfaConfig;
use phonix_core::identity::UserId;
use phonix_core::identity::mfa::{
    MfaChallengeResult, MfaFactorSummary, MfaPolicy, MfaStatus, RecoveryCodes, TotpEnrolment,
};
use phonix_core::permissions;
use phonix_db::identity::mfa as store;
use phonix_db::identity::{AuditEntry, IdentityEvent, audit};
use phonix_db::sqlx::{PgExecutor, PgPool};
use secrecy::ExposeSecret;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::caller::Caller;
use crate::crypto::totp::{self, TotpParams};
use crate::crypto::vault::{KEY_VERSION, SecretVault, user_context};
use crate::error::{ServiceError, ServiceResult};
use phonix_core::msg;

/// Which factor answered a challenge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifiedFactor {
    /// A code from an authenticator app.
    Totp,
    /// A recovery code, now spent. Worth surfacing: a user falling back to
    /// these has lost their phone and needs to enrol again.
    RecoveryCode { remaining: usize },
}

// ---------------------------------------------------------------------------
// Enrolment
// ---------------------------------------------------------------------------

/// Start enrolling an authenticator app.
///
/// Returns the secret exactly once, in two forms: base32 for typing in by hand
/// and an `otpauth://` URI for the QR code. The factor is unconfirmed until
/// [`confirm_totp`] succeeds.
// Eight, and every one is load-bearing: three collaborators, the caller, whose
// factor it is, and the two strings that end up inside the QR code. Bundling
// them into a struct would move the same list one line up and cost the reader
// a second type to look at.
#[allow(clippy::too_many_arguments)]
pub async fn begin_totp_enrolment(
    pool: &PgPool,
    vault: &SecretVault,
    cfg: &MfaConfig,
    policy: &MfaPolicy,
    caller: &Caller,
    user_id: UserId,
    account_label: &str,
    workspace: &str,
) -> ServiceResult<TotpEnrolment> {
    // Enrolling your own factor needs no permission. Enrolling one *for*
    // somebody else means handing them a secret, so it needs `Users.Edit` -
    // and is why this is not simply "the caller's own id".
    caller.require_self_or(user_id, permissions::USERS_EDIT)?;

    if !policy.allows_enrolment() {
        return Err(ServiceError::rejected(
            "mfa",
            msg!("error.mfa.totp_not_allowed"),
        ));
    }

    let secret = totp::generate_secret(cfg.secret_bytes);
    let sealed = vault
        .seal(&secret, &user_context(user_id))
        .map_err(|err| ServiceError::Crypto(err.to_string()))?;

    let factor_id = store::insert_unconfirmed_totp(
        pool,
        user_id,
        &format!("Authenticator app ({workspace})"),
        &sealed,
        i16::from(KEY_VERSION),
    )
    .await?;

    let params = TotpParams::from_config(cfg);

    // The issuer carries the workspace, so somebody with accounts in two of
    // them gets two distinguishable entries rather than two called "Phonix".
    let issuer = format!("{} ({workspace})", cfg.issuer);

    Ok(TotpEnrolment {
        factor_id,
        secret_base32: totp::encode_secret(&secret),
        provisioning_uri: totp::provisioning_uri(&issuer, account_label, &secret, params),
        digits: params.digits,
        period_secs: params.step_secs,
    })
}

/// Finish enrolment by proving a code can be produced from the new secret.
///
/// Returns `false` for a wrong code, leaving the row unconfirmed so the user
/// can try again from the same screen.
pub async fn confirm_totp(
    pool: &PgPool,
    vault: &SecretVault,
    cfg: &MfaConfig,
    user_id: UserId,
    factor_id: Uuid,
    submitted_code: &str,
) -> ServiceResult<bool> {
    let Some(pending) = store::pending_totp(pool, user_id, factor_id).await? else {
        return Ok(false);
    };

    let secret = vault
        .open(&pending.material, &user_context(user_id))
        .map_err(|err| ServiceError::Crypto(err.to_string()))?;

    let matched = totp::verify(
        secret.expose_secret(),
        submitted_code,
        now_unix(),
        TotpParams::from_config(cfg),
    );
    if matched.is_none() {
        return Ok(false);
    }

    store::confirm_factor(pool, user_id, factor_id).await?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Challenge
// ---------------------------------------------------------------------------

/// Check a code submitted at the challenge screen.
///
/// Tries the authenticator app first and falls back to recovery codes, decided
/// by what matches rather than by a field the caller controls - a form that let
/// the client say "this is a recovery code" would let an attacker aim their
/// guesses at whichever store is weaker.
///
/// Returns `None` when nothing matched. Counting that against an attempt budget
/// is [`answer_challenge`]'s job, because this function is also used by the
/// "confirm it's you" flows where consuming an attempt would be wrong.
pub async fn verify_code(
    pool: &PgPool,
    vault: &SecretVault,
    cfg: &MfaConfig,
    user_id: UserId,
    submitted: &str,
) -> ServiceResult<Option<VerifiedFactor>> {
    if let Some(factor_id) = verify_totp(pool, vault, cfg, user_id, submitted).await? {
        store::touch_factor(pool, factor_id).await?;
        return Ok(Some(VerifiedFactor::Totp));
    }

    let cleaned = normalise_recovery_code(submitted);
    if cleaned.len() == RECOVERY_CODE_LEN
        && store::consume_recovery_code(pool, user_id, &digest_code(&cleaned)).await?
    {
        let remaining = store::count_recovery_codes(pool, user_id).await?;
        tracing::warn!(
            %user_id,
            remaining,
            "a recovery code was used; the user has lost access to their authenticator"
        );
        return Ok(Some(VerifiedFactor::RecoveryCode { remaining }));
    }

    Ok(None)
}

/// Answer the challenge attached to a half-authenticated session.
///
/// This is the one that spends attempts. The budget exists because a six-digit
/// code is a million guesses and each one costs an HMAC rather than an Argon2
/// hash - without a cap the second factor is a formality.
pub async fn answer_challenge(
    pool: &PgPool,
    vault: &SecretVault,
    cfg: &MfaConfig,
    session_id: Uuid,
    user_id: UserId,
    submitted: &str,
) -> ServiceResult<MfaChallengeResult> {
    use phonix_db::identity::session;

    let Some(state) = session::challenge_state(pool, session_id).await? else {
        return Ok(MfaChallengeResult::NoChallenge);
    };
    if state.expired {
        session::revoke_by_id(pool, session_id, "MFA challenge expired").await?;
        return Ok(MfaChallengeResult::NoChallenge);
    }

    if verify_code(pool, vault, cfg, user_id, submitted)
        .await?
        .is_none()
    {
        let attempts = session::record_mfa_attempt(pool, session_id).await?;
        let remaining = cfg
            .max_challenge_attempts
            .saturating_sub(attempts.max(0) as u32);

        if remaining == 0 {
            // The password was proven, so the session is real - which is
            // exactly why it cannot be left sitting at a code box being
            // guessed at. Destroy it and make them start again.
            session::revoke_by_id(pool, session_id, "too many MFA attempts").await?;
            return Ok(MfaChallengeResult::Exhausted);
        }

        return Ok(MfaChallengeResult::Rejected {
            attempts_remaining: remaining,
        });
    }

    session::mark_mfa_satisfied(pool, session_id).await?;

    let auth_user = super::authentication::load_auth_user_by_id(pool, user_id, true)
        .await?
        .ok_or(ServiceError::Unauthenticated)?;

    Ok(MfaChallengeResult::Accepted(Box::new(auth_user)))
}

async fn verify_totp(
    pool: &PgPool,
    vault: &SecretVault,
    cfg: &MfaConfig,
    user_id: UserId,
    submitted: &str,
) -> ServiceResult<Option<Uuid>> {
    let Some(stored) = store::confirmed_totp(pool, user_id).await? else {
        return Ok(None);
    };

    // A row that will not decrypt is a broken enrolment, not a wrong code: the
    // key changed under it. Logged loudly and treated as no match, because
    // failing the whole sign-in would strand the user with no path back either.
    let secret = match vault.open(&stored.material, &user_context(user_id)) {
        Ok(secret) => secret,
        Err(err) => {
            tracing::error!(%user_id, error = %err, "stored TOTP secret could not be decrypted");
            return Ok(None);
        }
    };

    let matched = totp::verify(
        secret.expose_secret(),
        submitted,
        now_unix(),
        TotpParams::from_config(cfg),
    );

    Ok(matched.map(|_| stored.id))
}

// ---------------------------------------------------------------------------
// Recovery codes
// ---------------------------------------------------------------------------

/// Characters recovery codes are drawn from.
///
/// No `0`, `1`, `o`, `l` or `i`: these get read off a printout and typed back
/// in, and the pairs people confuse cost more than the two bits they save.
const RECOVERY_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// Characters per code, printed as two groups of five.
const RECOVERY_CODE_LEN: usize = 10;

/// Issue a fresh set of recovery codes, replacing any that are outstanding.
pub async fn generate_recovery_codes(
    pool: &PgPool,
    policy: &MfaPolicy,
    caller: &Caller,
    user_id: UserId,
    count: usize,
) -> ServiceResult<RecoveryCodes> {
    caller.require_self_or(user_id, permissions::USERS_EDIT)?;

    if !policy.allow_recovery_codes {
        return Err(ServiceError::rejected(
            "recovery_codes",
            msg!("error.mfa.recovery_not_allowed"),
        ));
    }

    let codes: Vec<String> = (0..count).map(|_| random_recovery_code()).collect();
    let digests: Vec<Vec<u8>> = codes.iter().map(|code| digest_code(code)).collect();

    store::replace_recovery_codes(pool, user_id, &digests, Uuid::now_v7()).await?;

    Ok(RecoveryCodes {
        codes: codes.iter().map(|code| format_code(code)).collect(),
        generated_at: chrono::Utc::now(),
    })
}

fn random_recovery_code() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};

    // Rejection sampling rather than `% alphabet.len()`: the modulo would make
    // the first few characters of the alphabet slightly likelier, and "slightly
    // biased CSPRNG output" is not a phrase worth having in a credential.
    let mut code = String::with_capacity(RECOVERY_CODE_LEN);
    let limit = (256 / RECOVERY_ALPHABET.len() * RECOVERY_ALPHABET.len()) as u8;

    while code.len() < RECOVERY_CODE_LEN {
        let mut byte = [0u8; 1];
        OsRng.fill_bytes(&mut byte);
        if byte[0] < limit {
            code.push(RECOVERY_ALPHABET[byte[0] as usize % RECOVERY_ALPHABET.len()] as char);
        }
    }

    code
}

/// `abcde-fghij`, which is how it is printed.
fn format_code(code: &str) -> String {
    format!("{}-{}", &code[..5], &code[5..])
}

/// Undo [`format_code`], and forgive case and stray spaces.
fn normalise_recovery_code(submitted: &str) -> String {
    submitted
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

fn digest_code(code: &str) -> Vec<u8> {
    Sha256::digest(code.as_bytes()).to_vec()
}

/// Constant-time digest comparison, for any path holding both sides.
///
/// The lookup above compares in the database, where a 50-bit random string has
/// no prefix to leak; this exists so there is one obvious right way to do it
/// anywhere else.
pub fn digests_match(left: &[u8], right: &[u8]) -> bool {
    left.ct_eq(right).into()
}

// ---------------------------------------------------------------------------
// Removal
// ---------------------------------------------------------------------------

/// Remove one second factor.
///
/// Refused while the workspace requires a factor and this is the last one -
/// otherwise turning off your own two-factor authentication under a `Required`
/// policy locks you out on the next sign-in, and the screen that let you do it
/// looked like it was working.
///
/// Removing somebody else's is `Users.Edit`, and is how an administrator
/// answers "I lost my phone".
pub async fn remove_factor(
    pool: &PgPool,
    policy: &MfaPolicy,
    caller: &Caller,
    user_id: UserId,
    factor_id: Uuid,
    account_age_days: i64,
) -> ServiceResult<bool> {
    caller.require_self_or(user_id, permissions::USERS_EDIT)?;

    let remaining_after = store::list_factors(pool, user_id)
        .await?
        .iter()
        .filter(|factor| factor.confirmed && factor.id != factor_id)
        .count();

    if remaining_after == 0 && !policy.allows_sign_in_without_factor(account_age_days) {
        return Err(ServiceError::rejected(
            "factor",
            msg!("error.mfa.last_factor"),
        ));
    }

    let removed = store::remove_factor(pool, user_id, factor_id).await?;

    if removed {
        audit::record_best_effort(
            pool,
            AuditEntry::new(IdentityEvent::MfaRemoved, true)
                .user(user_id)
                .detail(serde_json::json!({
                    "factor_id": factor_id,
                    "removed_by": caller.user_id(),
                    "factors_remaining": remaining_after,
                })),
        )
        .await;

        tracing::info!(%user_id, %factor_id, remaining_after, "second factor removed");
    }

    Ok(removed)
}

/// Remove every factor a user holds.
///
/// The administrator's answer to "I lost my phone *and* my recovery codes". Not
/// available for your own account: somebody who still has a session does not
/// need this, and offering it there is offering a way to lock yourself out.
pub async fn reset_factors(pool: &PgPool, caller: &Caller, user_id: UserId) -> ServiceResult<u64> {
    caller.require(permissions::USERS_EDIT)?;

    let removed = store::reset_all_factors(pool, user_id).await?;

    audit::record_best_effort(
        pool,
        AuditEntry::new(IdentityEvent::MfaRemoved, true)
            .user(user_id)
            .detail(serde_json::json!({
                "reset_by": caller.user_id(),
                "factors_removed": removed,
            })),
    )
    .await;

    tracing::warn!(
        %user_id,
        reset_by = ?caller.user_id(),
        removed,
        "every second factor reset by an administrator"
    );

    Ok(removed)
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Everything the security screen shows about a user's second factor.
pub async fn status<'e, E>(
    executor: E,
    policy: &MfaPolicy,
    user_id: UserId,
    account_age_days: i64,
) -> ServiceResult<MfaStatus>
where
    E: PgExecutor<'e> + Copy,
{
    let factors: Vec<MfaFactorSummary> = store::list_factors(executor, user_id).await?;
    let enabled = factors.iter().any(|factor| factor.confirmed);
    let recovery_codes_remaining = store::count_recovery_codes(executor, user_id).await?;

    let grace_days_remaining = (!enabled)
        .then(|| i64::from(policy.grace_period_days) - account_age_days)
        .filter(|remaining| *remaining > 0);

    Ok(MfaStatus {
        policy: policy.clone(),
        enabled,
        enrolment_required: !enabled && !policy.allows_sign_in_without_factor(account_age_days),
        grace_days_remaining,
        factors,
        recovery_codes_remaining,
    })
}

fn now_unix() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recovery_code_is_readable_off_a_printout() {
        let code = random_recovery_code();

        assert_eq!(code.len(), RECOVERY_CODE_LEN);
        for confusable in ['0', '1', 'o', 'l', 'i'] {
            assert!(!code.contains(confusable), "{code} contains {confusable}");
        }
        assert!(format_code(&code).contains('-'));
        assert_eq!(format_code(&code).len(), RECOVERY_CODE_LEN + 1);
    }

    #[test]
    fn a_code_typed_back_in_any_reasonable_way_still_matches() {
        let code = random_recovery_code();
        let printed = format_code(&code);

        for typed in [
            printed.clone(),
            printed.to_uppercase(),
            printed.replace('-', ""),
            printed.replace('-', " "),
            format!("  {printed}  "),
        ] {
            assert_eq!(normalise_recovery_code(&typed), code, "{typed:?}");
        }
    }

    #[test]
    fn recovery_codes_do_not_repeat() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1_000 {
            assert!(seen.insert(random_recovery_code()));
        }
    }

    #[test]
    fn the_stored_form_is_a_digest_not_the_code() {
        let code = random_recovery_code();
        let digest = digest_code(&code);

        assert_eq!(digest.len(), 32);
        assert_ne!(digest, code.as_bytes());
        assert!(digests_match(&digest, &digest_code(&code)));
        assert!(!digests_match(
            &digest,
            &digest_code(&random_recovery_code())
        ));
    }

    #[test]
    fn the_alphabet_is_large_enough_to_be_worth_typing() {
        // 31 characters, 10 of them, is about 50 bits - past guessing, which is
        // why these are digested rather than hashed with Argon2.
        assert_eq!(RECOVERY_ALPHABET.len(), 31);
        let entropy_bits = (RECOVERY_ALPHABET.len() as f64).log2() * RECOVERY_CODE_LEN as f64;
        assert!(entropy_bits > 48.0, "{entropy_bits} bits is not enough");
    }

    #[test]
    fn a_submission_of_the_wrong_length_never_reaches_a_query() {
        for bad in ["", "abc", &"a".repeat(9), &"a".repeat(11)] {
            assert_ne!(normalise_recovery_code(bad).len(), RECOVERY_CODE_LEN);
        }
    }
}
