//! The `roles`, `user_roles` and `role_permissions` tables.
//!
//! Roles are rows, so an organization can define its own. Two of them - `Admin`
//! and `User` - are created by migration 0003 in every tenant database and
//! marked `is_static`; their permission grants are written from the compiled
//! definitions by [`sync_static_roles`], so the permission tree has exactly one
//! source of truth rather than being restated in SQL and left to drift.

use chrono::{DateTime, Utc};
use phonix_core::RoleSummary;
use phonix_core::authorization::{PermissionSet, roles as static_roles};
use phonix_core::identity::UserId;
use sqlx::{FromRow, PgExecutor, PgPool, Row};
use uuid::Uuid;

use crate::error::DbError;

/// One row of `roles`.
#[derive(Debug, Clone)]
pub struct RoleRecord {
    pub id: Uuid,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub is_static: bool,
    pub is_default: bool,
    pub created_at: DateTime<Utc>,
}

impl<'r> FromRow<'r, sqlx::postgres::PgRow> for RoleRecord {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            display_name: row.try_get("display_name")?,
            description: row.try_get("description")?,
            is_static: row.try_get("is_static")?,
            is_default: row.try_get("is_default")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

/// Bring the two static roles' grants in line with the compiled definitions.
///
/// Run once per tenant database, right after its migrations. Idempotent, so it
/// is also the upgrade path: a release that adds a permission gets it into
/// every `Admin` role by running this again.
///
/// Only the *static* roles are touched. A role an organization defined is
/// their business, and silently adding a new permission to it would hand out
/// access nobody asked for.
pub async fn sync_static_roles(pool: &PgPool) -> Result<(), DbError> {
    let mut tx = pool.begin().await.map_err(DbError::Query)?;

    // Admin holds the whole tree, by definition. Anything less and a release
    // that adds a permission leaves every workspace's administrator unable to
    // use the feature until someone edits the role by hand.
    replace_permissions_by_name(&mut tx, static_roles::ADMIN, &PermissionSet::all()).await?;

    // User gets only what is marked `default_for_user`. Grants an
    // administrator added on top are preserved: a workspace that gave every
    // user access to the audit log meant it.
    add_permissions_by_name(
        &mut tx,
        static_roles::USER,
        &PermissionSet::defaults_for_user(),
    )
    .await?;

    tx.commit().await.map_err(DbError::Query)?;

    tracing::debug!("static role permissions synchronised");
    Ok(())
}

/// Every role in this workspace, with counts, for the roles screen.
pub async fn list<'e, E>(executor: E) -> Result<Vec<RoleSummary>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query(
        "SELECT r.id, r.name, r.display_name, r.description, r.is_static, r.is_default,
                (SELECT count(*) FROM role_permissions rp WHERE rp.role_id = r.id) AS permission_count,
                (SELECT count(*) FROM user_roles ur
                   JOIN users u ON u.id = ur.user_id AND u.deleted_at IS NULL
                  WHERE ur.role_id = r.id) AS user_count
           FROM roles r
          ORDER BY r.is_static DESC, lower(r.name)",
    )
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    rows.into_iter()
        .map(|row| {
            Ok(RoleSummary {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                display_name: row.try_get("display_name")?,
                description: row.try_get("description")?,
                is_static: row.try_get("is_static")?,
                is_default: row.try_get("is_default")?,
                permission_count: row.try_get("permission_count")?,
                user_count: row.try_get("user_count")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(DbError::Query)
}

/// Find a role by name, case-insensitively.
pub async fn find_by_name<'e, E>(executor: E, name: &str) -> Result<Option<RoleRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, RoleRecord>(
        "SELECT id, name, display_name, description, is_static, is_default, created_at
           FROM roles WHERE lower(name) = lower($1)",
    )
    .bind(name)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)
}

/// The roles automatically given to every new user in this workspace.
pub async fn default_roles<'e, E>(executor: E) -> Result<Vec<RoleRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, RoleRecord>(
        "SELECT id, name, display_name, description, is_static, is_default, created_at
           FROM roles WHERE is_default ORDER BY lower(name)",
    )
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)
}

/// Create a role. The caller is expected to have validated the name through
/// `phonix_core::authorization::validate_role_name`, which refuses the static
/// names; the unique index is the backstop.
pub async fn create<'e, E>(
    executor: E,
    name: &str,
    display_name: &str,
    description: Option<&str>,
    is_default: bool,
) -> Result<RoleRecord, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, RoleRecord>(
        "INSERT INTO roles (name, display_name, description, is_static, is_default)
         VALUES ($1, $2, $3, FALSE, $4)
         RETURNING id, name, display_name, description, is_static, is_default, created_at",
    )
    .bind(name)
    .bind(display_name)
    .bind(description)
    .bind(is_default)
    .fetch_one(executor)
    .await
    .map_err(|err| match &err {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            DbError::RoleExists(name.to_owned())
        }
        _ => DbError::Query(err),
    })
}

/// Delete a role.
///
/// Refuses the static ones. Deleting `Admin` would leave a workspace nobody can
/// administer, and `user_roles` cascades, so it would happen silently.
pub async fn delete<'e, E>(executor: E, id: Uuid) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query("DELETE FROM roles WHERE id = $1 AND NOT is_static")
        .bind(id)
        .execute(executor)
        .await
        .map_err(DbError::Query)?;

    if result.rows_affected() == 0 {
        return Err(DbError::StaticRoleProtected);
    }
    Ok(())
}

/// One role by id.
pub async fn find_by_id<'e, E>(executor: E, id: Uuid) -> Result<Option<RoleRecord>, DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query_as::<_, RoleRecord>(
        "SELECT id, name, display_name, description, is_static, is_default, created_at
           FROM roles WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(executor)
    .await
    .map_err(DbError::Query)
}

/// Rename or re-describe a role.
///
/// A static role keeps its `name` whatever is passed - code assigns `Admin` by
/// that string, and `is_static` in the `WHERE` is what makes that true of the
/// database rather than only of the caller. Its label, description and default
/// flag are ordinary editable columns.
pub async fn update<'e, E>(
    executor: E,
    id: Uuid,
    name: &str,
    display_name: &str,
    description: Option<&str>,
    is_default: bool,
) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "UPDATE roles
            SET name = CASE WHEN is_static THEN name ELSE $2 END,
                display_name = $3,
                description = $4,
                is_default = $5
          WHERE id = $1",
    )
    .bind(id)
    .bind(name)
    .bind(display_name)
    .bind(description)
    .bind(is_default)
    .execute(executor)
    .await
    .map_err(|err| match &err {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            DbError::RoleExists(name.to_owned())
        }
        _ => DbError::Query(err),
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Role permissions
// ---------------------------------------------------------------------------

/// The permissions a role grants.
///
/// Names this build does not define are pruned on the way out, so a rollback to
/// an older binary cannot resurrect a permission it does not enforce.
pub async fn permissions_of<'e, E>(executor: E, role_id: Uuid) -> Result<PermissionSet, DbError>
where
    E: PgExecutor<'e>,
{
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT name FROM role_permissions WHERE role_id = $1")
            .bind(role_id)
            .fetch_all(executor)
            .await
            .map_err(DbError::Query)?;

    let mut set: PermissionSet = rows.into_iter().map(|(name,)| name).collect();
    let dropped = set.prune_unknown();
    if !dropped.is_empty() {
        tracing::debug!(?dropped, %role_id, "ignoring grants this build does not define");
    }
    Ok(set)
}

/// Replace a role's grants wholesale. What the role editor submits.
pub async fn set_permissions(
    pool: &PgPool,
    role_id: Uuid,
    permissions: &PermissionSet,
) -> Result<(), DbError> {
    let mut tx = pool.begin().await.map_err(DbError::Query)?;

    // Delete-then-insert inside one transaction, so a concurrent permission
    // check never observes the role with an empty set.
    sqlx::query("DELETE FROM role_permissions WHERE role_id = $1")
        .bind(role_id)
        .execute(&mut *tx)
        .await
        .map_err(DbError::Query)?;

    insert_permissions(&mut *tx, role_id, permissions).await?;

    tx.commit().await.map_err(DbError::Query)?;
    Ok(())
}

// The helpers below run more than one statement, so they take a connection
// rather than a generic executor: `&mut PgConnection` is not `Copy`, and a
// generic `E` would have to be to be used twice. A transaction derefs to one,
// which is how `sync_static_roles` keeps both calls inside its transaction.

async fn replace_permissions_by_name(
    conn: &mut sqlx::PgConnection,
    role_name: &str,
    permissions: &PermissionSet,
) -> Result<(), DbError> {
    let Some(role) = find_by_name(&mut *conn, role_name).await? else {
        // The static roles are inserted by migration 0003. Their absence means
        // the database was not migrated, which is worth failing loudly for.
        return Err(DbError::MissingStaticRole(role_name.to_owned()));
    };

    sqlx::query("DELETE FROM role_permissions WHERE role_id = $1")
        .bind(role.id)
        .execute(&mut *conn)
        .await
        .map_err(DbError::Query)?;

    insert_permissions(&mut *conn, role.id, permissions).await
}

async fn add_permissions_by_name(
    conn: &mut sqlx::PgConnection,
    role_name: &str,
    permissions: &PermissionSet,
) -> Result<(), DbError> {
    let Some(role) = find_by_name(&mut *conn, role_name).await? else {
        return Err(DbError::MissingStaticRole(role_name.to_owned()));
    };

    insert_permissions(&mut *conn, role.id, permissions).await
}

/// Insert grants, ignoring ones already present.
async fn insert_permissions<'e, E>(
    executor: E,
    role_id: Uuid,
    permissions: &PermissionSet,
) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    let names: Vec<String> = permissions.iter().map(str::to_owned).collect();
    if names.is_empty() {
        return Ok(());
    }

    // One statement with an array parameter rather than a loop: N round trips
    // to insert a dozen rows is a lot of latency for nothing, and `unnest`
    // keeps the values bound rather than interpolated.
    sqlx::query(
        "INSERT INTO role_permissions (role_id, name)
         SELECT $1, name FROM unnest($2::text[]) AS name
         ON CONFLICT DO NOTHING",
    )
    .bind(role_id)
    .bind(&names)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// User roles
// ---------------------------------------------------------------------------

/// Give a user a role. Idempotent.
pub async fn assign_to_user<'e, E>(
    executor: E,
    user_id: UserId,
    role_id: Uuid,
    granted_by: Option<UserId>,
) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "INSERT INTO user_roles (user_id, role_id, granted_by)
         VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(role_id)
    .bind(granted_by)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// Give a user a role by name. Used at signup to assign `Admin`.
pub async fn assign_to_user_by_name(
    conn: &mut sqlx::PgConnection,
    user_id: UserId,
    role_name: &str,
) -> Result<(), DbError> {
    let Some(role) = find_by_name(&mut *conn, role_name).await? else {
        return Err(DbError::MissingStaticRole(role_name.to_owned()));
    };
    assign_to_user(&mut *conn, user_id, role.id, None).await
}

/// Take a role away.
///
/// Refuses to strip `Admin` from the workspace owner. Their flag is what makes
/// "nobody can administer this workspace" unreachable, and a UI that forgot to
/// check would reach it in one click.
pub async fn remove_from_user<'e, E>(
    executor: E,
    user_id: UserId,
    role_id: Uuid,
) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    let result = sqlx::query(
        "DELETE FROM user_roles ur
          USING roles r, users u
          WHERE ur.role_id = r.id
            AND ur.user_id = u.id
            AND ur.user_id = $1
            AND ur.role_id = $2
            AND NOT (u.is_owner AND lower(r.name) = 'admin')",
    )
    .bind(user_id)
    .bind(role_id)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    if result.rows_affected() == 0 {
        return Err(DbError::OwnerProtected);
    }
    Ok(())
}

/// Replace a user's roles with exactly this set of names.
pub async fn set_user_roles(
    pool: &PgPool,
    user_id: UserId,
    role_names: &[String],
    granted_by: Option<UserId>,
) -> Result<(), DbError> {
    let mut tx = pool.begin().await.map_err(DbError::Query)?;

    // The owner keeps Admin whatever the caller submitted.
    sqlx::query(
        "DELETE FROM user_roles ur
          USING roles r, users u
          WHERE ur.role_id = r.id
            AND ur.user_id = u.id
            AND ur.user_id = $1
            AND NOT (u.is_owner AND lower(r.name) = 'admin')",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(DbError::Query)?;

    if !role_names.is_empty() {
        sqlx::query(
            "INSERT INTO user_roles (user_id, role_id, granted_by)
             SELECT $1, r.id, $3 FROM roles r
              WHERE lower(r.name) = ANY(SELECT lower(n) FROM unnest($2::text[]) AS n)
             ON CONFLICT DO NOTHING",
        )
        .bind(user_id)
        .bind(role_names)
        .bind(granted_by)
        .execute(&mut *tx)
        .await
        .map_err(DbError::Query)?;
    }

    tx.commit().await.map_err(DbError::Query)?;
    Ok(())
}

/// Assign every role marked `is_default`. Called when a user is created.
pub async fn assign_default_roles<'e, E>(executor: E, user_id: UserId) -> Result<(), DbError>
where
    E: PgExecutor<'e>,
{
    sqlx::query(
        "INSERT INTO user_roles (user_id, role_id)
         SELECT $1, id FROM roles WHERE is_default
         ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .execute(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(())
}

/// The names of the roles a user holds.
pub async fn names_for_user<'e, E>(executor: E, user_id: UserId) -> Result<Vec<String>, DbError>
where
    E: PgExecutor<'e>,
{
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT r.name FROM roles r
           JOIN user_roles ur ON ur.role_id = r.id
          WHERE ur.user_id = $1
          ORDER BY r.is_static DESC, lower(r.name)",
    )
    .bind(user_id)
    .fetch_all(executor)
    .await
    .map_err(DbError::Query)?;

    Ok(rows.into_iter().map(|(name,)| name).collect())
}

#[cfg(test)]
mod tests {
    use phonix_core::authorization::names;

    use super::*;

    #[test]
    fn the_admin_role_is_synchronised_with_the_whole_tree() {
        // What `sync_static_roles` writes. If a permission is added to the
        // definitions and this changes, every workspace's Admin picks it up on
        // the next sync - which is the property worth pinning down.
        let admin = PermissionSet::all();
        assert!(admin.contains_all_defined());
        assert!(admin.is_granted(names::USERS_DELETE));
        assert!(admin.is_granted(names::AUDIT_LOGS));
    }

    #[test]
    fn the_user_role_cannot_reach_administration() {
        let user = PermissionSet::defaults_for_user();
        assert!(user.is_granted(names::DASHBOARD));
        assert!(!user.has_any_under(names::ADMINISTRATION));
    }
}
