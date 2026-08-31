//! `/api/v1/users` - the second resource, and read-only on purpose.
//!
//! Currencies proved the machinery; this proves the two things it could not.
//! Its read is **gated**, on `Pages.Administration.Users`, where the currency
//! list is ungated - so a key with no scopes gets a 403 here and a 200 there,
//! which is the scope intersection working rather than merely compiling. And
//! it is addressed by a UUID rather than by a code the caller already knows,
//! so a client has to have read the list to reach a row.
//!
//! # Why there is no write here
//!
//! `directory::update_user` sets roles and status together, and roles are the
//! thing a key must not be able to hand itself. Adding `PUT` to `v1` later is
//! additive; publishing the wrong shape is not, so the write is worth
//! designing deliberately rather than inheriting from the screen's form.
//!
//! # Why this pages in memory
//!
//! `directory::list` is unpaged, and its own doc comment says that is
//! deliberate: the workspace that needs server-side paging needs a cursor and
//! a server-side search designed together, and neither should be guessed at.
//! This handler is not the place to force that decision - a public endpoint
//! quietly growing a second, different paging story for the same rows the
//! screen shows is how the two drift apart. It reads what the screen reads and
//! cuts the page here; when the service grows a `PageRequest`, this passes it
//! down and nothing on the wire changes.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use phonix_core::identity::directory::UserListing;
use phonix_core::identity::user::{UserId, UserStatus};
use phonix_core::query::{Page, PageRequest};
use phonix_services::identity::directory;
use utoipa::ToSchema;
use uuid::Uuid;

use super::auth::ApiCaller;
use super::paging::{ListParams, ListRequest, PageEnvelope, cut};
use super::problem::Problem;

/// Where an account stands.
///
/// Declared here rather than deriving `ToSchema` on `UserStatus`, for the
/// reason the whole module set exists: a variant renamed in `phonix-core` has
/// to stop this file compiling. It is also the one enum in `v1` that a client
/// will match on exhaustively, so adding a variant is a real event and should
/// look like one from here.
#[derive(Debug, Clone, Copy, serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = UserStatus)]
pub enum UserStatusResource {
    /// Invited, but has not set a password or verified their address yet.
    Pending,
    /// Ordinary.
    Active,
    /// Blocked by an administrator. Reversible.
    Suspended,
    /// Left the organization. Kept so documents they touched still resolve.
    Deactivated,
}

impl From<UserStatus> for UserStatusResource {
    fn from(status: UserStatus) -> Self {
        match status {
            UserStatus::Pending => Self::Pending,
            UserStatus::Active => Self::Active,
            UserStatus::Suspended => Self::Suspended,
            UserStatus::Deactivated => Self::Deactivated,
        }
    }
}

/// A person with an account on this workspace.
#[derive(Debug, Clone, serde::Serialize, ToSchema)]
#[schema(as = User)]
pub struct UserResource {
    pub id: Uuid,
    #[schema(example = "ada@example.com")]
    pub email: String,
    #[schema(example = "Ada Lovelace")]
    pub display_name: String,
    pub status: UserStatusResource,
    /// Created the workspace. Such an account cannot be suspended or stripped
    /// of its roles, so a client offering those actions should not offer them
    /// on this row.
    pub is_owner: bool,
    pub email_verified: bool,
    /// Holds at least one confirmed second factor.
    pub mfa_enabled: bool,
    /// Role names, which are the same vocabulary an API key's scopes are drawn
    /// from. Ordered as the database returned them.
    pub roles: Vec<String>,
    /// Set while the account is locked out after failed sign-ins. `null` is the
    /// ordinary case, and an instant in the past means the lockout has expired
    /// without anything having cleared the column - so this is not on its own
    /// the answer to "can they sign in".
    pub locked_until: Option<DateTime<Utc>>,
    /// Whether that lockout still held when this row was read. The comparison
    /// `locked_until` needs, done against the server's clock rather than the
    /// caller's.
    pub locked: bool,
    /// `null` for somebody who was invited and never arrived.
    pub last_login_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl From<&UserListing> for UserResource {
    fn from(row: &UserListing) -> Self {
        Self {
            id: row.id,
            email: row.email.clone(),
            display_name: row.display_name.clone(),
            status: row.status.into(),
            is_owner: row.is_owner,
            email_verified: row.email_verified,
            mfa_enabled: row.mfa_enabled,
            roles: row.roles.clone(),
            locked_until: row.locked_until,
            locked: row.locked,
            last_login_at: row.last_login_at,
            created_at: row.created_at,
        }
    }
}

/// Everybody with an account on this workspace.
///
/// Searches display name, address and role name - the three the screen's own
/// search box looks in, so a script and a person typing find the same rows.
///
/// Sorts by `display_name` (the default), `email`, `status`, `mfa_enabled`,
/// `created_at` or `last_login_at`. Ties always break the same way, so paging
/// through the list shows every row exactly once.
#[utoipa::path(
    get,
    path = "/users",
    tag = "users",
    operation_id = "listUsers",
    params(
        ListParams,
        ("filter[status]" = Option<String>, Query,
            description = "One of pending, active, suspended, deactivated.",
            example = "active"),
        ("filter[role]" = Option<String>, Query,
            description = "A role name, matched whole and case-insensitively.",
            example = "Administrator"),
    ),
    responses(
        (status = 200, description = "One page of accounts", body = PageEnvelope<UserResource>),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "No API access, or the key does not carry Users", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn list(
    caller: ApiCaller,
    ListRequest(request): ListRequest,
) -> Result<Json<PageEnvelope<UserResource>>, Problem> {
    let rows = directory::list(&caller.pool, &caller.caller).await?;

    Ok(Json(PageEnvelope::new(paginate(rows, &request))))
}

/// One account, by id.
#[utoipa::path(
    get,
    path = "/users/{id}",
    tag = "users",
    operation_id = "getUser",
    params(("id" = Uuid, Path, description = "The account's id")),
    responses(
        (status = 200, description = "The account", body = UserResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Users", body = Problem),
        (status = 404, description = "No such account on this workspace", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn get(caller: ApiCaller, Path(id): Path<UserId>) -> Result<Json<UserResource>, Problem> {
    // `directory::find` is the obvious call and the wrong one: it answers a
    // missing account with `ServiceError::rejected`, which this router renders
    // as a 422 carrying a field error. That is right for a form, where the id
    // came from a hidden input and being wrong is a validation failure. It is
    // wrong here, where the id is the address and there is nothing at it.
    // `list` applies the same `Caller::require`, and `find` scans the same
    // whole list anyway, so this costs nothing and gives a client's router the
    // 404 it expects.
    let rows = directory::list(&caller.pool, &caller.caller).await?;

    rows.iter()
        .find(|row| row.id == id)
        .map(|row| Json(UserResource::from(row)))
        .ok_or_else(|| {
            Problem::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "There is no account with that id on this workspace.",
            )
        })
}

/// Search, narrow, sort and cut one page.
fn paginate(rows: Vec<UserListing>, request: &PageRequest) -> Page<UserResource> {
    let needle = request.needle();

    let mut matching: Vec<&UserListing> = rows
        .iter()
        .filter(|row| match &needle {
            Some(needle) => {
                row.display_name.to_lowercase().contains(needle)
                    || row.email.to_lowercase().contains(needle)
                    || row
                        .roles
                        .iter()
                        .any(|role| role.to_lowercase().contains(needle))
            }
            None => true,
        })
        // A status this build does not have narrows to nothing rather than to
        // everything. Unlike an unknown sort field, the caller named a value
        // and meant it, and answering the whole list to `status=retired` reads
        // as "everybody is retired".
        .filter(|row| match request.filter("status") {
            Some(status) => row.status.as_str().eq_ignore_ascii_case(status),
            None => true,
        })
        // Whole rather than substring, because role names nest by convention
        // and "Admin" must not drag in "Administrator".
        .filter(|row| match request.filter("role") {
            Some(role) => row.roles.iter().any(|held| held.eq_ignore_ascii_case(role)),
            None => true,
        })
        .collect();

    let descending = request
        .sort
        .as_ref()
        .is_some_and(|sort| !sort.direction.is_ascending());

    // Every arm breaks its ties, because rows that compare equal under the
    // chosen field would otherwise sit in whatever order the database felt
    // like - and two rows that swap places between one request and the next
    // are a row that appears on both pages and a row that appears on neither.
    match request.sort.as_ref().map(|sort| sort.field.as_str()) {
        Some("email") => matching.sort_by(|a, b| {
            a.email
                .to_lowercase()
                .cmp(&b.email.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        }),
        Some("status") => matching.sort_by(|a, b| {
            a.status
                .as_str()
                .cmp(b.status.as_str())
                .then_with(|| a.id.cmp(&b.id))
        }),
        Some("mfa_enabled") => matching.sort_by(|a, b| {
            a.mfa_enabled
                .cmp(&b.mfa_enabled)
                .then_with(|| a.id.cmp(&b.id))
        }),
        Some("created_at") => {
            matching.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
        }
        // `None` sorts first ascending, which puts everybody who has never
        // signed in at the top - the end of the list somebody chasing dormant
        // invitations actually wants.
        Some("last_login_at") => matching.sort_by(|a, b| {
            a.last_login_at
                .cmp(&b.last_login_at)
                .then_with(|| a.id.cmp(&b.id))
        }),
        // Display name is the default, matching the screen, and compares
        // case-insensitively so "ada" and "Ada" are not two neighbourhoods.
        _ => matching.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        }),
    }
    if descending {
        matching.reverse();
    }

    cut(matching, request, UserResource::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(name: &str, email: &str, status: UserStatus, roles: &[&str]) -> UserListing {
        UserListing {
            id: Uuid::new_v4(),
            email: email.to_owned(),
            display_name: name.to_owned(),
            status,
            is_owner: false,
            email_verified: true,
            mfa_enabled: false,
            roles: roles.iter().map(|role| (*role).to_owned()).collect(),
            locked_until: None,
            locked: false,
            last_login_at: None,
            created_at: Utc::now(),
        }
    }

    fn directory() -> Vec<UserListing> {
        vec![
            person("ada lovelace", "ada@example.com", UserStatus::Active, &["Administrator"]),
            person("Grace Hopper", "grace@example.com", UserStatus::Active, &["Admin"]),
            person("Alan Turing", "alan@example.com", UserStatus::Suspended, &[]),
            person("Katherine Johnson", "kj@example.com", UserStatus::Pending, &["Administrator"]),
        ]
    }

    fn names(page: &Page<UserResource>) -> Vec<String> {
        page.rows.iter().map(|row| row.display_name.clone()).collect()
    }

    #[test]
    fn the_default_page_sorts_by_name_ignoring_case() {
        let page = paginate(directory(), &PageRequest::default());

        // Sorting on the raw bytes would put every capital ahead of every
        // lower case letter, and "ada" would land after "Katherine".
        assert_eq!(
            names(&page),
            vec!["ada lovelace", "Alan Turing", "Grace Hopper", "Katherine Johnson"]
        );
        assert_eq!(page.total, 4);
    }

    #[test]
    fn a_search_looks_at_the_name_the_address_and_the_roles() {
        let by_name = paginate(directory(), &PageRequest { search: "turing".to_owned(), ..PageRequest::default() }.sanitised());
        let by_email = paginate(directory(), &PageRequest { search: "kj@".to_owned(), ..PageRequest::default() }.sanitised());
        let by_role = paginate(directory(), &PageRequest { search: "administrator".to_owned(), ..PageRequest::default() }.sanitised());

        assert_eq!(names(&by_name), vec!["Alan Turing"]);
        assert_eq!(names(&by_email), vec!["Katherine Johnson"]);
        assert_eq!(by_role.total, 2, "the role search is a substring, so Administrator matches two");
    }

    #[test]
    fn the_status_filter_narrows_and_is_case_insensitive() {
        let active = paginate(directory(), &PageRequest::default().filtered_by("status", "ACTIVE"));

        assert_eq!(active.total, 2);
        assert!(active.rows.iter().all(|row| matches!(row.status, UserStatusResource::Active)));
    }

    #[test]
    fn a_status_that_does_not_exist_narrows_to_nothing() {
        // The opposite of an unrecognised sort field, and deliberately so: the
        // caller named a value here, and handing back everybody would read as
        // "everybody is retired".
        let page = paginate(directory(), &PageRequest::default().filtered_by("status", "retired"));

        assert_eq!(page.total, 0);
    }

    #[test]
    fn the_role_filter_matches_the_whole_name_rather_than_a_prefix() {
        let page = paginate(directory(), &PageRequest::default().filtered_by("role", "Admin"));

        // Grace alone. "Administrator" starts with "Admin", and a substring
        // match would hand a script two people who hold different roles.
        assert_eq!(names(&page), vec!["Grace Hopper"]);
    }

    #[test]
    fn never_signed_in_sorts_to_the_top() {
        let mut rows = directory();
        rows[1].last_login_at = Some(Utc::now());

        let request = PageRequest { sort: Some(phonix_core::query::Sort::ascending("last_login_at")), ..PageRequest::default() };
        let page = paginate(rows, &request.sanitised());

        // Three nulls first, then the one person who has actually been here.
        assert_eq!(page.rows[3].display_name, "Grace Hopper");
        assert!(page.rows[..3].iter().all(|row| row.last_login_at.is_none()));
    }

    #[test]
    fn a_tie_is_broken_by_id_rather_than_left_to_chance() {
        // Everybody sorts equal on this field, so only the tie-break decides -
        // and it has to decide the same way twice, or paging through the list
        // shows a row on two pages and another on none.
        let rows = directory();
        let request = PageRequest::default().filtered_by("status", "active");
        let request = PageRequest { sort: Some(phonix_core::query::Sort::ascending("status")), ..request };

        let once = paginate(rows.clone(), &request.clone().sanitised());
        let twice = paginate(rows, &request.sanitised());

        assert_eq!(names(&once), names(&twice));
    }

    #[test]
    fn a_page_past_the_end_comes_back_clamped() {
        let request = PageRequest { page: 99, ..PageRequest::first(2) };

        let page = paginate(directory(), &request.sanitised());

        assert_eq!(page.page, 2, "four rows, two per page: the last page is the second");
        assert_eq!(page.rows.len(), 2);
        assert_eq!(page.total, 4, "the total is what matched, not what fitted on the page");
    }
}
