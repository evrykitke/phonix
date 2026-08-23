//! Signing in: the one path where every defence has to line up.
//!
//! Five rules govern everything below.
//!
//! **Rejections are indistinguishable.** No such address, wrong password,
//! suspended account, soft-deleted account - all return
//! [`LoginResult::Rejected`]. Any difference the caller can observe turns the
//! form into an oracle for which addresses have accounts. The real reason goes
//! to `identity_events`.
//!
//! **Rejections cost the same.** A missing account still pays for a full
//! Argon2 verification against a dummy hash. Without that, "no such user"
//! returns in microseconds while "wrong password" takes 50 ms, and the
//! difference is trivially measurable over the network.
//!
//! **The lockout is checked before the password.** Otherwise a locked account
//! is still a working password oracle, just a slow one.
//!
//! **A successful sign-in is the only chance to upgrade a hash.** It is the one
//! moment the plaintext exists, so a hash made with weaker parameters is
//! re-made here or never.
//!
//! **The workspace's own policy is applied last.** A correct password can still
//! end at the enrolment screen or the change-password screen, because the
//! organization requires a second factor or expires passwords. Those are
//! outcomes, not rejections: the session exists, it just cannot reach anything
//! else yet.

use chrono::Utc;
use phonix_core::WorkspaceSecuritySettings;
use phonix_core::identity::{AuthUser, Credentials, LoginResult, UserId};
use phonix_db::identity::session::ClientFacts;
use phonix_db::identity::user::UserRecord;
use phonix_db::identity::{AuditEntry, IdentityEvent};
use phonix_db::identity::{audit, mfa as mfa_store, session as session_store, user};
use phonix_db::sqlx::PgPool;
use phonix_db::{authorization, settings as settings_store};
use secrecy::SecretString;

use crate::Security;
use crate::error::{ServiceError, ServiceResult};
use crate::identity::session as session_service;

/// How a successful sign-in reaches the browser.
///
/// Session cookies are host-only, which is what stops one workspace's server
/// from receiving another's token. The cost is that a sign-in form running
/// somewhere other than the workspace's own host cannot set the cookie itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Open the session now and hand back its token. For a sign-in on the
    /// workspace's own host, where the cookie can simply be set.
    Cookie,
    /// Open nothing; hand back a single-use token for the browser to redeem at
    /// [`redeem_handoff`] on the workspace host. For the bare domain, and for
    /// the end of signup.
    Handoff,
}

/// A completed sign-in.
pub struct SignedIn {
    pub result: LoginResult,
    /// The session token to put in a cookie. `None` unless the password was
    /// accepted and delivery was [`Delivery::Cookie`].
    pub token: Option<SecretString>,
    /// Seconds the cookie should live, matching the session's absolute
    /// deadline.
    pub max_age_secs: i64,
    /// Single-use token to carry to the workspace host. `None` unless delivery
    /// was [`Delivery::Handoff`].
    pub handoff: Option<SecretString>,
}

impl SignedIn {
    fn rejected() -> Self {
        Self {
            result: LoginResult::Rejected,
            token: None,
            max_age_secs: 0,
            handoff: None,
        }
    }

    fn locked(retry_after_secs: u64) -> Self {
        Self {
            result: LoginResult::Locked { retry_after_secs },
            token: None,
            max_age_secs: 0,
            handoff: None,
        }
    }
}

/// Verify credentials and open a session.
///
/// `pool` is the tenant's own database, so this only ever authenticates against
/// the workspace the request was routed to.
pub async fn sign_in(
    pool: &PgPool,
    security: &Security<'_>,
    credentials: &Credentials,
    facts: ClientFacts<'_>,
    delivery: Delivery,
) -> ServiceResult<SignedIn> {
    let email = credentials.email.trim().to_ascii_lowercase();
    let password = SecretString::from(credentials.password.clone());
    let now = Utc::now();

    let Some(account) = user::find_by_email(pool, &email).await? else {
        // Spend the time a real verification would, so the absence of an
        // account is not visible in the response time.
        security.hasher.verify_dummy(&password).await;

        audit::record_best_effort(
            pool,
            AuditEntry::new(IdentityEvent::Login, false)
                .email(&email)
                .client(facts.ip, facts.user_agent)
                .reason("no account with that address"),
        )
        .await;

        return Ok(SignedIn::rejected());
    };

    // Before the password check: a locked account must not be usable to test
    // passwords, however slowly.
    if account.is_locked(now) {
        security.hasher.verify_dummy(&password).await;

        audit::record_best_effort(
            pool,
            AuditEntry::new(IdentityEvent::Login, false)
                .user(account.id)
                .email(&email)
                .client(facts.ip, facts.user_agent)
                .reason("account is locked"),
        )
        .await;

        return Ok(SignedIn::locked(account.lockout_remaining_secs(now)));
    }

    // A pending, suspended or deactivated account. Still pays for the hash, and
    // still reports the same thing a wrong password would.
    let Some(stored_hash) = account.password_hash.clone() else {
        security.hasher.verify_dummy(&password).await;
        return Ok(reject_and_audit(pool, &account, &email, &facts, "no password set").await);
    };
    if !account.status.can_sign_in() {
        security.hasher.verify_dummy(&password).await;
        let reason = format!("account is {}", account.status);
        return Ok(reject_and_audit(pool, &account, &email, &facts, &reason).await);
    }

    let matched = security
        .hasher
        .verify(&password, &stored_hash)
        .await
        .map_err(|err| ServiceError::Crypto(err.to_string()))?;

    if !matched {
        let locked_until =
            user::record_failed_login(pool, account.id, &security.config.lockout).await?;

        if let Some(until) = locked_until {
            audit::record_best_effort(
                pool,
                AuditEntry::new(IdentityEvent::AccountLocked, true)
                    .user(account.id)
                    .email(&email)
                    .client(facts.ip, facts.user_agent)
                    .detail(serde_json::json!({ "until": until })),
            )
            .await;

            // Signing out everywhere on lockout: if the failures are somebody
            // else guessing, any session they already hold should go too.
            session_store::revoke_all_for_user(pool, account.id, "account locked").await?;

            return Ok(SignedIn::locked(
                (until - Utc::now()).num_seconds().max(0) as u64
            ));
        }

        return Ok(reject_and_audit(pool, &account, &email, &facts, "wrong password").await);
    }

    // --- Authenticated ----------------------------------------------------

    // The only moment the plaintext is available, so the only moment a hash
    // made under weaker parameters can be upgraded.
    if security.hasher.needs_rehash(&stored_hash) {
        match security.hasher.hash(&password).await {
            Ok(upgraded) => {
                user::set_password_hash(pool, account.id, &upgraded).await?;
                tracing::info!(user_id = %account.id, "password hash upgraded to current parameters");
            }
            // Not fatal: the sign-in already succeeded and the old hash is
            // still valid, just cheaper than it should be.
            Err(err) => tracing::warn!(error = %err, "could not upgrade password hash"),
        }
    }

    user::record_successful_login(pool, account.id, facts.ip).await?;

    finish_sign_in(
        pool,
        security,
        account,
        &email,
        credentials.remember_me,
        facts,
        delivery,
    )
    .await
}

/// Decide what a correct password actually earns, under this workspace's policy.
async fn finish_sign_in(
    pool: &PgPool,
    security: &Security<'_>,
    account: UserRecord,
    email: &str,
    remember_me: bool,
    facts: ClientFacts<'_>,
    delivery: Delivery,
) -> ServiceResult<SignedIn> {
    let settings = settings_store::load(pool).await?;
    let outcome = Outcome::decide(pool, &settings, &account).await?;
    let result = outcome.into_login_result(pool, &account).await?;

    audit::record_best_effort(
        pool,
        AuditEntry::new(IdentityEvent::Login, true)
            .user(account.id)
            .email(email)
            .client(facts.ip, facts.user_agent)
            .detail(serde_json::json!({
                "outcome": outcome.label(),
                "delivery": match delivery {
                    Delivery::Cookie => "cookie",
                    Delivery::Handoff => "handoff",
                },
            })),
    )
    .await;

    match delivery {
        Delivery::Cookie => {
            let opened =
                open_session(pool, security, &account, remember_me, outcome, facts).await?;

            Ok(SignedIn {
                result,
                max_age_secs: opened.max_age_secs(),
                token: Some(opened.token),
                handoff: None,
            })
        }
        // No session is opened here. The one that matters is opened on the
        // workspace host by `redeem_handoff`, which is where the cookie can
        // actually be set - and which re-decides the outcome, so a password
        // change or an enforcement switch between the two requests is honoured.
        Delivery::Handoff => {
            let handoff = crate::identity::one_time_token::issue(
                pool,
                account.id,
                phonix_db::identity::one_time_token::TokenPurpose::SessionHandoff,
                security.config.session.handoff_ttl_secs as i64,
                facts.ip,
            )
            .await?;

            Ok(SignedIn {
                result,
                max_age_secs: 0,
                token: None,
                handoff: Some(handoff.secret),
            })
        }
    }
}

/// Trade a single-use handoff token for a session on this host.
///
/// Both paths that cross hosts end here: the end of signup, and a sign-in
/// submitted on the bare domain. The token is consumed exactly once - a second
/// request with the same one finds no row.
///
/// Returns `None` for a token that is unknown, expired, already spent or issued
/// for something else, and for an account that has since lost the right to sign
/// in. All of them mean the same thing to the caller: send them to the sign-in
/// form.
pub async fn redeem_handoff(
    pool: &PgPool,
    security: &Security<'_>,
    presented: &SecretString,
    facts: ClientFacts<'_>,
) -> ServiceResult<Option<SignedIn>> {
    use phonix_db::identity::one_time_token::TokenPurpose;

    let Some(user_id) =
        crate::identity::one_time_token::consume(pool, presented, TokenPurpose::SessionHandoff)
            .await?
    else {
        return Ok(None);
    };

    let Some(account) = user::find_by_id(pool, user_id).await? else {
        return Ok(None);
    };
    if !account.can_sign_in(Utc::now()) {
        return Ok(None);
    }

    let settings = settings_store::load(pool).await?;
    let outcome = Outcome::decide(pool, &settings, &account).await?;
    let result = outcome.into_login_result(pool, &account).await?;

    // Never "remember me": the flag belonged to a form on another host, and a
    // token in a URL is not the thing to extend a session ceiling on.
    let opened = open_session(pool, security, &account, false, outcome, facts.clone()).await?;

    audit::record_best_effort(
        pool,
        AuditEntry::new(IdentityEvent::Login, true)
            .user(account.id)
            .email(&account.email)
            .client(facts.ip, facts.user_agent)
            .detail(serde_json::json!({
                "outcome": outcome.label(),
                "delivery": "handoff_redeemed",
            })),
    )
    .await;

    Ok(Some(SignedIn {
        result,
        max_age_secs: opened.max_age_secs(),
        token: Some(opened.token),
        handoff: None,
    }))
}

/// Open the session an outcome calls for, and start the challenge clock if it
/// needs one.
async fn open_session(
    pool: &PgPool,
    security: &Security<'_>,
    account: &UserRecord,
    remember_me: bool,
    outcome: Outcome,
    facts: ClientFacts<'_>,
) -> ServiceResult<session_service::OpenedSession> {
    let opened = session_service::open(
        pool,
        account.id,
        &security.config.session,
        remember_me,
        outcome.mfa_satisfied(),
        facts,
    )
    .await?;

    if matches!(outcome, Outcome::ChallengeMfa) {
        session_store::start_mfa_challenge(
            pool,
            opened.record.id,
            security.config.mfa.challenge_ttl_mins as i64,
        )
        .await?;
    }

    Ok(opened)
}

/// What a correct password leads to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Straight in.
    SignedIn,
    /// Holds a confirmed factor, and the workspace still challenges factors.
    ChallengeMfa,
    /// The workspace requires a factor this user does not have, and the grace
    /// period is over.
    RequireEnrolment,
    /// The password has aged out, or was flagged for a forced change.
    RequirePasswordChange,
}

impl Outcome {
    /// Order matters. A forced password change comes first because a password
    /// that must not be used any more must not be used to reach anything, and
    /// enrolment before the challenge because there is nothing to challenge.
    async fn decide(
        pool: &PgPool,
        settings: &WorkspaceSecuritySettings,
        account: &UserRecord,
    ) -> ServiceResult<Self> {
        let now = Utc::now();

        let password_aged = account
            .password_updated_at
            .is_some_and(|changed| settings.password.is_expired(changed, now));

        if account.must_change_password || password_aged {
            return Ok(Self::RequirePasswordChange);
        }

        let has_factor = mfa_store::has_confirmed_factor(pool, account.id).await?;

        if has_factor {
            return Ok(if settings.mfa.challenges_existing_factors() {
                Self::ChallengeMfa
            } else {
                // Enforcement was switched off. The enrolment survives, it is
                // just not asked for - turning it back on must not have
                // destroyed everybody's factor in between.
                Self::SignedIn
            });
        }

        let account_age_days = (now - account.created_at).num_days();
        if settings.mfa.allows_sign_in_without_factor(account_age_days) {
            Ok(Self::SignedIn)
        } else {
            Ok(Self::RequireEnrolment)
        }
    }

    /// What the caller is told, which is not quite the same as what happened.
    ///
    /// `Success` carries the resolved user because the client renders from it;
    /// the other three carry only an id, because a session that may reach one
    /// screen has no business shipping a permission set to the browser.
    async fn into_login_result(
        self,
        pool: &PgPool,
        account: &UserRecord,
    ) -> ServiceResult<LoginResult> {
        Ok(match self {
            Self::ChallengeMfa => LoginResult::MfaRequired {
                user_id: account.id,
            },
            Self::RequireEnrolment => LoginResult::MfaEnrolmentRequired {
                user_id: account.id,
            },
            Self::RequirePasswordChange => LoginResult::PasswordChangeRequired {
                user_id: account.id,
            },
            Self::SignedIn => {
                LoginResult::Success(Box::new(load_auth_user(pool, account, true).await?))
            }
        })
    }

    /// Whether the session this produces has cleared its second factor.
    ///
    /// False for all three "yes, but" outcomes: a session that may only reach
    /// one screen must report nothing as permitted, and
    /// `AuthUser::is_fully_authenticated` is what enforces that.
    fn mfa_satisfied(self) -> bool {
        matches!(self, Self::SignedIn)
    }

    fn label(self) -> &'static str {
        match self {
            Self::SignedIn => "signed_in",
            Self::ChallengeMfa => "mfa_required",
            Self::RequireEnrolment => "mfa_enrolment_required",
            Self::RequirePasswordChange => "password_change_required",
        }
    }
}

/// Resolve the user behind a session token, sliding its idle deadline forward.
///
/// Runs on every request that carries a cookie, so it is deliberately three
/// queries and no more: touch the session, load the account, resolve its roles
/// and permissions.
///
/// Returns `None` for a token that is unknown, expired or revoked, and for an
/// account that has since been suspended or deleted - a live session must not
/// outlive the account's right to hold one.
pub async fn authenticate_session(
    pool: &PgPool,
    token: &SecretString,
    security: &phonix_config::SecurityConfig,
) -> ServiceResult<Option<AuthUser>> {
    let Some(session) = session_service::resume(pool, token, &security.session).await? else {
        return Ok(None);
    };

    let Some(account) = user::find_by_id(pool, session.user_id).await? else {
        // The account was hard-deleted under a live session. `ON DELETE
        // CASCADE` should have taken the session with it, so this is belt and
        // braces.
        return Ok(None);
    };

    if !account.can_sign_in(Utc::now()) {
        // Suspended or locked since the session was opened. Revoked rather than
        // merely refused, so the stale token stops costing a lookup.
        session_store::revoke_by_id(pool, session.id, "account no longer permitted to sign in")
            .await?;
        return Ok(None);
    }

    Ok(Some(
        load_auth_user(pool, &account, session.mfa_satisfied).await?,
    ))
}

/// End a session.
pub async fn sign_out(
    pool: &PgPool,
    token: &SecretString,
    user_id: Option<UserId>,
) -> ServiceResult<()> {
    session_service::close(pool, token, "signed out").await?;

    if let Some(user_id) = user_id {
        audit::record_best_effort(
            pool,
            AuditEntry::new(IdentityEvent::Logout, true).user(user_id),
        )
        .await;
    }

    Ok(())
}

/// Load the roles and permissions that go with an account.
pub async fn load_auth_user(
    pool: &PgPool,
    account: &UserRecord,
    mfa_satisfied: bool,
) -> ServiceResult<AuthUser> {
    let roles = authorization::role::names_for_user(pool, account.id).await?;
    let permissions = authorization::permission::resolve_for_user(pool, account.id).await?;

    Ok(account.to_auth_user(roles, permissions, mfa_satisfied))
}

/// [`load_auth_user`] starting from an id.
pub async fn load_auth_user_by_id(
    pool: &PgPool,
    user_id: UserId,
    mfa_satisfied: bool,
) -> ServiceResult<Option<AuthUser>> {
    let Some(account) = user::find_by_id(pool, user_id).await? else {
        return Ok(None);
    };
    Ok(Some(load_auth_user(pool, &account, mfa_satisfied).await?))
}

/// Record a failed attempt and return the uniform rejection.
async fn reject_and_audit(
    pool: &PgPool,
    account: &UserRecord,
    email: &str,
    facts: &ClientFacts<'_>,
    reason: &str,
) -> SignedIn {
    audit::record_best_effort(
        pool,
        AuditEntry::new(IdentityEvent::Login, false)
            .user(account.id)
            .email(email)
            .client(facts.ip, facts.user_agent)
            .reason(reason),
    )
    .await;

    SignedIn::rejected()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_refusal_looks_the_same_from_outside() {
        // The reasons a sign-in can be refused, as the caller sees them. If any
        // of these ever gains its own variant, the login form becomes an
        // account-enumeration oracle.
        let refusals = [
            "no account with that address",
            "wrong password",
            "no password set",
            "account is suspended",
            "account is pending",
        ];

        for _reason in refusals {
            let rejected = SignedIn::rejected();
            assert!(matches!(rejected.result, LoginResult::Rejected));
            assert!(rejected.token.is_none());
            assert_eq!(rejected.max_age_secs, 0);
        }

        // A lockout is the one exception, and deliberately so: the caller
        // triggered it, so the wait is not a secret.
        let locked = SignedIn::locked(900);
        assert!(
            locked
                .result
                .message()
                .unwrap()
                .to_string()
                .contains("15 minutes")
        );
        assert!(locked.token.is_none());
    }

    #[test]
    fn only_a_finished_sign_in_carries_permissions() {
        // The three "yes, but" outcomes hold a session, so the challenge and
        // enrolment screens have something to attach to - but none of them has
        // cleared the second factor, and `AuthUser::can` returns false until it
        // has.
        assert!(Outcome::SignedIn.mfa_satisfied());
        assert!(!Outcome::ChallengeMfa.mfa_satisfied());
        assert!(!Outcome::RequireEnrolment.mfa_satisfied());
        assert!(!Outcome::RequirePasswordChange.mfa_satisfied());
    }

    #[test]
    fn every_outcome_has_a_distinct_audit_label() {
        let labels = [
            Outcome::SignedIn.label(),
            Outcome::ChallengeMfa.label(),
            Outcome::RequireEnrolment.label(),
            Outcome::RequirePasswordChange.label(),
        ];
        let unique: std::collections::HashSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len(), "{labels:?}");
    }
}
