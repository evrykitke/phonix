//! API keys: issuing one, listing them, stopping one, and turning a presented
//! token back into a [`Caller`].
//!
//! See `docs/adr/0002-public-api.md` for the decisions this implements. Three
//! of them are load-bearing here and worth repeating where the code is:
//!
//! **The database never sees a token.** [`issue`] mints 32 bytes, prefixes
//! them, hands the whole thing back exactly once, and stores the SHA-256
//! digest. There is no way to recover a key afterwards, by design and by
//! construction - nothing in the row can be turned back into the secret.
//!
//! **A key cannot exceed the person who issued it, ever.** Not at issue time -
//! [`issue`] refuses a scope the issuer does not hold - and not afterwards:
//! [`authenticate`] intersects the key's scopes with the owner's *current*
//! permissions, read fresh on every request. Removing a grant from a user, or
//! suspending them, therefore removes it from every key they issued, with
//! nothing here to keep in step.
//!
//! **Scopes are permission names.** A scope of `Pages.Administration.Settings`
//! covers everything beneath it exactly as a grant does, because it is checked
//! by the same tree. An API-specific scope vocabulary would be a second list
//! saying the same thing, and the two would disagree within a release.

use chrono::{DateTime, Duration, Utc};
use phonix_core::authorization::{is_defined, is_descendant_of};
use phonix_core::form::{Submission, rejected};
use phonix_core::identity::{ApiKeyDraft, ApiKeyIssued, ApiKeySummary, AuthUser, UserId};
use phonix_core::query::{Page, PageRequest};
use phonix_core::{PermissionSet, msg, permissions};
use phonix_db::identity::api_key as store;
use phonix_db::identity::{ApiKeyListing, ApiKeyRecord, NewApiKey};
use phonix_db::settings;
use phonix_db::sqlx::PgPool;
use phonix_db::{identity::user, sqlx::PgExecutor};
use secrecy::{ExposeSecret, SecretString};
use uuid::Uuid;

use crate::audit::{self, Target, kinds};
use crate::caller::{Caller, acting_user};
use crate::crypto::token::{self, IssuedToken};
use crate::error::{ServiceError, ServiceResult};

/// What every API key starts with.
///
/// Not decoration. It is what lets a secret scanner recognise one of ours in a
/// public repository, and what tells somebody reading a deployment's
/// configuration what they are looking at. It is stripped before the digest is
/// computed, so what is stored has the same shape as a session's.
pub const TOKEN_PREFIX: &str = "phx_";

/// What was minted, with the secret still in hand.
///
/// Never returned as it stands: [`issue`] takes the token out of it and the
/// value is dropped. Keeping the pair in one short-lived struct is what stops
/// a code path storing a record and forgetting to hand back the key.
struct Minted {
    record: ApiKeyRecord,
    secret: SecretString,
}

/// Issue a key for the calling user.
///
/// The key acts as the caller, which is why there is no "issue a key for
/// somebody else" parameter: that would be a way to obtain a credential for an
/// account whose permissions one does not have, and no screen needs it.
///
/// Answers a [`Submission`], so "that is not a permission you hold" arrives at
/// the scopes field rather than as a sentence at the top of the form.
pub async fn issue(
    pool: &PgPool,
    caller: &Caller,
    draft: ApiKeyDraft,
) -> ServiceResult<Submission<ApiKeyIssued>> {
    caller.require(permissions::API_KEYS_CREATE)?;
    let user_id = acting_user(caller)?;

    if let Some(rejection) = rejected(draft.validate()) {
        return Ok(rejection);
    }

    let scopes = match validated_scopes(caller, &draft) {
        Ok(scopes) => scopes,
        Err(rejection) => return Ok(rejection),
    };

    let expires_at = draft
        .expires_in_days
        .map(|days| Utc::now() + Duration::days(days));

    let minted = mint(pool, user_id, draft.name.trim(), &scopes, expires_at).await?;

    // Recorded as a creation, with the scopes in the payload: "who could reach
    // this workspace from outside, and with what" is the question this trail
    // exists to answer. The token is not in it and cannot be - nothing here
    // holds it but the value being returned.
    audit::created(
        pool,
        caller,
        Target::new(kinds::API_KEY, minted.record.id).named(&minted.record.name),
        &audit_shape(&minted.record),
    )
    .await;

    tracing::info!(
        key_id = %minted.record.id,
        scopes = minted.record.scopes.len(),
        "api key issued"
    );

    let owner_name = caller
        .auth_user()
        .map_or_else(String::new, |user| user.display_name.clone());

    Ok(Submission::Saved(ApiKeyIssued {
        key: summary(&minted.record, owner_name),
        secret: minted.secret.expose_secret().to_owned(),
    }))
}

/// The scopes as they will be stored, or the rejection to answer with.
///
/// Two separate refusals, because they are two different mistakes. A name that
/// is not in the tree is a typo or a screen from an older build; a name the
/// issuer does not hold is an attempt - deliberate or not - to mint a
/// credential more powerful than the person minting it.
#[allow(
    clippy::result_large_err,
    reason = "the Err is a Submission, which is what the caller returns unchanged -               boxing it here would only move the allocation to the one call site, on a               path that issues a credential once and is nowhere near hot"
)]
fn validated_scopes(
    caller: &Caller,
    draft: &ApiKeyDraft,
) -> Result<Vec<String>, Submission<ApiKeyIssued>> {
    let mut scopes = Vec::with_capacity(draft.scopes.len());

    for scope in &draft.scopes {
        let scope = scope.trim();
        if scope.is_empty() {
            continue;
        }

        if !is_defined(scope) {
            return Err(Submission::rejected(
                "scopes",
                msg!("error.api_key.unknown_scope", scope = scope),
            ));
        }

        if !caller.can(scope) {
            return Err(Submission::rejected(
                "scopes",
                msg!("error.api_key.scope_not_held", scope = scope),
            ));
        }

        scopes.push(scope.to_owned());
    }

    scopes.sort();
    scopes.dedup();
    Ok(scopes)
}

/// Mint the token and store its digest.
///
/// Split out so that the one place in the system where a usable API credential
/// exists is a short function with nothing else in it.
async fn mint(
    pool: &PgPool,
    user_id: UserId,
    name: &str,
    scopes: &[String],
    expires_at: Option<DateTime<Utc>>,
) -> ServiceResult<Minted> {
    let issued = IssuedToken::generate();
    let secret = format!("{TOKEN_PREFIX}{}", issued.secret.expose_secret());

    // The last four characters of the token, which is the part somebody
    // compares against what is in their configuration file. Counted in
    // characters rather than bytes even though base64 is ASCII: a slice by
    // byte index is a habit that panics the first time it meets something
    // else.
    let hint: String = secret
        .chars()
        .skip(secret.chars().count().saturating_sub(4))
        .collect();

    let record = store::create(
        pool,
        NewApiKey {
            user_id,
            name,
            token_hash: &issued.digest,
            token_hint: &hint,
            scopes,
            expires_at,
            created_by: Some(user_id),
        },
    )
    .await?;

    Ok(Minted {
        record,
        secret: SecretString::from(secret),
    })
}

/// One page of this workspace's keys.
///
/// Paged in SQL rather than in the browser. A workspace has few keys today and
/// an integrator's will not: every phone build, every customer script and every
/// retired credential is a row, and rows are kept for their history.
pub async fn list(
    pool: &PgPool,
    caller: &Caller,
    request: &PageRequest,
) -> ServiceResult<Page<ApiKeySummary>> {
    caller.require(permissions::API_KEYS)?;

    let page = store::page(pool, request).await?;

    Ok(Page {
        rows: page.rows.iter().map(listing_to_summary).collect(),
        total: page.total,
        page: page.page,
        per_page: page.per_page,
    })
}

/// Stop a key.
///
/// Immediate: the next request presenting it fails the lookup, because liveness
/// is decided in the same statement.
pub async fn revoke(pool: &PgPool, caller: &Caller, id: Uuid, reason: &str) -> ServiceResult<()> {
    caller.require(permissions::API_KEYS_REVOKE)?;
    let actor = acting_user(caller)?;

    let Some(record) = store::find_by_id(pool, id).await? else {
        return Err(ServiceError::rejected(
            "id",
            msg!("error.api_key.gone"),
        ));
    };

    if !store::revoke(pool, id, reason, Some(actor)).await? {
        // Already revoked. The key is stopped either way, which is what the
        // caller wanted - but saying so is better than reporting success on a
        // button that did nothing.
        return Err(ServiceError::rejected("id", msg!("error.api_key.gone")));
    }

    audit::deleted(
        pool,
        caller,
        Target::new(kinds::API_KEY, record.id).named(&record.name),
        &audit_shape(&record),
    )
    .await;

    tracing::info!(key_id = %id, "api key revoked");

    Ok(())
}

/// Stop every key an account holds, for the moment it is suspended or deleted.
///
/// The intersection in [`authenticate`] already makes such keys powerless. This
/// is the difference between powerless and gone, and it is worth the extra
/// statement: a credential that still authenticates is one somebody has to
/// reason about later.
pub async fn revoke_all_for_user<'e, E>(
    executor: E,
    user_id: UserId,
    reason: &str,
) -> ServiceResult<u64>
where
    E: PgExecutor<'e>,
{
    store::revoke_all_for_user(executor, user_id, reason)
        .await
        .map_err(ServiceError::from)
}

/// Who is behind a presented API token, and what they may do with it.
pub struct AuthenticatedKey {
    /// The caller to hand to a use case: the owner, narrowed to the scopes.
    pub caller: Caller,
    /// For the log line, the rate limiter's key, and `last_used_at`.
    pub key_id: Uuid,
}

/// Turn `Authorization: Bearer ...` into a caller, or into nothing.
///
/// `None` covers every way a token can fail - malformed, unknown, revoked,
/// expired, owned by an account that may no longer sign in - deliberately
/// without distinguishing them. The API answers all of them `401`: telling
/// somebody probing for tokens that theirs is *revoked* rather than unknown
/// confirms they had a real one.
pub async fn authenticate(
    pool: &PgPool,
    presented: &SecretString,
) -> ServiceResult<Option<AuthenticatedKey>> {
    let Some(raw) = presented.expose_secret().strip_prefix(TOKEN_PREFIX) else {
        return Ok(None);
    };

    // Shape first, exactly as sessions do it: an obviously wrong token - a
    // truncated paste, a scanner's probe - is refused without an indexed
    // lookup, and unbounded input never reaches a query parameter.
    if !token::looks_like_a_token(raw) {
        return Ok(None);
    }

    let Some(key) = store::find_live_by_hash(pool, &token::digest_of(raw)).await? else {
        return Ok(None);
    };

    let Some(account) = user::find_by_id(pool, key.user_id).await? else {
        // The account was hard-deleted under a live key. `ON DELETE CASCADE`
        // should have taken the key with it; belt and braces.
        return Ok(None);
    };

    if !account.can_sign_in(Utc::now()) {
        // Suspended or locked since the key was issued. Revoked rather than
        // merely refused, so the dead credential stops costing a lookup.
        store::revoke(pool, key.id, "owner may no longer sign in", None).await?;
        return Ok(None);
    }

    // `mfa_satisfied = true`, and this is a decision rather than a shortcut.
    // A second factor answers "is the person at this browser who they claim";
    // a key is not a browser and has nobody at it. It was issued *from* a
    // fully authenticated session, it is a credential in its own right, and
    // treating it as half-authenticated would make every key powerless while
    // still appearing to work.
    let owner = super::authentication::load_auth_user(pool, &account, true).await?;

    // Best-effort, coarse, and never allowed to fail the request: this column
    // exists to answer "is anything still using this key", which nobody asks
    // to the minute.
    if let Err(err) = store::touch_last_used(pool, key.id).await {
        tracing::debug!(error = %err, key_id = %key.id, "could not record api key use");
    }

    Ok(Some(AuthenticatedKey {
        caller: Caller::user(narrowed(owner, &key.scopes)),
        key_id: key.id,
    }))
}

/// Whether this workspace has the API at all.
///
/// A licence rather than a grant, checked before the key is even looked up -
/// see the ADR. Deliberately not a permission: an administrator can grant
/// themselves a permission, and cannot sell themselves a feature.
///
/// Ungated to read, because the screen that manages keys has to be able to tell
/// the person looking at it that the API is switched off.
pub async fn api_enabled<'e, E>(executor: E) -> ServiceResult<bool>
where
    E: PgExecutor<'e>,
{
    settings::api_enabled(executor)
        .await
        .map_err(ServiceError::from)
}

/// Turn the API on or off for this workspace.
///
/// `Settings`, and not one of the API-key permissions: this is what the
/// workspace *has*, and it belongs with the other decisions about the workspace
/// itself rather than with the credentials issued inside it.
pub async fn set_api_enabled(pool: &PgPool, caller: &Caller, enabled: bool) -> ServiceResult<()> {
    caller.require(permissions::SETTINGS)?;
    let actor = acting_user(caller)?;

    let before = api_enabled(pool).await?;
    if before == enabled {
        // Nothing changed, so nothing is recorded. A history saying "switched
        // off" three times because a screen saved on every render is a history
        // nobody reads.
        return Ok(());
    }

    settings::set_api_enabled(pool, enabled, Some(actor)).await?;

    audit::changed_json(
        pool,
        caller,
        Target::singleton(kinds::SECURITY_POLICY).fact("setting", "api_enabled"),
        serde_json::json!({ "api_enabled": before }),
        serde_json::json!({ "api_enabled": enabled }),
    )
    .await;

    Ok(())
}

/// The owner, holding only what the key's scopes cover.
///
/// `insert_exact` rather than `grant`, because granting a permission implies
/// its ancestors - which is right when somebody is being *given* something and
/// wrong here: this set is a narrowing of one that has already been resolved,
/// and adding a parent back would hand the key a permission the scope did not
/// mention.
fn narrowed(mut owner: AuthUser, scopes: &[String]) -> AuthUser {
    let mut allowed = PermissionSet::new();

    for held in owner.permissions.iter() {
        let covered = scopes
            .iter()
            .any(|scope| scope == held || is_descendant_of(held, scope));

        if covered {
            allowed.insert_exact(held);
        }
    }

    owner.permissions = allowed;
    owner
}

/// A stored row as a screen reads it.
///
/// Hand-written rather than derived, for the same reason the API's wire types
/// are: a column added to `api_keys` does not silently start appearing on a
/// screen, and nothing that could identify the token can reach one. The digest
/// is not here either - it is not secret, but it is not anything a person can
/// use.
fn summary(record: &ApiKeyRecord, owner_name: String) -> ApiKeySummary {
    ApiKeySummary {
        id: record.id,
        name: record.name.clone(),
        hint: record.token_hint.clone(),
        scopes: record.scopes.clone(),
        owner_name,
        created_at: record.created_at,
        expires_at: record.expires_at,
        last_used_at: record.last_used_at,
        revoked_at: record.revoked_at,
    }
}

fn listing_to_summary(listing: &ApiKeyListing) -> ApiKeySummary {
    summary(&listing.key, listing.owner_name.clone())
}

/// What the audit trail records about a key.
///
/// A named shape rather than the row, so that a column added to `api_keys` does
/// not silently start appearing in the trail - and so that nothing which could
/// identify the token ever can.
fn audit_shape(record: &ApiKeyRecord) -> serde_json::Value {
    serde_json::json!({
        "name": record.name,
        "scopes": record.scopes,
        "expires_at": record.expires_at,
        "hint": record.token_hint,
    })
}

#[cfg(test)]
mod tests {
    use phonix_core::PermissionSet;
    use phonix_core::identity::UserStatus;
    use phonix_core::permissions as names;

    use super::*;

    fn owner_with(granted: &[&str]) -> AuthUser {
        let mut permissions = PermissionSet::new();
        for name in granted {
            permissions.grant(name);
        }

        AuthUser {
            id: uuid::Uuid::nil(),
            email: "ada@example.com".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            display_name: "Ada Lovelace".into(),
            roles: vec!["Admin".into()],
            permissions,
            is_owner: false,
            status: UserStatus::Active,
            mfa_enabled: false,
            mfa_satisfied: true,
            email_verified: true,
        }
    }

    fn draft_with(scopes: &[&str]) -> ApiKeyDraft {
        ApiKeyDraft {
            name: "nightly export".to_owned(),
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
            expires_in_days: None,
        }
    }

    #[test]
    fn a_scope_carries_what_hangs_beneath_it() {
        let owner = owner_with(&[names::USERS, names::USERS_CREATE, names::SETTINGS]);
        let key = narrowed(owner, &[names::USERS.to_owned()]);

        assert!(key.can(names::USERS));
        assert!(key.can(names::USERS_CREATE));
        // Scoped to users, and therefore not to settings - even though the
        // person who issued it holds settings.
        assert!(!key.can(names::SETTINGS));
    }

    #[test]
    fn a_key_cannot_hold_what_its_owner_does_not() {
        // The scope is wider than the grant. The intersection is the grant.
        let owner = owner_with(&[names::USERS]);
        let key = narrowed(owner, &[names::PAGES.to_owned()]);

        assert!(key.can(names::USERS));
        assert!(!key.can(names::SETTINGS));
    }

    #[test]
    fn a_scope_does_not_reach_upwards() {
        // Scoped to one leaf. The parent is a wider power than was granted,
        // and `insert_exact` is what keeps it out.
        let owner = owner_with(&[names::USERS, names::USERS_CREATE]);
        let key = narrowed(owner, &[names::USERS_CREATE.to_owned()]);

        assert!(key.can(names::USERS_CREATE));
        assert!(!key.can(names::USERS));
    }

    #[test]
    fn a_key_with_no_scopes_holds_nothing() {
        // Not a mistake to guard against - it is the useful shape for a key
        // that only reads what is ungated.
        let owner = owner_with(&[names::USERS, names::SETTINGS]);
        let key = narrowed(owner, &[]);

        assert!(key.permissions.is_empty());
        assert!(!key.can(names::USERS));
    }

    #[test]
    fn a_scope_the_issuer_does_not_hold_is_refused_at_the_field() {
        let caller = Caller::user(owner_with(&[names::API_KEYS, names::API_KEYS_CREATE]));

        let rejection = validated_scopes(&caller, &draft_with(&[names::SETTINGS]))
            .expect_err("issuing Settings is not something this caller may do");

        assert_eq!(
            rejection.errors().first().map(|err| err.field.as_str()),
            Some("scopes"),
            "the refusal has to reach the control that caused it"
        );
    }

    #[test]
    fn a_scope_that_is_not_in_the_tree_is_refused_separately() {
        // A typo and an escalation attempt are different mistakes, and the
        // person reading the message needs to know which one they made.
        let caller = Caller::user(owner_with(&[names::API_KEYS_CREATE]));

        let rejection = validated_scopes(&caller, &draft_with(&["Pages.Adminstration.Settings"]))
            .expect_err("a misspelled permission is not a permission");

        assert_eq!(
            rejection.errors().first().map(|err| err.message.key.as_str()),
            Some("error.api_key.unknown_scope")
        );
    }

    #[test]
    fn scopes_are_stored_sorted_and_without_repeats() {
        let caller = Caller::user(owner_with(&[
            names::SETTINGS,
            names::USERS,
            names::API_KEYS_CREATE,
        ]));

        let scopes = validated_scopes(
            &caller,
            &draft_with(&[names::USERS, names::SETTINGS, names::USERS, " "]),
        )
        .expect("every scope is held");

        assert_eq!(
            scopes,
            vec![names::SETTINGS.to_owned(), names::USERS.to_owned()],
            "a blank entry is not a scope, and the same one twice is one scope"
        );
    }
}
