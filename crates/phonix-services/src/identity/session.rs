//! Opening and closing sessions.
//!
//! The one job this layer has that the repository cannot: **minting the
//! token**. The secret exists here and nowhere else - `phonix_db::identity::
//! session` is handed its SHA-256 digest and never sees the value a browser
//! would present.

use phonix_config::SessionConfig;
use phonix_core::identity::UserId;
use phonix_db::identity::session::{self, ClientFacts, SessionRecord};
use phonix_db::sqlx::PgExecutor;
use secrecy::SecretString;

use crate::crypto::token::{IssuedToken, digest_of_secret, looks_like_a_token};
use crate::error::ServiceResult;

/// A session that has just been opened.
pub struct OpenedSession {
    pub record: SessionRecord,
    /// Hand to the client once, in a cookie. Not recoverable afterwards.
    pub token: SecretString,
}

impl OpenedSession {
    /// Seconds the cookie should live, matching the immovable deadline.
    pub fn max_age_secs(&self) -> i64 {
        (self.record.absolute_expires_at - chrono::Utc::now())
            .num_seconds()
            .max(0)
    }
}

/// Open a session for a user who has just proved something.
///
/// `mfa_satisfied` is false when a second factor is still outstanding: the
/// session exists so the challenge page has something to attach to, and the
/// resolved `AuthUser` reports nothing as permitted until it flips.
pub async fn open<'e, E>(
    executor: E,
    user_id: UserId,
    cfg: &SessionConfig,
    remember_me: bool,
    mfa_satisfied: bool,
    facts: ClientFacts<'_>,
) -> ServiceResult<OpenedSession>
where
    E: PgExecutor<'e>,
{
    let issued = IssuedToken::generate();

    let record = session::create(
        executor,
        user_id,
        &issued.digest,
        cfg,
        remember_me,
        mfa_satisfied,
        facts,
    )
    .await?;

    Ok(OpenedSession {
        record,
        token: issued.secret,
    })
}

/// Resolve a presented cookie value to a live session, sliding its deadline.
///
/// Returns `None` without touching the database for a value that cannot be a
/// token at all - an expired bookmark, a truncated paste, a scanner probing for
/// one. That is a saved indexed lookup on every piece of junk, and it keeps
/// unbounded input out of a query parameter.
pub async fn resume<'e, E>(
    executor: E,
    presented: &SecretString,
    cfg: &SessionConfig,
) -> ServiceResult<Option<SessionRecord>>
where
    E: PgExecutor<'e>,
{
    use secrecy::ExposeSecret;

    if !looks_like_a_token(presented.expose_secret()) {
        return Ok(None);
    }

    Ok(session::touch(executor, &digest_of_secret(presented), cfg).await?)
}

/// Close one session.
pub async fn close<'e, E>(executor: E, presented: &SecretString, reason: &str) -> ServiceResult<()>
where
    E: PgExecutor<'e>,
{
    session::revoke(executor, &digest_of_secret(presented), reason).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    #[test]
    fn junk_cookie_values_are_rejected_before_a_query() {
        // `resume` needs an executor, so the guard it depends on is tested
        // directly - that guard is the whole reason it is safe to pass an
        // arbitrary header value in.
        for junk in ["", "deleted", "' OR 1=1 --", &"a".repeat(44)] {
            assert!(!looks_like_a_token(junk), "{junk:?} reached the database");
        }
        assert!(looks_like_a_token(
            IssuedToken::generate().secret.expose_secret()
        ));
    }
}
