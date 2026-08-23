//! Resolving what a user may actually do.
//!
//! ```text
//! union of the user's roles' grants
//!   .. plus  individual grants   (user_permissions where is_granted = true)
//!   .. minus individual denials  (user_permissions where is_granted = false)
//! ```
//!
//! A denial beats any role grant, so one person can be excluded from something
//! their role allows without inventing a near-duplicate role. That precedence
//! is the whole reason `user_permissions` carries a flag while
//! `role_permissions` does not.
//!
//! The resolution is a single query. Doing it as three - roles, then grants,
//! then denials - would open a window in which a role change lands between two
//! of them and produces a set that never existed.

use phonix_core::authorization::PermissionSet;
use phonix_core::identity::UserId;
use sqlx::PgExecutor;

use crate::error::DbError;

/// Everything a user may do, flattened.
///
/// Returns an empty set for a user with no roles and no overrides, which is the
/// correct answer and also what an unknown id yields - "deny" is the right
/// default for a lookup that found nothing.
pub async fn resolve_for_user<'e, E>(executor: E, user_id: UserId) -> Result<PermissionSet, DbError>
where
    E: PgExecutor<'e>,
{
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT name FROM (
                 SELECT rp.name
                   FROM role_permissions rp
                   JOIN user_roles ur ON ur.role_id = rp.role_id
                  WHERE ur.user_id = $1
                 UNION
                 SELECT up.name
                   FROM user_permissions up
                  WHERE up.user_id = $1 AND up.is_granted
             ) AS granted
          WHERE name NOT IN (
                 SELECT name FROM user_permissions
                  WHERE user_id = $1 AND NOT is_granted
             )",
    )
    .bind(user_id)
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    let mut set: PermissionSet = rows.into_iter().map(|(name,)| name).collect();

    // A grant for a permission this build no longer defines must not survive:
    // a later release that reuses the name would silently inherit it.
    let dropped = set.prune_unknown();
    if !dropped.is_empty() {
        tracing::debug!(
            %user_id,
            ?dropped,
            "ignoring grants this build does not define"
        );
    }

    Ok(set)
}

/// A user's individual overrides, separated into grants and denials.
///
/// For the per-user permission editor, which has to show the two apart from
/// what the roles already give.
pub async fn overrides_for_user<'e, E>(
    executor: E,
    user_id: UserId,
) -> Result<UserOverrides, DbError>
where
    E: PgExecutor<'e>,
{
    let rows: Vec<(String, bool)> =
        sqlx::query_as("SELECT name, is_granted FROM user_permissions WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(executor)
            .await
            .map_err(DbError::Query)?;

    let mut overrides = UserOverrides::default();
    for (name, is_granted) in rows {
        if is_granted {
            overrides.granted.grant(&name);
        } else {
            overrides.denied.grant(&name);
        }
    }

    Ok(overrides)
}

/// Per-user additions and subtractions on top of their roles.
#[derive(Debug, Clone, Default)]
pub struct UserOverrides {
    pub granted: PermissionSet,
    pub denied: PermissionSet,
}

/// Set one individual override.
///
/// `is_granted = false` records an explicit denial, which is not the same as
/// having no row: the row is what overrules a role.
pub async fn set_override<'e, E>(
    executor: E,
    user_id: UserId,
    name: &str,
    is_granted: bool,
    set_by: Option<UserId>,
) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    if !phonix_core::authorization::is_defined(name) {
        return Err(DbError::UnknownPermission(name.to_owned()));
    }

    sqlx::query(
        "INSERT INTO user_permissions (user_id, name, is_granted, set_by)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (user_id, name)
         DO UPDATE SET is_granted = EXCLUDED.is_granted,
                       set_by     = EXCLUDED.set_by,
                       set_at     = now()",
    )
    .bind(user_id)
    .bind(name)
    .bind(is_granted)
    .bind(set_by)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Remove an override, returning the user to whatever their roles say.
pub async fn clear_override<'e, E>(executor: E, user_id: UserId, name: &str) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query("DELETE FROM user_permissions WHERE user_id = $1 AND name = $2")
        .bind(user_id)
        .bind(name)
        .execute(executor)
        .await
        .map_err(DbError::Query)?;
    Ok(())
}

/// Remove every override for a user.
pub async fn clear_all_overrides<'e, E>(executor: E, user_id: UserId) -> Result<u64, DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query("DELETE FROM user_permissions WHERE user_id = $1")
        .bind(user_id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?;
    Ok(result.rows_affected())
}

/// Whether a user holds one permission.
///
/// Convenience over [`resolve_for_user`]. Prefer resolving once per request and
/// checking the set repeatedly - this is a query every time it is called.
pub async fn is_granted<'e, E>(executor: E, user_id: UserId, name: &str) -> Result<bool, DbError>
where
    E: PgExecutor<'e>,
{
    Ok(resolve_for_user(executor, user_id).await?.is_granted(name))
}

#[cfg(test)]
mod tests {
    use phonix_core::authorization::names;

    use super::*;

    /// The precedence rule from the module doc, as plain set arithmetic.
    ///
    /// The real thing is one SQL statement; this pins down what that statement
    /// is supposed to mean, independently of whether a database is running.
    fn resolve(
        role_grants: &[&str],
        individual_grants: &[&str],
        individual_denials: &[&str],
    ) -> PermissionSet {
        let mut set = PermissionSet::new();
        for name in role_grants {
            set.grant(name);
        }
        for name in individual_grants {
            set.grant(name);
        }
        for name in individual_denials {
            set.revoke(name);
        }
        set
    }

    #[test]
    fn roles_are_unioned() {
        let set = resolve(&[names::DASHBOARD, names::AUDIT_LOGS], &[], &[]);
        assert!(set.is_granted(names::DASHBOARD));
        assert!(set.is_granted(names::AUDIT_LOGS));
    }

    #[test]
    fn an_individual_grant_adds_to_a_role() {
        let set = resolve(&[names::DASHBOARD], &[names::USERS_EDIT], &[]);
        assert!(set.is_granted(names::USERS_EDIT));
        assert!(set.is_granted(names::DASHBOARD));
    }

    #[test]
    fn an_individual_denial_beats_a_role_grant() {
        // The point of the whole design: exclude one person from one thing
        // without cloning their role.
        let set = resolve(
            &[names::USERS, names::USERS_EDIT, names::USERS_DELETE],
            &[],
            &[names::USERS_DELETE],
        );

        assert!(set.is_granted(names::USERS_EDIT));
        assert!(!set.is_granted(names::USERS_DELETE));
    }

    #[test]
    fn a_denial_takes_the_branch_beneath_it() {
        let set = resolve(
            &[
                names::USERS,
                names::USERS_EDIT,
                names::USERS_DELETE,
                names::ROLES,
            ],
            &[],
            &[names::USERS],
        );

        assert!(!set.is_granted(names::USERS));
        assert!(!set.is_granted(names::USERS_EDIT));
        assert!(!set.is_granted(names::USERS_DELETE));
        // A sibling branch survives.
        assert!(set.is_granted(names::ROLES));
    }

    #[test]
    fn no_roles_means_no_permissions() {
        assert!(resolve(&[], &[], &[]).is_empty());
    }
}
