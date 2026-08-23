//! Single-use secrets: email verification, password reset, invitations, and the
//! signup handoff.
//!
//! Same division as [`super::session`]: the secret is minted here, the digest
//! is what reaches the database. A dump of `user_tokens` therefore contains
//! nothing anyone can put in a link.

use chrono::{DateTime, Utc};
use phonix_core::identity::UserId;
use phonix_db::identity::one_time_token::{self, TokenPurpose};
use phonix_db::sqlx::PgExecutor;
use secrecy::SecretString;

use crate::crypto::token::{IssuedToken, digest_of_secret, looks_like_a_token};
use crate::error::ServiceResult;

/// A token that has been issued but not yet handed out.
pub struct IssuedOneTimeToken {
    pub id: uuid::Uuid,
    pub user_id: UserId,
    pub expires_at: DateTime<Utc>,
    /// Put this in the link or the redirect. Not recoverable afterwards.
    pub secret: SecretString,
}

/// Issue a token, superseding any outstanding one for the same purpose.
///
/// Superseding matters: if requesting a second password reset left the first
/// live, an email intercepted an hour ago would still work.
pub async fn issue<'e, E>(
    executor: E,
    user_id: UserId,
    purpose: TokenPurpose,
    ttl_secs: i64,
    created_ip: Option<&str>,
) -> ServiceResult<IssuedOneTimeToken>
where
    E: PgExecutor<'e> + Copy,
{
    let issued = IssuedToken::generate();

    let record = one_time_token::issue(
        executor,
        user_id,
        purpose,
        &issued.digest,
        ttl_secs,
        created_ip,
    )
    .await?;

    Ok(IssuedOneTimeToken {
        id: record.id,
        user_id: record.user_id,
        expires_at: record.expires_at,
        secret: issued.secret,
    })
}

/// Redeem a token, exactly once.
///
/// Returns the user it belonged to, or `None` when it is unknown, expired,
/// already spent, or was issued for a different purpose - four cases that are
/// deliberately indistinguishable, because "already used" tells whoever
/// intercepted the link that they had a real one.
pub async fn consume<'e, E>(
    executor: E,
    presented: &SecretString,
    purpose: TokenPurpose,
) -> ServiceResult<Option<UserId>>
where
    E: PgExecutor<'e>,
{
    use secrecy::ExposeSecret;

    if !looks_like_a_token(presented.expose_secret()) {
        return Ok(None);
    }

    Ok(one_time_token::consume(executor, &digest_of_secret(presented), purpose).await?)
}
