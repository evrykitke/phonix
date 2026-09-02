//! `/api/v1/permissions` - the tree every scope and every grant is named from.
//!
//! # Why this is here at all
//!
//! ADR 0002 §3 settles that **a scope is a permission name**, not a second
//! vocabulary. That decision is only usable if a client can find out what the
//! names are. `GET /roles/{id}` answers "what does this role confer", which is
//! a different question: it names the handful a workspace happens to have
//! ticked, and says nothing about the ones nobody has used yet. Somebody
//! building a narrowly-scoped key needs the whole list, with the shape.
//!
//! # It is compiled in, so it is not workspace data
//!
//! `phonix_core::authorization::DEFINITIONS` is a `const` table: the same
//! bytes in every workspace, describing the software rather than a tenant.
//! So this read is **ungated** - a key with no scopes at all reads it, exactly
//! as it reads the currency list. Requiring `Roles` to find out what a scope
//! may say would mean a key can only be narrowed by somebody who already holds
//! the administration area.
//!
//! What it is *not* is unauthenticated. The specification and the
//! documentation page describe the product to anybody; this describes what
//! this build can do, and a credential is a low enough bar to keep it behind.
//!
//! # The parent is the contract, the depth is a convenience
//!
//! A scope covers everything beneath it, so a client narrowing a key has to
//! know which names hang under which. `parent` is that relationship stated
//! once. `depth` is derivable from the dots in the name and is carried anyway,
//! because a client rendering the tree would otherwise count them itself and
//! we would have two answers to one question.

use axum::Json;
use phonix_core::authorization::{DEFINITIONS, PermissionDefinition};
use phonix_core::query::{Page, PageRequest};
use utoipa::ToSchema;

use super::auth::ApiCaller;
use super::paging::{ListParams, ListRequest, PageEnvelope, cut};
use super::problem::Problem;

/// One node of the permission tree.
#[derive(Debug, Clone, serde::Serialize, ToSchema)]
#[schema(as = Permission)]
pub struct PermissionResource {
    /// Dotted, stable, and the exact string an API key's `scopes` may carry or
    /// a role's `permissions` may contain. Renaming one is a data migration on
    /// our side, never a release.
    #[schema(example = "Pages.Administration.Users.Create")]
    pub name: String,
    /// What an editor shows. Free to be reworded; never branch on it.
    #[schema(example = "Create")]
    pub display_name: String,
    pub description: Option<String>,
    /// The name one level up, or `null` for a root. Holding a parent implies
    /// holding everything beneath it, which is what makes this the field a
    /// client scoping a key actually reads.
    #[schema(example = "Pages.Administration.Users")]
    pub parent: Option<String>,
    /// How deep, with a root at 0. The dots in `name` say the same thing; this
    /// is here so a client rendering the tree does not have to count them.
    #[schema(example = 3)]
    pub depth: u32,
    /// Granted to the static `User` role in every new workspace. Says what the
    /// product considers ordinary, not what anybody here holds.
    pub default_for_user: bool,
}

impl From<&PermissionDefinition> for PermissionResource {
    fn from(node: &PermissionDefinition) -> Self {
        Self {
            name: node.name.to_owned(),
            display_name: node.display_name.to_owned(),
            description: node.description.map(str::to_owned),
            parent: node.parent.map(str::to_owned),
            // `usize` on the way in and never anywhere near `u32::MAX`: the
            // deepest name in the tree has four segments.
            depth: u32::try_from(node.depth()).unwrap_or(0),
            default_for_user: node.default_for_user,
        }
    }
}

/// Every permission this build defines.
///
/// Searches the name, the display name and the description. Sorts by `name` or
/// `display_name`; the default is **declaration order**, which is the tree
/// depth-first - parents before their children, siblings together - and is the
/// order an editor renders. Narrows on `filter[parent]`, which is an exact
/// name and answers the children of one node, and on `filter[root]=true`,
/// which answers the nodes that have no parent at all.
#[utoipa::path(
    get,
    path = "/permissions",
    tag = "permissions",
    operation_id = "listPermissions",
    params(
        ListParams,
        ("filter[parent]" = Option<String>, Query,
            description = "The children of exactly this node, one level down.",
            example = "Pages.Administration"),
        ("filter[root]" = Option<String>, Query,
            description = "`true` for the nodes with no parent.",
            example = "true"),
    ),
    responses(
        (status = 200, description = "One page of the permission tree", body = PageEnvelope<PermissionResource>),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The workspace has no API access", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn list(
    // Taken and unused: this read is ungated, but it is not public. The
    // extractor is the whole of the check, and naming it `_caller` would read
    // as an oversight rather than as the decision it is.
    _caller: ApiCaller,
    ListRequest(request): ListRequest,
) -> Result<Json<PageEnvelope<PermissionResource>>, Problem> {
    Ok(Json(PageEnvelope::new(paginate(&request))))
}

/// Search, narrow, sort and cut one page.
fn paginate(request: &PageRequest) -> Page<PermissionResource> {
    let needle = request.needle();

    let mut matching: Vec<(usize, &PermissionDefinition)> = DEFINITIONS
        .iter()
        // The index is carried so the default sort can be "as declared" -
        // there is no column that reproduces depth-first order, and sorting by
        // name would interleave `Pages.Administration` with `Pages.Files`.
        .enumerate()
        .filter(|(_, node)| match &needle {
            Some(needle) => {
                node.name.to_lowercase().contains(needle)
                    || node.display_name.to_lowercase().contains(needle)
                    || node
                        .description
                        .is_some_and(|text| text.to_lowercase().contains(needle))
            }
            None => true,
        })
        // Exact rather than a prefix match: `parent` means one level down. A
        // prefix would answer the whole subtree, which is a different question
        // and one the caller can ask by searching for the name.
        .filter(|(_, node)| match request.filter("parent") {
            Some(parent) => node.parent == Some(parent),
            None => true,
        })
        .filter(|(_, node)| match request.filter("root") {
            // Two-valued, so anything that is not `true` narrows the other way
            // rather than being refused - the same rule `roles` follows.
            Some(flag) => node.parent.is_none() == flag.eq_ignore_ascii_case("true"),
            None => true,
        })
        .collect();

    let descending = request
        .sort
        .as_ref()
        .is_some_and(|sort| !sort.direction.is_ascending());

    // The name is the tie-break on every arm, and it is unique by construction
    // - a test in `phonix_core::authorization` refuses a repeat - so no two
    // rows can compare equal and paging cannot show one twice.
    match request.sort.as_ref().map(|sort| sort.field.as_str()) {
        Some("name") => matching.sort_by_key(|(_, node)| node.name),
        Some("display_name") => matching.sort_by(|(_, a), (_, b)| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
                .then_with(|| a.name.cmp(b.name))
        }),
        // Declaration order, which is the tree. Already unique, already total.
        _ => matching.sort_by_key(|(index, _)| *index),
    }
    if descending {
        matching.reverse();
    }

    cut(matching, request, |(_, node)| {
        PermissionResource::from(node)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(page: &Page<PermissionResource>) -> Vec<String> {
        page.rows.iter().map(|row| row.name.clone()).collect()
    }

    #[test]
    fn the_default_page_is_the_tree_in_declaration_order() {
        let page = paginate(&PageRequest::first(500));

        assert_eq!(page.total as usize, DEFINITIONS.len());
        // Depth-first, so the first row is a root and every child follows a
        // parent that has already appeared.
        assert_eq!(page.rows[0].parent, None);
        for row in &page.rows {
            if let Some(parent) = &row.parent {
                assert!(
                    page.rows
                        .iter()
                        .take_while(|earlier| earlier.name != row.name)
                        .any(|earlier| &earlier.name == parent),
                    "{} appears before its parent {parent}",
                    row.name
                );
            }
        }
    }

    #[test]
    fn the_parent_filter_answers_one_level_rather_than_a_subtree() {
        let page = paginate(
            &PageRequest::first(500).filtered_by("parent", phonix_core::permissions::USERS),
        );

        assert!(!page.rows.is_empty(), "Users has children");
        // Every row is a direct child. A prefix match would have dragged in
        // grandchildren, and a client reading this as "one level" would then
        // render the tree twice over.
        assert!(
            page.rows
                .iter()
                .all(|row| row.parent.as_deref() == Some(phonix_core::permissions::USERS))
        );
    }

    #[test]
    fn the_root_filter_narrows_both_ways() {
        let roots = paginate(&PageRequest::first(500).filtered_by("root", "true"));
        let rest = paginate(&PageRequest::first(500).filtered_by("root", "false"));

        assert!(roots.rows.iter().all(|row| row.parent.is_none()));
        assert!(rest.rows.iter().all(|row| row.parent.is_some()));
        assert_eq!(roots.total + rest.total, DEFINITIONS.len() as u64);
    }

    #[test]
    fn every_name_a_scope_may_use_is_reachable() {
        // The point of the endpoint: a client that reads every page can build
        // any scope the service would accept, without reading it off a screen.
        let page = paginate(&PageRequest::first(500));

        assert!(names(&page).contains(&phonix_core::permissions::SETTINGS.to_owned()));
        assert!(names(&page).contains(&phonix_core::permissions::API_KEYS_CREATE.to_owned()));
    }

    #[test]
    fn the_depth_agrees_with_the_dots_in_the_name() {
        // Two answers to one question is how they come to disagree, so the
        // carried one has to be the derived one.
        let page = paginate(&PageRequest::first(500));

        for row in &page.rows {
            assert_eq!(row.depth as usize, row.name.matches('.').count());
        }
    }
}
