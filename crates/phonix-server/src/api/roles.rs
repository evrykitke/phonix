//! `/api/v1/roles` - what a workspace's roles are, and what each one grants.
//!
//! Read-only, and gated on `Pages.Administration.Roles`.
//!
//! # Why this one earns its place in `v1`
//!
//! A key's scopes are permission *names*, and its effective power is its
//! owner's grants intersected with them. Until now nothing on the published
//! surface said what those names are or which of them a person actually holds,
//! so building a correctly-scoped key meant reading them off the administration
//! screen by eye. `GET /roles/{id}` answers it: the permissions a role grants
//! are exactly the strings a scope may name.
//!
//! It also completes the users resource. `User.roles` is a list of role names
//! and nothing else; this is where a client resolves one.
//!
//! # Two shapes, because the list and the detail answer different questions
//!
//! The list carries `permission_count` - a number a table can show without
//! fetching a set per row. The detail carries the set. That is the split
//! `phonix_core::authorization` already draws between `RoleSummary` and
//! `RoleDetail`, and the wire keeps it rather than inventing a third answer
//! where every row drags its whole permission list along.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use phonix_core::authorization::{RoleDetail, RoleSummary};
use phonix_core::query::{Page, PageRequest};
use phonix_services::authorization::{grants, roles};
use utoipa::ToSchema;
use uuid::Uuid;

use super::auth::ApiCaller;
use super::paging::{ListParams, ListRequest, PageEnvelope, cut};
use super::problem::Problem;

/// A role this workspace has defined.
#[derive(Debug, Clone, serde::Serialize, ToSchema)]
#[schema(as = Role)]
pub struct RoleResource {
    pub id: Uuid,
    /// The stable key. This is what appears in a user's `roles` and what
    /// `filter[role]` on `/users` matches.
    #[schema(example = "Administrator")]
    pub name: String,
    /// What a screen shows. Free to change without breaking anything that
    /// refers to the role by `name`.
    pub display_name: String,
    pub description: Option<String>,
    /// Ships with the product, and cannot be renamed or deleted. A client
    /// offering those actions should not offer them on this row.
    pub is_static: bool,
    /// Given automatically to every account created from now on. It does not
    /// reach accounts that already exist.
    pub is_default: bool,
    /// How many permissions it grants. The names are on `GET /roles/{id}`,
    /// which is one request rather than one per row.
    pub permission_count: i64,
    /// How many people hold it.
    pub user_count: i64,
}

impl From<&RoleSummary> for RoleResource {
    fn from(row: &RoleSummary) -> Self {
        Self {
            id: row.id,
            name: row.name.clone(),
            display_name: row.display_name.clone(),
            description: row.description.clone(),
            is_static: row.is_static,
            is_default: row.is_default,
            permission_count: row.permission_count,
            user_count: row.user_count,
        }
    }
}

/// A role and every permission it grants.
#[derive(Debug, Clone, serde::Serialize, ToSchema)]
#[schema(as = RoleDetail)]
pub struct RoleDetailResource {
    /// The same object `GET /roles` returns, nested rather than merged into
    /// this one. A client that already has a `Role` from the list can read
    /// this field into the same type, and adding a field to `Role` cannot
    /// collide with a field added here.
    pub role: RoleResource,
    /// Permission names, sorted. These are exactly the strings an API key's
    /// `scopes` may contain - a scope names a permission, and this is where a
    /// client learns which ones exist and which this role confers.
    ///
    /// Note this is what the role grants, not what any particular person ends
    /// up with: an individual denial can take one away, and an account holding
    /// two roles has the union.
    #[schema(example = json!(["Pages.Administration", "Pages.Administration.Users"]))]
    pub permissions: Vec<String>,
}

/// Every role this workspace has defined.
///
/// Searches name, display name and description. Sorts by `name` (the default),
/// `display_name`, `permission_count` or `user_count`. Narrows on
/// `filter[static]` and `filter[default]`, each `true` or `false`.
#[utoipa::path(
    get,
    path = "/roles",
    tag = "roles",
    operation_id = "listRoles",
    params(
        ListParams,
        ("filter[static]" = Option<String>, Query,
            description = "`true` for the roles that ship with the product, `false` for \
                           the ones this workspace defined.",
            example = "false"),
        ("filter[default]" = Option<String>, Query,
            description = "`true` for roles given automatically to new accounts.",
            example = "true"),
    ),
    responses(
        (status = 200, description = "One page of roles", body = PageEnvelope<RoleResource>),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "No API access, or the key does not carry Roles", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn list(
    caller: ApiCaller,
    ListRequest(request): ListRequest,
) -> Result<Json<PageEnvelope<RoleResource>>, Problem> {
    let rows = roles::list(&caller.pool, &caller.caller).await?;

    Ok(Json(PageEnvelope::new(paginate(rows, &request))))
}

/// One role, with everything it grants.
#[utoipa::path(
    get,
    path = "/roles/{id}",
    tag = "roles",
    operation_id = "getRole",
    params(("id" = Uuid, Path, description = "The role's id")),
    responses(
        (status = 200, description = "The role and its permissions", body = RoleDetailResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Roles", body = Problem),
        (status = 404, description = "No such role on this workspace", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn get(caller: ApiCaller, Path(id): Path<Uuid>) -> Result<Json<RoleDetailResource>, Problem> {
    // `grants::role_permissions` answers a missing role with
    // `ServiceError::rejected`, which renders as a 422 with a field error -
    // right for the role editor's form, wrong for an address with nothing at
    // it. Same trap `users::get` documents, and the same answer: find the row
    // first and spell the 404, then ask the question that assumes it exists.
    //
    // Both calls list the roles, and neither is worth avoiding: a workspace
    // has a handful, and one extra read of them buys the status code every
    // client's router already branches on.
    let summaries = roles::list(&caller.pool, &caller.caller).await?;

    if !summaries.iter().any(|role| role.id == id) {
        return Err(Problem::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "There is no role with that id on this workspace.",
        ));
    }

    let detail = grants::role_permissions(&caller.pool, &caller.caller, id).await?;

    Ok(Json(resource_of(&detail)))
}

/// The wire shape of one role and its grants.
fn resource_of(detail: &RoleDetail) -> RoleDetailResource {
    RoleDetailResource {
        role: RoleResource::from(&detail.summary),
        // `PermissionSet` is a `BTreeSet`, so this is already sorted and
        // stable - a client diffing two reads of the same role sees a change
        // only when one happened.
        permissions: detail.permissions.iter().map(str::to_owned).collect(),
    }
}

/// Search, narrow, sort and cut one page.
fn paginate(rows: Vec<RoleSummary>, request: &PageRequest) -> Page<RoleResource> {
    let needle = request.needle();

    let mut matching: Vec<&RoleSummary> = rows
        .iter()
        .filter(|row| match &needle {
            Some(needle) => {
                row.name.to_lowercase().contains(needle)
                    || row.display_name.to_lowercase().contains(needle)
                    || row
                        .description
                        .as_deref()
                        .is_some_and(|text| text.to_lowercase().contains(needle))
            }
            None => true,
        })
        .filter(|row| match request.filter("static") {
            Some(flag) => row.is_static == is_true(flag),
            None => true,
        })
        .filter(|row| match request.filter("default") {
            Some(flag) => row.is_default == is_true(flag),
            None => true,
        })
        .collect();

    let descending = request
        .sort
        .as_ref()
        .is_some_and(|sort| !sort.direction.is_ascending());

    // Name is the tie-break everywhere, lowercased. `roles.name` is matched
    // case-insensitively, so the lowercased name is still unique per
    // workspace - no two rows can compare equal after it, and paging cannot
    // show one row twice. Lowercased rather than raw because the default sort
    // is, and a tie-break that ordered `Bookkeeper` before `auditor` while the
    // default put it after would make the same two rows swap places depending
    // on which column you sorted by.
    match request.sort.as_ref().map(|sort| sort.field.as_str()) {
        Some("display_name") => matching.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        }),
        Some("permission_count") => matching.sort_by(|a, b| {
            a.permission_count
                .cmp(&b.permission_count)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        }),
        Some("user_count") => {
            matching.sort_by(|a, b| a.user_count.cmp(&b.user_count).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        }
        _ => matching.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
    }
    if descending {
        matching.reverse();
    }

    cut(matching, request, RoleResource::from)
}

/// A boolean in a query string.
///
/// Only `true` is true. Anything else - `false`, `0`, `yes`, a typo - narrows
/// the other way rather than being refused, which is the paging contract's rule
/// for a value this reader does not recognise. The two filters here are
/// two-valued, so "not true" and "false" are the same set and there is nothing
/// a third answer could mean.
fn is_true(flag: &str) -> bool {
    flag.eq_ignore_ascii_case("true")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(name: &str, is_static: bool, permissions: i64, users: i64) -> RoleSummary {
        RoleSummary {
            id: Uuid::new_v4(),
            name: name.to_owned(),
            display_name: name.to_owned(),
            description: None,
            is_static,
            is_default: false,
            permission_count: permissions,
            user_count: users,
        }
    }

    fn defined() -> Vec<RoleSummary> {
        vec![
            role("Admin", true, 120, 2),
            role("User", true, 8, 40),
            role("Bookkeeper", false, 15, 3),
            role("auditor", false, 4, 1),
        ]
    }

    fn names(page: &Page<RoleResource>) -> Vec<String> {
        page.rows.iter().map(|row| row.name.clone()).collect()
    }

    #[test]
    fn the_default_page_sorts_by_name_ignoring_case() {
        let page = paginate(defined(), &PageRequest::default());

        // Byte order would put "auditor" last, behind every capital.
        assert_eq!(names(&page), vec!["Admin", "auditor", "Bookkeeper", "User"]);
        assert_eq!(page.total, 4);
    }

    #[test]
    fn the_static_filter_separates_what_ships_from_what_a_workspace_defined() {
        let ours = paginate(defined(), &PageRequest::default().filtered_by("static", "false"));
        let theirs = paginate(defined(), &PageRequest::default().filtered_by("static", "true"));

        assert_eq!(names(&ours), vec!["auditor", "Bookkeeper"]);
        assert_eq!(names(&theirs), vec!["Admin", "User"]);
    }

    #[test]
    fn a_boolean_that_is_not_true_narrows_the_other_way() {
        // Two-valued, so there is no third set for a typo to mean - and
        // refusing would break the contract that a bad parameter still
        // answers with rows.
        let page = paginate(defined(), &PageRequest::default().filtered_by("static", "yes"));

        assert_eq!(names(&page), vec!["auditor", "Bookkeeper"]);
    }

    #[test]
    fn sorting_by_a_count_breaks_ties_by_name() {
        let mut rows = defined();
        rows[2].user_count = 1;

        // Bookkeeper and auditor now both have one holder, and only the
        // tie-break decides which comes first.
        let request = PageRequest {
            sort: Some(phonix_core::query::Sort::ascending("user_count")),
            ..PageRequest::default()
        };
        let page = paginate(rows, &request.sanitised());

        assert_eq!(names(&page)[..2], ["auditor".to_owned(), "Bookkeeper".to_owned()]);
    }

    #[test]
    fn a_search_looks_at_the_description_as_well_as_the_names() {
        let mut rows = defined();
        rows[3].description = Some("Reads everything, changes nothing".to_owned());

        let request = PageRequest {
            search: "changes nothing".to_owned(),
            ..PageRequest::default()
        };
        let page = paginate(rows, &request.sanitised());

        assert_eq!(names(&page), vec!["auditor"]);
    }

    #[test]
    fn the_detail_carries_the_permission_names_a_scope_may_use() {
        use phonix_core::authorization::PermissionSet;

        let mut permissions = PermissionSet::new();
        permissions.insert_exact("Pages.Administration.Users");
        permissions.insert_exact("Pages.Administration");

        let detail = RoleDetail {
            summary: role("Bookkeeper", false, 2, 3),
            permissions,
        };

        let resource = resource_of(&detail);

        // Sorted, because the set is a BTreeSet - so two reads of an unchanged
        // role are byte-identical, and a client diffing them sees nothing.
        assert_eq!(
            resource.permissions,
            vec!["Pages.Administration", "Pages.Administration.Users"]
        );
        assert_eq!(resource.role.name, "Bookkeeper");
    }
}
