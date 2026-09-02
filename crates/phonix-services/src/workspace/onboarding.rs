//! Creating a workspace and its first user.
//!
//! The one use case that spans the catalog and a tenant database, which is why
//! it sits in the application layer rather than inside either repository.
//!
//! # Why this cannot be one transaction
//!
//! `CREATE DATABASE` is not transactional in Postgres, and the catalog and the
//! tenant database are two different connections regardless. So the steps are
//! ordered by what a partial failure leaves behind:
//!
//! ```text
//! 1. catalog row, status 'provisioning'   <- the serialisation point
//! 2. CREATE DATABASE                      <- skipped if present
//! 3. tenant migrations                    <- idempotent
//! 3b. trial licence                       <- only if it has none
//! 4. static role permissions + settings   <- idempotent
//! 5. owner account + Admin role           <- inside ONE tenant transaction
//! 6. catalog row -> 'active'              <- the commit point
//! ```
//!
//! Step 3b is inside `provision_tenant` and sits before the row goes active
//! deliberately: `serves_traffic` is "active **and** currently licensed", so
//! issuing the licence afterwards would leave an interval in which a finished
//! workspace answered 403.
//!
//! A crash before step 6 leaves a workspace stuck in `provisioning`, which
//! serves no traffic and can be retried. A crash after it leaves a working
//! workspace. What cannot happen is a workspace that serves traffic with no
//! owner in it - that is what step 5 being one transaction, before step 6, buys.

use chrono::{Duration, Utc};
use phonix_config::AppConfig;
use phonix_core::authorization::roles;
use phonix_core::identity::{UserStatus, ValidSignup};
use phonix_core::{LicenceState, TenantSlug};
use phonix_db::authorization::role;
use phonix_db::identity::one_time_token::TokenPurpose;
use phonix_db::identity::user::{self, NewUser};
use phonix_db::identity::{AuditEntry, IdentityEvent, audit};
use phonix_db::settings as settings_store;
use phonix_db::sqlx::PgPool;
use phonix_db::tenancy::catalog::{Catalog, TenantOrigin, TenantRecord};
use phonix_db::tenancy::{LicenceInput, provision};
use secrecy::SecretString;

use crate::crypto::password::Hasher;
use crate::error::{ServiceError, ServiceResult};
use crate::identity::one_time_token;

/// A workspace that has just been created.
pub struct OnboardedWorkspace {
    pub tenant: TenantRecord,
    pub owner_id: phonix_core::identity::UserId,
    /// Single-use, short-lived. The browser is redirected to the workspace's
    /// own host carrying this, and trades it there for a session cookie.
    ///
    /// It exists because session cookies are host-only: the signup form runs on
    /// the bare domain and cannot set a cookie for a subdomain it does not
    /// control. See `identity::cookie`.
    pub handoff_token: SecretString,
}

/// Create a workspace, its database, and its owner account.
///
/// `input` must have been through `SignupInput::validate`, which is why it is
/// a [`ValidSignup`] rather than raw strings - there is no way to call this
/// with unvalidated data by mistake.
pub async fn onboard_workspace(
    catalog: &Catalog,
    config: &AppConfig,
    hasher: &Hasher,
    input: &ValidSignup,
    client_ip: Option<&str>,
) -> ServiceResult<OnboardedWorkspace> {
    let slug = &input.workspace_slug;

    // Cheap pre-check so the common "that address is taken" case does not pay
    // for a password hash. The unique index is still the authority - two
    // simultaneous signups for the same slug both pass this.
    if !catalog.slug_is_available(slug).await? {
        return Err(phonix_db::DbError::TenantExists(slug.to_string()).into());
    }

    // Hashed before the database exists, deliberately: ~50 ms of CPU is much
    // cheaper to throw away than a created-and-abandoned Postgres database.
    let password_hash = hasher
        .hash(&SecretString::from(input.password.clone()))
        .await
        .map_err(|err| ServiceError::Crypto(err.to_string()))?;

    // Steps 1-3b. The trial is a licence with an end date and not a status of
    // its own, so signing up exercises the expiry path from the first day
    // rather than for the first time on a real customer. Its length is the one
    // number that says what a trial is: `[desk] trial_days`.
    let trial_note = format!(
        "Trial issued by self-service signup, {} days.",
        config.desk.trial_days
    );
    let tenant = provision::provision_tenant(
        catalog,
        &config.database,
        slug,
        &input.organization_name,
        TenantOrigin::Signup,
        Some(&input.email),
        LicenceInput {
            state: LicenceState::Trial,
            valid_from: Utc::now(),
            valid_until: Some(Utc::now() + Duration::days(config.desk.trial_days as i64)),
            note: Some(&trial_note),
            // Nobody authorized this one; the form did. Said plainly rather
            // than attributed to the person signing up, who is not in a
            // position to license anything.
            updated_by: "signup",
        },
    )
    .await?;

    let pool = phonix_db::tenant_pool(&config.database, &tenant.database_name);

    // Everything from here can fail and be retried; the workspace stays in
    // 'provisioning' until the very end.
    let result = finish_onboarding(
        &pool,
        catalog,
        config,
        input,
        &password_hash,
        client_ip,
        slug,
    )
    .await;

    // The pool was opened for this one job. The registry opens its own on the
    // first real request.
    pool.close().await;

    let (owner_id, handoff_token) = result?;

    // Step 6. Re-read so the returned record carries the final status.
    let tenant = catalog
        .find_by_slug(slug)
        .await?
        .ok_or_else(|| phonix_db::DbError::UnknownTenant(slug.to_string()))?;

    tracing::info!(
        tenant = %slug,
        database = %tenant.database_name,
        "workspace onboarded"
    );

    Ok(OnboardedWorkspace {
        tenant,
        owner_id,
        handoff_token,
    })
}

/// Steps 4-6: policy, roles, the owner account, and activation.
async fn finish_onboarding(
    pool: &PgPool,
    catalog: &Catalog,
    config: &AppConfig,
    input: &ValidSignup,
    password_hash: &str,
    client_ip: Option<&str>,
    slug: &TenantSlug,
) -> ServiceResult<(phonix_core::identity::UserId, SecretString)> {
    let security = &config.security;

    // Step 4. Writes Admin's grants from the compiled permission tree, so a
    // workspace created today has whatever permissions this build defines.
    role::sync_static_roles(pool).await?;

    // The one moment the configuration file is allowed to decide an
    // organization's policy. From here the workspace's own row is the
    // authority, and changing the file does not reach back into it.
    settings_store::seed(pool, &config.security.workspace_defaults.as_settings()).await?;

    // Step 5, as one transaction. A user without their Admin role would be
    // locked out of their own new workspace.
    let mut tx = pool.begin().await.map_err(phonix_db::DbError::Query)?;

    let owner = user::create(
        &mut *tx,
        NewUser {
            email: &input.email,
            first_name: &input.first_name,
            last_name: &input.last_name,
            password_hash: Some(password_hash),
            // Active immediately. Email verification needs SMTP, and gating the
            // account on it today would mean nobody could ever get in.
            status: UserStatus::Active,
            is_owner: true,
            invited_by: None,
        },
    )
    .await?;

    role::assign_to_user_by_name(&mut tx, owner.id, roles::ADMIN).await?;

    tx.commit().await.map_err(phonix_db::DbError::Query)?;

    // Not inside the transaction above: the token's whole purpose is to be
    // redeemed by the very next request, so it must be visible immediately.
    let handoff = one_time_token::issue(
        pool,
        owner.id,
        TokenPurpose::SessionHandoff,
        security.session.handoff_ttl_secs as i64,
        client_ip,
    )
    .await?;

    audit::record_best_effort(
        pool,
        AuditEntry::new(IdentityEvent::Signup, true)
            .user(owner.id)
            .email(&input.email)
            .client(client_ip, None)
            .detail(serde_json::json!({
                "workspace": slug.as_str(),
                "organization": input.organization_name,
            })),
    )
    .await;

    // Step 6.
    catalog.mark_onboarded(slug).await?;

    Ok((owner.id, handoff.secret))
}

/// Whether a workspace address can be taken.
///
/// Answers the availability check on the signup form. Reports only free or
/// taken, never *why* a slug is taken - that would let an anonymous caller
/// enumerate suspended and archived workspaces.
pub async fn slug_is_available(
    catalog: &Catalog,
    config: &AppConfig,
    slug: &TenantSlug,
) -> ServiceResult<bool> {
    // Reserved names are refused before the catalog is consulted. `www`,
    // `admin` and `api` never route to a tenant, so a workspace registered
    // under one would be unreachable for ever.
    if config.tenancy.is_reserved(slug.as_str()) {
        return Ok(false);
    }

    // A name that would overflow Postgres' 63-byte identifier limit cannot be
    // provisioned, so it is not available however free it looks.
    if slug
        .database_name(&config.database.tenant_database_prefix)
        .len()
        > 63
    {
        return Ok(false);
    }

    Ok(catalog.slug_is_available(slug).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_name_that_cannot_become_a_database_is_not_available() {
        // 40 characters is the slug ceiling, which with the default prefix is
        // well inside the 63-byte identifier limit - but a deployment that
        // configured a long prefix could push it over.
        let slug = TenantSlug::parse("a".repeat(40)).unwrap();

        assert!(slug.database_name("phonix_tenant_").len() <= 63);
        assert!(slug.database_name(&"very_long_prefix_".repeat(2)).len() > 63);
    }
}
