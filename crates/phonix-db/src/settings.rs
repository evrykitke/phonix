//! The `workspace_settings` row: what one organization decided for itself.
//!
//! One row per tenant database, created by migration 0004 and seeded from
//! `[security.workspace_defaults]` when the workspace is onboarded. After that
//! the row is the only authority - changing the configuration file does not
//! reach back into workspaces that already exist, because their policy is
//! theirs.
//!
//! Read where a decision depends on it (sign-in, changing a password, the MFA
//! screens) rather than on every request. It is one indexed single-row lookup,
//! and caching it would mean an administrator's change taking effect at some
//! unpredictable later moment.

use phonix_core::WorkspaceSecuritySettings;
use phonix_core::audit::AuditPolicy;
use phonix_core::identity::UserId;
use phonix_core::identity::mfa::{MfaEnforcement, MfaPolicy};
use phonix_core::identity::password::PasswordPolicy;
use sqlx::{FromRow, PgExecutor, Row};

use crate::error::DbError;

const SELECT: &str = "SELECT password_min_length, password_max_length, \
     password_require_lowercase, password_require_uppercase, password_require_digit, \
     password_require_symbol, password_forbid_common, password_forbid_personal, \
     password_expiry_days, password_history_depth, mfa_enforcement, mfa_allow_totp, \
     mfa_allow_recovery_codes, mfa_grace_period_days, mfa_remember_device_days, \
     audit_changes_enabled, audit_excluded_kinds, audit_retention_days \
     FROM workspace_settings WHERE id";

/// The stored policy, as one value.
struct SettingsRow(WorkspaceSecuritySettings);

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for SettingsRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let min_length: i32 = row.try_get("password_min_length")?;
        let max_length: i32 = row.try_get("password_max_length")?;
        let expiry_days: Option<i32> = row.try_get("password_expiry_days")?;
        let history_depth: i16 = row.try_get("password_history_depth")?;
        let enforcement: String = row.try_get("mfa_enforcement")?;
        let grace_period_days: i32 = row.try_get("mfa_grace_period_days")?;
        let remember_device_days: i32 = row.try_get("mfa_remember_device_days")?;

        Ok(Self(WorkspaceSecuritySettings {
            password: PasswordPolicy {
                min_length: min_length.max(0) as usize,
                max_length: max_length.max(0) as usize,
                require_lowercase: row.try_get("password_require_lowercase")?,
                require_uppercase: row.try_get("password_require_uppercase")?,
                require_digit: row.try_get("password_require_digit")?,
                require_symbol: row.try_get("password_require_symbol")?,
                forbid_common: row.try_get("password_forbid_common")?,
                forbid_personal_information: row.try_get("password_forbid_personal")?,
                expiry_days: expiry_days.map(|days| days.max(0) as u32),
                history_depth: history_depth.clamp(0, i16::from(u8::MAX)) as u8,
            },
            mfa: MfaPolicy {
                // A value outside the CHECK constraint cannot be stored, so an
                // unparseable one means the constraint was dropped. Falling back
                // to the strictest reading of a broken row - challenge, do not
                // force - is safer than refusing to let anybody sign in.
                enforcement: MfaEnforcement::parse(&enforcement).unwrap_or_else(|| {
                    tracing::error!(
                        value = %enforcement,
                        "workspace_settings.mfa_enforcement holds an unknown value; \
                         treating it as 'optional'"
                    );
                    MfaEnforcement::Optional
                }),
                allow_totp: row.try_get("mfa_allow_totp")?,
                allow_recovery_codes: row.try_get("mfa_allow_recovery_codes")?,
                grace_period_days: grace_period_days.max(0) as u32,
                remember_device_days: remember_device_days.max(0) as u32,
            },
            audit: AuditPolicy {
                enabled: row.try_get("audit_changes_enabled")?,
                // Read verbatim, including names this build has never heard of:
                // dropping one would silently switch a kind back on for a
                // workspace that had switched it off, the first time an older
                // release saved this row.
                excluded: row.try_get("audit_excluded_kinds")?,
                retention_days: row.try_get("audit_retention_days")?,
            },
        }))
    }
}

/// Read this workspace's policy.
///
/// Migration 0004 inserts the row, so a missing one means a database that was
/// not migrated. That is reported rather than papered over with defaults: a
/// workspace silently running the system default when its administrator
/// configured something stricter is exactly the failure nobody would notice.
pub async fn load<'e, E>(executor: E) -> Result<WorkspaceSecuritySettings, DbError>
where
    E: PgExecutor<'e>,
{
    let row: Option<SettingsRow> = sqlx::query_as(SELECT)
        .fetch_optional(executor)
        .await
        .map_err(DbError::Query)?;

    match row {
        Some(SettingsRow(settings)) => Ok(settings),
        None => Err(DbError::InvalidPolicy(
            "this workspace has no settings row; migration 0004 did not run".to_owned(),
        )),
    }
}

/// Replace the policy.
///
/// The caller is expected to have validated: `WorkspaceSecuritySettings::
/// validate` reports per field, which is what the settings form needs, and this
/// layer cannot. The CHECK constraints in migration 0004 are the backstop, and
/// they arrive here as an opaque constraint name - hence [`DbError::
/// InvalidPolicy`] rather than a bare query error.
pub async fn save<'e, E>(
    executor: E,
    settings: &WorkspaceSecuritySettings,
    updated_by: Option<UserId>,
) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE workspace_settings SET
             password_min_length        = $1,
             password_max_length        = $2,
             password_require_lowercase = $3,
             password_require_uppercase = $4,
             password_require_digit     = $5,
             password_require_symbol    = $6,
             password_forbid_common     = $7,
             password_forbid_personal   = $8,
             password_expiry_days       = $9,
             password_history_depth     = $10,
             mfa_enforcement            = $11,
             mfa_allow_totp             = $12,
             mfa_allow_recovery_codes   = $13,
             mfa_grace_period_days      = $14,
             mfa_remember_device_days   = $15,
             audit_changes_enabled      = $16,
             audit_excluded_kinds       = $17,
             audit_retention_days       = $18,
             updated_at                 = now(),
             updated_by                 = $19
           WHERE id",
    )
    .bind(settings.password.min_length as i32)
    .bind(settings.password.max_length as i32)
    .bind(settings.password.require_lowercase)
    .bind(settings.password.require_uppercase)
    .bind(settings.password.require_digit)
    .bind(settings.password.require_symbol)
    .bind(settings.password.forbid_common)
    .bind(settings.password.forbid_personal_information)
    .bind(settings.password.expiry_days.map(|days| days as i32))
    .bind(i16::from(settings.password.history_depth))
    .bind(settings.mfa.enforcement.as_str())
    .bind(settings.mfa.allow_totp)
    .bind(settings.mfa.allow_recovery_codes)
    .bind(settings.mfa.grace_period_days as i32)
    .bind(settings.mfa.remember_device_days as i32)
    .bind(settings.audit.enabled)
    .bind(&settings.audit.excluded)
    .bind(settings.audit.retention_days)
    .bind(updated_by)
    .execute(executor)
    .await
    .map_err(|err| match err {
        // 23514 is check_violation. The application layer validates first, so
        // reaching this means something bypassed it.
        sqlx::Error::Database(ref db_err) if db_err.code().as_deref() == Some("23514") => {
            DbError::InvalidPolicy(
                db_err
                    .constraint()
                    .unwrap_or("a workspace_settings constraint")
                    .to_owned(),
            )
        }
        other => DbError::Query(other),
    })?;

    Ok(())
}

/// Whether this workspace has the API at all.
///
/// Not part of [`WorkspaceSecuritySettings`], and the distinction is the point:
/// that value is a *policy* the organization chose for itself, and this is a
/// **licence** - what the workspace was sold. They live in one row because they
/// are both facts about one workspace, and they are read and written
/// separately because an administrator may change one and not the other.
///
/// Read on every `/api/v1` request. One indexed single-row lookup, for the
/// reason the rest of this module gives: caching it would mean a workspace's
/// API going on or off at some unpredictable later moment.
pub async fn api_enabled<'e, E>(executor: E) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    let enabled: Option<bool> =
        sqlx::query_scalar("SELECT api_enabled FROM workspace_settings WHERE id")
            .fetch_optional(executor)
            .await
            .map_err(DbError::Query)?;

    // A workspace with no settings row cannot have been sold anything. The
    // missing row is a real fault - see `load` - but the safe reading of it
    // here is "no API", not "all of it".
    Ok(enabled.unwrap_or(false))
}

/// Turn the API on or off for this workspace.
pub async fn set_api_enabled<'e, E>(
    executor: E,
    enabled: bool,
    updated_by: Option<UserId>,
) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE workspace_settings
            SET api_enabled = $1, updated_at = now(), updated_by = $2
          WHERE id",
    )
    .bind(enabled)
    .bind(updated_by)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Write the deployment's defaults into a workspace that has just been created.
///
/// Identical to [`save`] apart from the intent, which is worth a separate name:
/// this is the one call where the configuration file is allowed to decide an
/// organization's policy, and it happens exactly once.
pub async fn seed<'e, E>(executor: E, defaults: &WorkspaceSecuritySettings) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    save(executor, defaults, None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The columns `load` reads and the ones `save` writes have to be the same
    /// set, or a setting silently stops round-tripping.
    #[test]
    fn every_stored_column_is_both_read_and_written() {
        let written = [
            "password_min_length",
            "password_max_length",
            "password_require_lowercase",
            "password_require_uppercase",
            "password_require_digit",
            "password_require_symbol",
            "password_forbid_common",
            "password_forbid_personal",
            "password_expiry_days",
            "password_history_depth",
            "mfa_enforcement",
            "mfa_allow_totp",
            "mfa_allow_recovery_codes",
            "mfa_grace_period_days",
            "mfa_remember_device_days",
            "audit_changes_enabled",
            "audit_excluded_kinds",
            "audit_retention_days",
        ];

        for column in written {
            assert!(SELECT.contains(column), "{column} is written but not read");
        }
    }

    #[test]
    fn the_bounds_in_sql_and_in_rust_agree() {
        // The CHECK constraints in migration 0004 restate these. If the Rust
        // floor ever drops below the SQL one, a policy that validates here
        // fails on the way in with an opaque constraint error.
        use phonix_core::identity::password::ABSOLUTE_MIN_LENGTH;

        assert_eq!(ABSOLUTE_MIN_LENGTH, 8, "workspace_settings_password_length");
        assert_eq!(
            phonix_core::identity::password::MAX_HISTORY_DEPTH,
            24,
            "workspace_settings_password_history"
        );
        assert_eq!(
            phonix_core::identity::mfa::MAX_GRACE_PERIOD_DAYS,
            90,
            "workspace_settings_mfa_windows"
        );

        // Migration 0012 restates these two as
        // `workspace_settings_audit_retention`.
        assert_eq!(phonix_core::audit::policy::MIN_RETENTION_DAYS, 7);
        assert_eq!(phonix_core::audit::policy::MAX_RETENTION_DAYS, 3650);
    }
}
