//! Changing what a workspace requires of its people.
//!
//! Two things happen here that a repository must not do on its own: the policy
//! is validated field by field before it is written, and the change is recorded
//! on the change trail. "Who relaxed the password rules, and when" is exactly
//! the question an audit asks after the fact, and a bare `UPDATE` cannot answer
//! it.
//!
//! The system defaults in `[security.workspace_defaults]` seed a workspace at
//! creation and are never consulted again. After that this row is the only
//! authority, which is what makes the policy the organization's rather than the
//! operator's.

use phonix_core::WorkspaceSecuritySettings;
use phonix_core::permissions;
use phonix_db::settings as store;
use phonix_db::sqlx::PgPool;

use crate::audit::{self, Target, kinds};
use crate::caller::{Caller, acting_user};
use crate::error::{ServiceError, ServiceResult};

/// What this workspace currently requires.
///
/// Deliberately ungated. Every user needs the password policy to fill in the
/// change-password form, and the MFA policy to be told why they are being sent
/// to enrolment - neither is a secret from the people it applies to.
pub async fn load(pool: &PgPool) -> ServiceResult<WorkspaceSecuritySettings> {
    Ok(store::load(pool).await?)
}

/// Save a policy an administrator submitted.
///
/// Validated here rather than in the repository because the settings form needs
/// to know *which* field is wrong: a CHECK constraint can only refuse the whole
/// row, and it arrives as a constraint name nobody outside this codebase can
/// read.
pub async fn save(
    pool: &PgPool,
    caller: &Caller,
    settings: &WorkspaceSecuritySettings,
) -> ServiceResult<()> {
    caller.require(permissions::SETTINGS)?;
    let changed_by = acting_user(caller)?;

    if let Err(errors) = settings.validate() {
        return Err(ServiceError::Rejected(errors));
    }

    let previous = store::load(pool).await?;
    store::save(pool, settings, Some(changed_by)).await?;

    // One entry per policy rather than one for the row, because the three are
    // read by different people for different reasons - and a workspace that
    // only ever touched its MFA settings should not have to read password-policy
    // noise to find out when. All three land on the one security-policy record,
    // so its history is the whole story in order; `updated` writes nothing when
    // a policy did not move, so saving the form does not produce three rows.
    audit::updated(
        pool,
        caller,
        Target::singleton(kinds::SECURITY_POLICY).named("Password policy"),
        &previous.password,
        &settings.password,
    )
    .await;

    audit::updated(
        pool,
        caller,
        Target::singleton(kinds::SECURITY_POLICY).named("Multi-factor policy"),
        &previous.mfa,
        &settings.mfa,
    )
    .await;

    // `always`, and this is the only call site that may say so. Switching the
    // change trail off must leave a row saying who did it - otherwise the one
    // act worth catching is the one act with no trace. Recorded after the save,
    // so a policy that now forbids recording does not suppress the record of
    // itself being set.
    audit::updated(
        pool,
        caller,
        Target::singleton(kinds::SECURITY_POLICY)
            .named("Audit policy")
            .always(),
        &previous.audit,
        &settings.audit,
    )
    .await;

    if previous.mfa != settings.mfa {
        tracing::info!(
            from = %previous.mfa.enforcement,
            to = %settings.mfa.enforcement,
            "workspace MFA enforcement changed"
        );
    }

    Ok(())
}
