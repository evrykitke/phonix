//! The taxes grid, and the tax groups grid.
//!
//! Two grids in one file because they are two halves of one screen: a tax is
//! what is charged, a group is what a document line points at, and neither
//! makes sense on its own. They live on the same page under two tabs.
//!
//! # Why the rate is on the row at all
//!
//! A tax code deliberately has no rate column - a code outlives its rates. But
//! a list of taxes with no rates on it answers none of the questions somebody
//! opens the screen with, so the rate *in force today* is resolved on the
//! server and carried alongside. A code with no rate says so, loudly: it
//! refuses every document that references it.

use leptos::prelude::*;
use phonix_core::permissions;
use phonix_core::query::Sort;
use phonix_tax::code::{TaxCodeSummary, TaxKind};
use phonix_tax::group::TaxGroup;

use super::GridConfig;
use crate::components::page::{Badge, Tone};
use crate::icons::Icon;
use crate::l;
use crate::server_fns::master_fns::{
    delete_tax_code, delete_tax_group, list_tax_codes_today, list_tax_groups,
};
use crate::ui::table::{
    Align, Cell, Column, Filter, FilterChoice, RowAction, Source, ToolbarAction,
};

/// Every tax this workspace charges.
pub fn taxes_grid() -> GridConfig<TaxCodeSummary> {
    GridConfig::new("taxes", Source::in_memory(list_tax_codes_today))
        .searching(l!("taxes.search"))
        .exports_as("taxes")
        .sorted_by(Sort::ascending("code"))
        .min_width("sm:min-w-[46rem]")
        .empty(
            Icon::Receipt,
            l!("taxes.empty.title"),
            l!("taxes.empty.detail"),
        )
        .column(
            Column::new("code", l!("field.code"), |tax: &TaxCodeSummary| {
                Cell::text(&tax.code.code)
            })
            .findable()
            // Without its code a row is a rate and a country.
            .pinned()
            .essential()
            .render(|tax| code_cell(tax).into_any()),
        )
        .column(
            Column::new("name", l!("field.name"), |tax: &TaxCodeSummary| {
                Cell::text(&tax.code.name)
            })
            .findable()
            // Already under the code; here so it can be sorted and exported.
            .hidden(),
        )
        .column(
            Column::new(
                "rate_today",
                l!("taxes.current_rate"),
                |tax: &TaxCodeSummary| match tax.rate_today {
                    // The *proportion*, so sorting is numeric. What is drawn is
                    // the percentage - see `render` below, and the note on
                    // `Column::render` about why the two can differ.
                    Some(rate) => Cell::number(rate.scaled() as f64),
                    None => Cell::text(""),
                },
            )
            .sortable()
            .align(Align::End)
            // The question the screen is opened to answer.
            .essential()
            .render(|tax| rate_cell(tax).into_any()),
        )
        .column(
            Column::new("kind", l!("field.kind"), |tax: &TaxCodeSummary| {
                Cell::text(tax.code.kind.as_str())
            })
            .sortable()
            .render(|tax| {
                let kind = crate::i18n::t(&tax.code.kind.label());
                view! { <Badge label=kind /> }.into_any()
            }),
        )
        .column(
            Column::new("country", l!("field.country"), |tax: &TaxCodeSummary| {
                Cell::maybe(tax.code.country.map(|c| c.name().to_owned()))
            })
            .findable(),
        )
        .column(
            Column::new("region", l!("field.region"), |tax: &TaxCodeSummary| {
                Cell::maybe(tax.code.region_code.clone())
            })
            .searchable()
            .hidden(),
        )
        .column(
            Column::new("flags", l!("field.detail"), |tax: &TaxCodeSummary| {
                Cell::list(flag_words(&tax.code))
            })
            .render(|tax| flags_cell(tax).into_any()),
        )
        .column(
            Column::new("is_active", l!("field.status"), |tax: &TaxCodeSummary| {
                Cell::bool(tax.code.is_active)
            })
            .sortable()
            .render(|tax| active_badge(tax.code.is_active).into_any()),
        )
        .filter(
            Filter::new("kind", l!("field.kind"), kind_choices())
                .matching(|tax: &TaxCodeSummary, wanted| tax.code.kind.as_str() == wanted),
        )
        .filter(
            Filter::new(
                "status",
                l!("field.status"),
                vec![
                    FilterChoice::all(l!("common.all")),
                    FilterChoice::new("active", l!("common.active")),
                    FilterChoice::new("inactive", l!("common.inactive")),
                    // Worth its own filter rather than being read off the rate
                    // column: a tax with no rate is a tax that refuses every
                    // document, and finding all of them is the first thing to
                    // do after setting a workspace up.
                    FilterChoice::new("unpriced", l!("taxes.no_rate")),
                ],
            )
            .matching(|tax: &TaxCodeSummary, wanted| match wanted {
                "active" => tax.code.is_active,
                "inactive" => !tax.code.is_active,
                "unpriced" => tax.rate_today.is_none(),
                _ => true,
            }),
        )
        .toolbar(
            ToolbarAction::link(l!("taxes.new"), Icon::Plus, "/master/taxes/new")
                .require(permissions::TAXES_EDIT)
                .primary(),
        )
        .action(
            RowAction::link(l!("common.open"), Icon::Eye, |tax: &TaxCodeSummary| {
                format!("/master/taxes/{}", tax.code.id)
            })
            .require(permissions::TAXES),
        )
        .action(
            RowAction::run(
                l!("common.delete"),
                Icon::Trash2,
                |tax: TaxCodeSummary, grid| {
                    let label = tax.code.name.clone();

                    leptos::task::spawn_local(async move {
                        match delete_tax_code(tax.code.id).await {
                            Ok(()) => {
                                grid.report(l!("taxes.deleted", name = label));
                                grid.refresh();
                            }
                            // The server names the group that is holding it,
                            // which is the only useful answer here.
                            Err(err) => grid.warn(err.to_string()),
                        }
                    });
                },
            )
            .require(permissions::TAXES_EDIT)
            .tone(Tone::Danger)
            .confirm(l!("taxes.delete.confirm")),
        )
}

/// Every tax group: what a document line can point at.
pub fn tax_groups_grid() -> GridConfig<TaxGroup> {
    GridConfig::new("tax-groups", Source::in_memory(list_tax_groups))
        .searching(l!("tax_groups.search"))
        .exports_as("tax-groups")
        .sorted_by(Sort::ascending("code"))
        .min_width("sm:min-w-[40rem]")
        .empty(
            Icon::ListTree,
            l!("tax_groups.empty.title"),
            l!("tax_groups.empty.detail"),
        )
        .column(
            Column::new("code", l!("field.code"), |group: &TaxGroup| {
                Cell::text(&group.code)
            })
            .findable()
            .pinned()
            .essential()
            .render(|group| group_cell(group).into_any()),
        )
        .column(
            Column::new("name", l!("field.name"), |group: &TaxGroup| {
                Cell::text(&group.name)
            })
            .findable()
            .hidden(),
        )
        .column(
            Column::new("members", l!("tax_groups.members"), |group: &TaxGroup| {
                Cell::list(group.members.iter().map(|member| member.code.clone()))
            })
            .searchable()
            // What the group *is*. A group without its members listed is a
            // name and nothing else.
            .essential()
            .render(|group| members_cell(group).into_any()),
        )
        .column(
            Column::new("country", l!("field.country"), |group: &TaxGroup| {
                Cell::maybe(group.country.map(|c| c.name().to_owned()))
            })
            .findable(),
        )
        .column(
            Column::new("is_active", l!("field.status"), |group: &TaxGroup| {
                Cell::bool(group.is_active)
            })
            .sortable()
            .render(|group| active_badge(group.is_active).into_any()),
        )
        .toolbar(
            ToolbarAction::link(l!("tax_groups.new"), Icon::Plus, "/master/tax-groups/new")
                .require(permissions::TAXES_EDIT)
                .primary(),
        )
        .action(
            RowAction::link(l!("common.edit"), Icon::Pencil, |group: &TaxGroup| {
                format!("/master/tax-groups/{}", group.id)
            })
            .require(permissions::TAXES_EDIT),
        )
        .action(
            RowAction::run(
                l!("common.delete"),
                Icon::Trash2,
                |group: TaxGroup, grid| {
                    let label = group.name.clone();

                    leptos::task::spawn_local(async move {
                        match delete_tax_group(group.id).await {
                            Ok(()) => {
                                grid.report(l!("tax_groups.deleted", name = label));
                                grid.refresh();
                            }
                            Err(err) => grid.warn(err.to_string()),
                        }
                    });
                },
            )
            .require(permissions::TAXES_EDIT)
            .tone(Tone::Danger)
            .confirm(l!("tax_groups.delete.confirm")),
        )
}

fn kind_choices() -> Vec<FilterChoice> {
    std::iter::once(FilterChoice::all(l!("common.all")))
        .chain(
            TaxKind::ALL
                .iter()
                .map(|kind| FilterChoice::new(kind.as_str(), crate::i18n::t(&kind.label()))),
        )
        .collect()
}

/// The code, with the name under it.
fn code_cell(tax: &TaxCodeSummary) -> impl IntoView {
    let code = tax.code.code.clone();
    let name = tax.code.name.clone();

    view! {
        <div class="min-w-0">
            <span class="font-medium text-content">{code}</span>
            <div class="truncate-fade text-2xs text-content-subtle">{name}</div>
        </div>
    }
}

/// What it is charged at today, or a warning that it is charged at nothing.
///
/// "No rate set" rather than a blank or a zero. A zero would be a lie - a
/// zero-rated tax is a real thing and this is not one - and a blank reads as
/// missing data rather than as a tax that refuses every document.
fn rate_cell(tax: &TaxCodeSummary) -> impl IntoView {
    let rate = tax.rate_today.map(|rate| rate.to_percent_string());

    view! {
        <span class="whitespace-nowrap">
            {match rate {
                Some(rate) => {
                    view! { <span class="tabular-nums text-content">{rate}</span> }.into_any()
                }
                None => {
                    view! {
                        <span class="text-xs text-warning">{l!("taxes.no_rate")}</span>
                    }
                        .into_any()
                }
            }}
        </span>
    }
}

/// The two flags that are not cosmetic, said in words.
fn flag_words(code: &phonix_tax::code::TaxCode) -> Vec<String> {
    let mut words = Vec::with_capacity(2);
    if code.is_compound {
        words.push(l!("taxes.compound"));
    }
    if code.is_recoverable {
        words.push(l!("taxes.recoverable"));
    }
    words
}

fn flags_cell(tax: &TaxCodeSummary) -> impl IntoView {
    let compound = tax.code.is_compound;
    let recoverable = tax.code.is_recoverable;

    view! {
        <div class="flex flex-wrap items-center gap-1">
            {compound
                .then(|| view! { <Badge label=l!("taxes.compound") tone=Tone::Warning /> })}
            {recoverable.then(|| view! { <Badge label=l!("taxes.recoverable") /> })}
        </div>
    }
}

fn group_cell(group: &TaxGroup) -> impl IntoView {
    let code = group.code.clone();
    let name = group.name.clone();

    view! {
        <div class="min-w-0">
            <span class="font-medium text-content">{code}</span>
            <div class="truncate-fade text-2xs text-content-subtle">{name}</div>
        </div>
    }
}

/// The member taxes, in the order they apply.
///
/// Order is the point: a compound tax is charged on everything above it, so a
/// group drawn in an arbitrary order would be a group nobody can check.
fn members_cell(group: &TaxGroup) -> impl IntoView {
    let members: Vec<(String, bool)> = group
        .members
        .iter()
        .map(|member| (member.code.clone(), member.is_compound))
        .collect();

    view! {
        <div class="flex flex-wrap items-center gap-1">
            {if members.is_empty() {
                view! { <span class="text-xs text-content-subtle">{l!("common.none")}</span> }
                    .into_any()
            } else {
                members
                    .into_iter()
                    .map(|(code, is_compound)| {
                        let tone = if is_compound { Tone::Warning } else { Tone::Brand };
                        view! { <Badge label=code tone=tone /> }
                    })
                    .collect_view()
                    .into_any()
            }}
        </div>
    }
}

fn active_badge(is_active: bool) -> impl IntoView {
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
    use phonix_tax::code::TaxCode;
    use phonix_tax::rate::TaxRate;
    use uuid::Uuid;

    fn grid() -> GridConfig<TaxCodeSummary> {
        Owner::new().with(taxes_grid)
    }

    fn tax(code: &str, rate: Option<&str>, is_active: bool) -> TaxCodeSummary {
        TaxCodeSummary {
            code: TaxCode {
                id: Uuid::nil(),
                code: code.to_owned(),
                name: format!("{code} tax"),
                kind: TaxKind::Vat,
                country: None,
                region_code: None,
                is_compound: false,
                is_recoverable: true,
                is_active,
            },
            rate_today: rate.map(|r| TaxRate::parse_percent(r).unwrap()),
        }
    }

    #[test]
    fn a_phone_keeps_the_code_and_the_rate() {
        let grid = grid();
        let essential: Vec<&str> = grid
            .columns
            .iter()
            .filter(|c| c.essential)
            .map(|c| c.field())
            .collect();

        assert_eq!(essential, vec!["code", "rate_today"]);
    }

    #[test]
    fn the_rate_column_sorts_numerically_rather_than_by_its_printed_text() {
        // "8.625%" and "20%" sort the wrong way round as strings, and a tax
        // list sorted like that is one nobody trusts.
        let grid = grid();
        let column = grid
            .columns
            .iter()
            .find(|c| c.field() == "rate_today")
            .unwrap();

        let small = column.value(&tax("A", Some("8.625"), true));
        let large = column.value(&tax("B", Some("20"), true));
        assert_eq!(small.compare(&large), std::cmp::Ordering::Less);
    }

    #[test]
    fn a_tax_with_no_rate_is_findable_as_a_group() {
        // The first thing to do after setting a workspace up is to find the
        // taxes that would refuse a document.
        let grid = grid();
        let filter = grid.filters.iter().find(|f| f.key() == "status").unwrap();
        let unpriced = PageRequest::first(25).filtered_by("status", "unpriced");

        assert!(filter.accepts(&tax("NEW", None, true), &unpriced));
        assert!(!filter.accepts(&tax("VAT", Some("20"), true), &unpriced));
    }

    #[test]
    fn every_kind_is_offered_as_a_filter() {
        let grid = grid();
        let filter = grid.filters.iter().find(|f| f.key() == "kind").unwrap();

        // Every kind, plus the "all" option.
        assert_eq!(filter.choices.len(), TaxKind::ALL.len() + 1);
    }

    #[test]
    fn every_action_on_both_grids_names_a_permission() {
        for actions in [
            Owner::new().with(taxes_grid).actions,
            // Different type, same rule - so it is asserted separately rather
            // than by a loop over something these two do not share.
        ] {
            for action in &actions {
                assert!(action.permission.is_some(), "{} is ungated", action.label);
            }
        }

        for action in &Owner::new().with(tax_groups_grid).actions {
            assert!(action.permission.is_some(), "{} is ungated", action.label);
        }
    }

    #[test]
    fn both_grids_open_sorted_by_a_column_that_sorts() {
        let taxes = Owner::new().with(taxes_grid);
        let sort = taxes.initial_request().sort.unwrap();
        assert!(
            taxes
                .columns
                .iter()
                .any(|c| c.sortable && c.field() == sort.field)
        );

        let groups = Owner::new().with(tax_groups_grid);
        let sort = groups.initial_request().sort.unwrap();
        assert!(
            groups
                .columns
                .iter()
                .any(|c| c.sortable && c.field() == sort.field)
        );
    }
}
