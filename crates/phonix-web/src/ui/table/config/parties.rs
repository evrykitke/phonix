//! The parties grid.
//!
//! # What a phone keeps
//!
//! The name and what the workspace calls them, and the roles. A party list is
//! opened to answer "do we already have these people, and are they a customer
//! or a supplier" - the phone number is why you look at the *row*, not why you
//! scan the list.
//!
//! # The role filter is the point of the screen
//!
//! One table holds customers, suppliers, carriers and agents, which is what
//! lets a company that buys from you and delivers for you be one record. The
//! cost is that the unfiltered list is everybody, so the filter is what makes
//! it usable - and it is a filter rather than four screens, because the answer
//! to "is Acme also a supplier" should be one click, not a second search.

use leptos::prelude::*;
use phonix_core::permissions;
use phonix_core::query::Sort;
use phonix_master::party::{PartyKind, PartyRole, PartySummary, roles};

use super::GridConfig;
use crate::components::page::{Badge, Tone};
use crate::icons::Icon;
use crate::l;
use crate::server_fns::master_fns::{delete_party, list_parties};
use crate::ui::table::{Cell, Column, Filter, FilterChoice, RowAction, Source, ToolbarAction};

/// Everyone this workspace trades with.
pub fn parties_grid() -> GridConfig<PartySummary> {
    GridConfig::new(
        "parties",
        // `None`: the master-data screen is about every party, and the role
        // filter narrows it in the browser. An app wanting only its own passes
        // its role to the same endpoint.
        Source::in_memory(|| list_parties(None)),
    )
    .searching(l!("parties.search"))
    .exports_as("parties")
    .sorted_by(Sort::ascending("name"))
    .min_width("sm:min-w-[52rem]")
    .empty(
        Icon::Users,
        l!("parties.empty.title"),
        l!("parties.empty.detail"),
    )
    .column(
        Column::new("name", l!("field.name"), |party: &PartySummary| {
            Cell::text(&party.name)
        })
        .findable()
        // Without its name a row is a code and a country.
        .pinned()
        .essential()
        .render(|party| name_cell(party).into_any()),
    )
    .column(
        Column::new("code", l!("field.code"), |party: &PartySummary| {
            Cell::text(&party.code)
        })
        .findable()
        // Already under the name; here so it can be sorted and exported on its
        // own.
        .hidden(),
    )
    .column(
        Column::new("roles", l!("parties.roles"), |party: &PartySummary| {
            Cell::list(party.roles.iter().map(PartyRole::as_str))
        })
        .searchable()
        // The second question the screen is opened to answer, after "do we
        // have them at all".
        .essential()
        .render(|party| roles_cell(party).into_any()),
    )
    .column(
        Column::new("kind", l!("field.kind"), |party: &PartySummary| {
            Cell::text(party.kind.as_str())
        })
        .sortable()
        .hidden(),
    )
    .column(
        Column::new("email", l!("field.email"), |party: &PartySummary| {
            Cell::maybe(party.email.clone())
        })
        .searchable()
        .class("text-xs text-content-muted"),
    )
    .column(
        Column::new("phone", l!("field.phone"), |party: &PartySummary| {
            Cell::maybe(party.phone.clone())
        })
        .searchable()
        .class("whitespace-nowrap text-xs text-content-muted"),
    )
    .column(
        Column::new("country", l!("field.country"), |party: &PartySummary| {
            Cell::maybe(party.country.map(|country| country.name().to_owned()))
        })
        .findable(),
    )
    .column(
        Column::new("currency", l!("field.currency"), |party: &PartySummary| {
            Cell::maybe(party.currency.map(|currency| currency.code().to_owned()))
        })
        .sortable()
        .hidden(),
    )
    .column(
        Column::new("is_active", l!("field.status"), |party: &PartySummary| {
            Cell::bool(party.is_active)
        })
        .sortable()
        .render(|party| status_cell(party).into_any()),
    )
    // Named predicates, answered where the rows are. See
    // `phonix_web::ui::table::filter`.
    .filter(
        Filter::new(
            "role",
            l!("parties.roles"),
            vec![
                FilterChoice::all(l!("common.all")),
                FilterChoice::new(roles::CUSTOMER, l!("parties.role.customer")),
                FilterChoice::new(roles::SUPPLIER, l!("parties.role.supplier")),
                FilterChoice::new(roles::CARRIER, l!("parties.role.carrier")),
                FilterChoice::new(roles::AGENT, l!("parties.role.agent")),
            ],
        )
        .matching(|party: &PartySummary, wanted| {
            party.roles.iter().any(|role| role.as_str() == wanted)
        }),
    )
    .filter(
        Filter::new(
            "status",
            l!("field.status"),
            vec![
                FilterChoice::all(l!("common.all")),
                FilterChoice::new("active", l!("common.active")),
                FilterChoice::new("inactive", l!("common.inactive")),
            ],
        )
        .matching(|party: &PartySummary, wanted| match wanted {
            "active" => party.is_active,
            "inactive" => !party.is_active,
            _ => true,
        }),
    )
    .toolbar(
        ToolbarAction::link(l!("parties.new"), Icon::Plus, "/master/parties/new")
            .require(permissions::PARTIES_CREATE)
            .primary(),
    )
    .action(
        RowAction::link(l!("common.open"), Icon::Eye, |party: &PartySummary| {
            format!("/master/parties/{}", party.id)
        })
        .require(permissions::PARTIES),
    )
    .action(
        RowAction::link(l!("common.edit"), Icon::Pencil, |party: &PartySummary| {
            format!("/master/parties/{}?tab=details", party.id)
        })
        .require(permissions::PARTIES_EDIT),
    )
    .action(
        RowAction::run(
            l!("common.delete"),
            Icon::Trash2,
            |party: PartySummary, grid| {
                let label = party.name.clone();

                leptos::task::spawn_local(async move {
                    match delete_party(party.id).await {
                        Ok(()) => {
                            grid.report(l!("parties.deleted", name = label));
                            grid.refresh();
                        }
                        // The server's own words: it knows whether this was
                        // refused for being in use or for permission, and
                        // either is worth reading.
                        Err(err) => grid.warn(err.to_string()),
                    }
                });
            },
        )
        // Offered only where it would do something. A party any app has claimed
        // is refused by the service, so a button for one is a button that only
        // ever produces an error.
        .when(|party: &PartySummary| party.roles.is_empty())
        .require(permissions::PARTIES_DELETE)
        .tone(Tone::Danger)
        .confirm(l!("parties.delete.confirm")),
    )
}

/// The name, what kind of party it is, and the code documents refer to it by.
fn name_cell(party: &PartySummary) -> impl IntoView {
    let name = party.name.clone();
    let code = party.code.clone();
    let is_person = party.kind == PartyKind::Person;

    view! {
        <div class="min-w-0">
            <div class="flex flex-wrap items-center gap-1.5">
                <span class="truncate-fade font-medium text-content">{name}</span>
                {is_person
                    .then(|| view! { <Badge label=l!("party.kind.person") /> })}
            </div>
            <code class="text-2xs text-content-subtle">{code}</code>
        </div>
    }
}

/// What the apps have claimed about this party.
///
/// "Not in use" rather than an empty cell: a party nobody has claimed is a row
/// somebody created and never used, which is a fact rather than missing data -
/// and it is the only kind that can be deleted.
fn roles_cell(party: &PartySummary) -> impl IntoView {
    let held: Vec<String> = party
        .roles
        .iter()
        .map(|role| role.as_str().to_owned())
        .collect();

    view! {
        <div class="flex flex-wrap items-center gap-1">
            {if held.is_empty() {
                view! {
                    <span class="text-xs text-content-subtle">{l!("parties.role.none")}</span>
                }
                    .into_any()
            } else {
                held.into_iter()
                    .map(|role| view! { <Badge label=role tone=Tone::Brand /> })
                    .collect_view()
                    .into_any()
            }}
        </div>
    }
}

fn status_cell(party: &PartySummary) -> impl IntoView {
    let is_active = party.is_active;

    view! {
        {if is_active {
            view! { <Badge label=l!("common.active") tone=Tone::Success /> }.into_any()
        } else {
            view! { <Badge label=l!("common.inactive") /> }.into_any()
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phonix_core::query::PageRequest;
    use uuid::Uuid;

    fn grid() -> GridConfig<PartySummary> {
        Owner::new().with(parties_grid)
    }

    fn party(name: &str, role: Option<&str>, is_active: bool) -> PartySummary {
        PartySummary {
            id: Uuid::nil(),
            code: name.to_uppercase(),
            kind: PartyKind::Organization,
            name: name.to_owned(),
            country: None,
            email: None,
            phone: None,
            currency: None,
            is_active,
            roles: role
                .and_then(|role| PartyRole::parse(role).ok())
                .into_iter()
                .collect(),
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

        assert!(searchable.contains(&"name"));
        assert!(searchable.contains(&"code"));
        assert!(searchable.contains(&"email"));
    }

    #[test]
    fn the_column_that_identifies_a_row_cannot_be_hidden() {
        let grid = grid();
        let label = grid.columns.iter().find(|c| c.field() == "name").unwrap();

        assert!(!label.hideable);
    }

    #[test]
    fn a_phone_keeps_the_name_and_what_the_apps_claim_about_it() {
        // Not more: three columns is what 390 pixels holds before the table
        // starts scrolling sideways and taking the page with it.
        let grid = grid();
        let essential: Vec<&str> = grid
            .columns
            .iter()
            .filter(|c| c.essential)
            .map(|c| c.field())
            .collect();

        assert_eq!(essential, vec!["name", "roles"]);
    }

    #[test]
    fn the_role_filter_finds_a_party_wearing_that_hat() {
        // The filter is the point of the screen: one table holds customers,
        // suppliers and carriers, so the unfiltered list is everybody.
        let grid = grid();
        let filter = grid.filters.iter().find(|f| f.key() == "role").unwrap();
        let acme = party("Acme", Some(roles::CUSTOMER), true);

        let asking = |role: &str| PageRequest::first(25).filtered_by("role", role);
        assert!(filter.accepts(&acme, &asking(roles::CUSTOMER)));
        assert!(!filter.accepts(&acme, &asking(roles::SUPPLIER)));

        // Nothing chosen is everybody, which is what the "All" option means.
        assert!(filter.accepts(&acme, &PageRequest::first(25)));
    }

    #[test]
    fn a_party_an_app_has_claimed_is_not_offered_a_delete_button() {
        // The service refuses it, so the button could only ever produce an
        // error message.
        let grid = grid();
        let delete = grid.actions.iter().find(|a| a.label == "Delete").unwrap();

        assert!(!delete.applies_to(&party("Acme", Some(roles::CUSTOMER), true)));
        assert!(delete.applies_to(&party("Unused", None, true)));
    }

    #[test]
    fn every_action_names_a_permission() {
        let grid = grid();

        for action in &grid.actions {
            assert!(action.permission.is_some(), "{} is ungated", action.label);
        }
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
