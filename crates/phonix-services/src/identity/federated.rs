//! Signing in on somebody else's word.
//!
//! [`sign_in_federated`] is [`super::authentication::sign_in`] with the
//! password step removed and nothing else removed with it. That sentence is the
//! whole design, and the temptation this module exists to resist is the other
//! reading - "the identity provider said yes, so open a session".
//!
//! # What a provider's yes does not cover
//!
//! Google vouches that whoever is at the browser controls an address. It knows
//! nothing about this workspace, so every decision the workspace makes for
//! itself still has to be made here:
//!
//! * **Membership.** The address must already belong to an account. There is no
//!   create-on-first-sign-in path, deliberately - see [`super::super::oauth`].
//! * **Status.** A suspended or deactivated account stays out. An administrator
//!   who closes an account expects it closed, not closed to one of two doors.
//! * **Lockout.** Checked, and for a reason that is easy to miss: without it,
//!   an account locked by password guessing has an unlocked second entrance.
//! * **Multi-factor.** A federated sign-in produces the same
//!   [`LoginResult`](phonix_core::identity::LoginResult) as any other, so a
//!   workspace that requires a second factor gets one. Google's own two-step
//!   verification is Google's policy about a Google account; it is not this
//!   workspace's policy about this account.
//! * **An expired password.** Still expired. The account lands on the
//!   change-password screen exactly as it would have.
//!
//! Everything after the credential check is therefore the *same code*:
//! `finish_sign_in` decides the outcome, opens the session or issues the
//! handoff, and writes the audit entry. A second implementation here would be
//! the place the two quietly stopped agreeing about what MFA means.
//!
//! # No lockout counter is touched
//!
//! A failed federated attempt does not increment `failed_login_count`. There is
//! nothing being guessed - the attempt failed because an address has no account
//! here, not because somebody got a secret wrong - and counting it would let
//! anyone with a Google account lock a member out by signing in repeatedly with
//! an address they know.

use chrono::Utc;
use phonix_db::identity::session::ClientFacts;
use phonix_db::identity::{AuditEntry, IdentityEvent, audit, user};
use phonix_db::sqlx::PgPool;

use super::authentication::{Delivery, SignedIn};
use crate::Security;
use crate::error::ServiceResult;

/// Which provider vouched, for the audit trail.
///
/// A `&'static str` rather than an enum with one variant: the value is written
/// to a JSON detail column and read by a person, and the second provider adds a
/// string rather than a migration.
pub const GOOGLE: &str = "google";

/// Open a session for an address an identity provider has vouched for.
///
/// **The caller must have verified the address itself.** This function takes it
/// on trust - that is what makes it federated - so everything upstream of it is
/// load-bearing: the token came from the provider over TLS, and the provider
/// said the address was verified. See
/// [`oauth::google`](super::super::oauth::google).
///
/// Returns `SignedIn::rejected()` for an address with no account here, which is
/// the same answer a wrong password gets and says nothing about which of the
/// two it was.
pub async fn sign_in_federated(
    pool: &PgPool,
    security: &Security<'_>,
    provider: &'static str,
    email: &str,
    remember_me: bool,
    facts: ClientFacts<'_>,
    delivery: Delivery,
) -> ServiceResult<SignedIn> {
    let email = email.trim().to_ascii_lowercase();
    let now = Utc::now();

    let Some(account) = user::find_by_email(pool, &email).await? else {
        // No dummy hash to spend here, unlike the password path: there is no
        // hash on either branch, so there is no difference in timing to hide.
        // What is worth recording is that somebody with a working Google
        // account tried an address this workspace has never heard of.
        audit::record_best_effort(
            pool,
            AuditEntry::new(IdentityEvent::Login, false)
                .email(&email)
                .client(facts.ip, facts.user_agent)
                .reason("no account with that address")
                .detail(serde_json::json!({ "provider": provider })),
        )
        .await;

        return Ok(SignedIn::rejected());
    };

    if account.is_locked(now) {
        audit::record_best_effort(
            pool,
            AuditEntry::new(IdentityEvent::Login, false)
                .user(account.id)
                .email(&email)
                .client(facts.ip, facts.user_agent)
                .reason("account is locked")
                .detail(serde_json::json!({ "provider": provider })),
        )
        .await;

        return Ok(SignedIn::locked(account.lockout_remaining_secs(now)));
    }

    if !account.status.can_sign_in() {
        audit::record_best_effort(
            pool,
            AuditEntry::new(IdentityEvent::Login, false)
                .user(account.id)
                .email(&email)
                .client(facts.ip, facts.user_agent)
                .reason(&format!("account is {}", account.status))
                .detail(serde_json::json!({ "provider": provider })),
        )
        .await;

        return Ok(SignedIn::rejected());
    }

    // A `Pending` account - invited, never accepted - has no password and is
    // caught by `can_sign_in` above. That is the right answer rather than a
    // convenience worth adding: letting Google finish an invitation would mean
    // an invitation could be accepted by somebody who never opened the link,
    // and the link is the proof that the address receives mail.

    user::record_successful_login(pool, account.id, facts.ip).await?;

    // Everything from here is shared with the password path on purpose. The
    // workspace's MFA policy, an expired password, the session or the handoff,
    // and the audit entry all come out the same - a federated sign-in is a
    // sign-in, and only the credential differed.
    super::authentication::finish_sign_in(
        pool,
        security,
        account,
        &email,
        remember_me,
        facts,
        delivery,
        Some(provider),
    )
    .await
}
