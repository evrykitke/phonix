//! The roles grid.
//!
//! # Why this stopped being a wall of cards
//!
//! Roles were drawn as a two-column card grid. It looked fine with two roles
//! and stopped answering questions at six: cards cannot be sorted, cannot be
//! searched, and put the two numbers that matter - how much a role grants and
//! how many people hold it - in small grey text at the bottom of a box.
//!
//! A grid answers "which role has the most people in it", "which one grants
//! nothing", and "what is the one called something like audit" without the
//! reader doing the scanning. It is also the standard here: every list in this
//! application is a [`GridConfig`], and roles were the one that was not.

use leptos::prelude::*;
use phonix_core::authorization::RoleSummary;
use phonix_core::permissions;
use phonix_core::query::Sort;

use super::GridConfig;
use crate::components::page::{Badge, Tone};
use crate::icons::Icon;
use crate::l;
use crate::server_fns::admin_fns::{delete_role, list_roles};
use crate::ui::table::{Align, Cell, Column, RowAction, Source, ToolbarAction};

/// Every role this workspace has defined.
pub fn roles_grid() -> GridConfig<RoleSummary> {
    GridConfig::new("roles", Source::in_memory(list_roles))
        .searching(l!("roles.search"))
        .exports_as("roles")
        // The built-in ones first, then alphabetically - the same order the
        // database returns, so the grid opens looking like what it fetched.
        .sorted_by(Sort::ascending("display_name"))
        .min_width("sm:min-w-[44rem]")
        .empty(
            Icon::ShieldCheck,
            l!("roles.empty.title"),
            l!("roles.empty.detail"),
        )
        .column(
            Column::new(
                "display_name",
                l!("entity.role.singular"),
                |role: &RoleSummary| Cell::text(&role.display_name),
            )
            .findable()
            // Without its name a row is two numbers and a badge.
            .pinned()
            .essential()
            .render(|role| role_cell(role).into_any()),
        )
        .column(
            Column::new("name", l!("field.key"), |role: &RoleSummary| {
                Cell::text(&role.name)
            })
            .findable()
            // Already under the label in the role column; here so it can be
            // sorted and exported on its own.
            .hidden(),
        )
        .column(
            Column::new(
                "description",
                l!("field.description"),
                |role: &RoleSummary| Cell::maybe(role.description.clone()),
            )
            .searchable()
            .class("text-xs text-content-muted"),
        )
        .column(
            Column::new(
                "permission_count",
                l!("field.grants"),
                |role: &RoleSummary| Cell::number(role.permission_count as f64),
            )
            .sortable()
            .align(Align::End)
            // The question this screen is opened to answer: what does this
            // role actually let somebody do.
            .essential()
            .render(|role| grants_cell(role).into_any()),
        )
        .column(
            Column::new("user_count", l!("field.people"), |role: &RoleSummary| {
                Cell::number(role.user_count as f64)
            })
            .sortable()
            .align(Align::End)
            .class("whitespace-nowrap text-content-muted"),
        )
        .column(
            Column::new("is_default", l!("roles.default"), |role: &RoleSummary| {
                Cell::bool(role.is_default)
            })
            .sortable()
            .align(Align::Center)
            .hidden(),
        )
        .toolbar(
            ToolbarAction::link(l!("roles.new"), Icon::Plus, "/admin/roles/new")
                .require(permissions::ROLES_CREATE)
                .primary(),
        )
        .action(
            RowAction::link(
                l!("permissions.title"),
                Icon::KeySquare,
                |role: &RoleSummary| format!("/admin/roles/{}?tab=permissions", role.id),
            )
            .require(permissions::ROLES),
        )
        .action(
            RowAction::link(l!("common.edit"), Icon::Pencil, |role: &RoleSummary| {
                format!("/admin/roles/{}?tab=details", role.id)
            })
            .require(permissions::ROLES_EDIT),
        )
        .action(
            RowAction::run(
                l!("common.delete"),
                Icon::Trash2,
                |role: RoleSummary, grid| {
                    let label = role.display_name.clone();

                    leptos::task::spawn_local(async move {
                        match delete_role(role.id).await {
                            Ok(()) => {
                                grid.report(l!("roles.deleted", name = label));
                                grid.refresh();
                            }
                            // The server's own words: it knows whether this was
                            // refused for being built in or for permission, and
                            // either is worth reading.
                            Err(err) => grid.warn(err.to_string()),
                        }
                    });
                },
            )
            // Offered only where it would do something. `Admin` and `User`
            // are refused by the service and by the statement itself, so a
            // button for them is a button that only ever produces an error.
            .when(|role: &RoleSummary| !role.is_static)
            .require(permissions::ROLES_DELETE)
            .tone(Tone::Danger)
            .confirm(l!("roles.delete.confirm")),
        )
}

/// The label, what kind of role it is, and the key code refers to it by.
fn role_cell(role: &RoleSummary) -> impl IntoView {
    let display_name = role.display_name.clone();
    let name = role.name.clone();
    let is_static = role.is_static;
    let is_default = role.is_default;
    // Only worth pointing out when the two differ: repeating "Auditor" under
    // "Auditor" is a row that says the same thing twice.
    let show_key = role.name != role.display_name;

    view! {
        <div class="min-w-0">
            <div class="flex flex-wrap items-center gap-1.5">
                <span class="truncate-fade font-medium text-content">{display_name}</span>
                {is_static
                    .then(|| view! { <Badge label=l!("roles.built_in") tone=Tone::Brand /> })}
                {is_default.then(|| view! { <Badge label=l!("roles.default") /> })}
            </div>
            {show_key
                .then(|| {
                    view! { <code class="text-2xs text-content-subtle">{name}</code> }
                })}
        </div>
    }
}

/// How much of the tree this role grants, said in words as well as a number.
///
/// "0" on its own reads as missing data. "Nothing yet" reads as a role somebody
/// has not finished, which is what it is.
fn grants_cell(role: &RoleSummary) -> impl IntoView {
    let count = role.permission_count;
    let is_static = role.is_static;

    view! {
        <span class="whitespace-nowrap">
            {if count == 0 {
                view! {
                    <span class="text-xs text-content-subtle">{l!("roles.grants.none")}</span>
                }
                    .into_any()
            } else if is_static {
                view! {
                    <span class="text-content-muted">
                        {crate::lp!("roles.grants.count", count)}
                    </span>
                }
                    .into_any()
            } else {
                view! {
                    <span class="text-content">
                        {crate::lp!("roles.grants.count", count)}
                    </span>
                }
                    .into_any()
            }}
        </span>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> GridConfig<RoleSummary> {
        Owner::new().with(roles_grid)
    }

    fn role(name: &str, is_static: bool) -> RoleSummary {
        RoleSummary {
            id: uuid::Uuid::nil(),
            name: name.to_owned(),
            display_name: name.to_owned(),
            description: None,
            is_static,
            is_default: false,
            permission_count: 3,
            user_count: 1,
        }
    }

    #[test]
    fn the_search_box_looks_in_the_columns_the_placeholder_names() {
        let grid = grid();
        let searchable: Vec<&str> = grid
            .columns
            .iter()
            .filter(|c| c.searchable)
            .map(|c| c.field())
            .collect();

        // "Filter by name or description".
        assert!(searchable.contains(&"display_name"));
        assert!(searchable.contains(&"name"));
        assert!(searchable.contains(&"description"));
    }

    #[test]
    fn the_column_that_identifies_a_row_cannot_be_hidden() {
        let grid = grid();
        let label = grid
            .columns
            .iter()
            .find(|c| c.field() == "display_name")
            .unwrap();

        assert!(!label.hideable);
    }

    #[test]
    fn every_action_names_a_permission() {
        let grid = grid();

        for action in &grid.actions {
            assert!(action.permission.is_some(), "{} is ungated", action.label);
        }
    }

    #[test]
    fn a_built_in_role_is_not_offered_a_delete_button() {
        // The service refuses it and so does the statement, so the button
        // could only ever produce an error message.
        let grid = grid();
        let delete = grid.actions.iter().find(|a| a.label == "Delete").unwrap();

        assert!(!delete.applies_to(&role("Admin", true)));
        assert!(delete.applies_to(&role("Auditor", false)));
    }

    #[test]
    fn the_destructive_action_asks_first_and_says_who_it_reaches() {
        let grid = grid();
        let delete = grid.actions.iter().find(|a| a.label == "Delete").unwrap();

        assert_eq!(delete.tone, Tone::Danger);
        assert!(
            delete
                .confirm
                .as_deref()
                .is_some_and(|question| question.contains("holding it"))
        );
    }

    #[test]
    fn the_column_it_opens_sorted_by_is_one_that_can_be_sorted() {
        let grid = grid();
        let sort = grid.initial_request().sort.unwrap();

        assert!(
            grid.columns
                .iter()
                .any(|c| c.sortable && c.field() == sort.field),
            "opens sorted by a column that does not sort",
        );
    }
}
