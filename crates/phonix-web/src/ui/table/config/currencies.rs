//! The workspace's currency list.
//!
//! # The row is the selection, not the currency
//!
//! Names and minor units are read from
//! [`Currency`](phonix_core::locale::Currency), which is compiled into the
//! bundle. This grid shows *which* codes the workspace has switched on and what
//! symbol it wants beside them - so nothing here is a second copy of ISO 4217
//! with its own answer to "how many decimal places does the yen have".
//!
//! # There is no delete
//!
//! A currency the workspace has stopped using is disabled. Rates and posted
//! documents still have to resolve, and a foreign-key error naming
//! `exchange_rates` is not a useful answer to somebody tidying a picker.

use leptos::prelude::*;
use phonix_core::money::WorkspaceCurrency;
use phonix_core::permissions;
use phonix_core::query::Sort;

use super::GridConfig;
use crate::components::page::{Badge, Tone};
use crate::icons::Icon;
use crate::l;
use crate::server_fns::currency_fns::{set_currency_enabled, workspace_currencies};
use crate::ui::table::{
    Align, Cell, Column, Filter, FilterChoice, RowAction, Source, ToolbarAction,
};

/// What this workspace deals in.
///
/// `on_edit` and `on_add` open the panel below the grid, which is why they are
/// arguments: a settings tab has nowhere to navigate to, so the "form" is a
/// panel on the same screen and the grid has to be able to point at it.
pub fn currencies_grid(
    on_edit: Callback<WorkspaceCurrency>,
    on_add: Callback<()>,
) -> GridConfig<WorkspaceCurrency> {
    GridConfig::new("currencies", Source::in_memory(workspace_currencies))
        .searching(l!("currencies.search"))
        .exports_as("currencies")
        .sorted_by(Sort::ascending("code"))
        .min_width("sm:min-w-[38rem]")
        .empty(
            Icon::Boxes,
            l!("currencies.empty.title"),
            l!("currencies.empty.detail"),
        )
        .column(
            Column::new("code", l!("field.code"), |row: &WorkspaceCurrency| {
                Cell::text(row.currency.code())
            })
            .findable()
            .pinned()
            .essential()
            .render(|row| code_cell(row).into_any()),
        )
        .column(
            Column::new("name", l!("field.name"), |row: &WorkspaceCurrency| {
                Cell::text(row.currency.name())
            })
            .findable()
            // Already under the code; here so it can be sorted and exported.
            .hidden(),
        )
        .column(
            Column::new("symbol", l!("field.symbol"), |row: &WorkspaceCurrency| {
                Cell::text(row.display())
            })
            .searchable()
            // The one field on this screen that is genuinely the organization's
            // choice: `$` is a dozen currencies and which one it means depends
            // entirely on who is reading.
            .essential()
            .align(Align::Center),
        )
        .column(
            Column::new(
                "minor_units",
                l!("currencies.minor_units"),
                |row: &WorkspaceCurrency| Cell::number(f64::from(row.currency.minor_units())),
            )
            .sortable()
            .align(Align::End)
            .class("tabular-nums text-content-muted"),
        )
        .column(
            Column::new(
                "is_enabled",
                l!("field.status"),
                |row: &WorkspaceCurrency| Cell::bool(row.is_enabled),
            )
            .sortable()
            .render(|row| status_cell(row.is_enabled).into_any()),
        )
        .filter(
            Filter::new(
                "status",
                l!("field.status"),
                vec![
                    FilterChoice::all(l!("common.all")),
                    FilterChoice::new("enabled", l!("common.active")),
                    FilterChoice::new("disabled", l!("common.inactive")),
                ],
            )
            .matching(|row: &WorkspaceCurrency, wanted| match wanted {
                "enabled" => row.is_enabled,
                "disabled" => !row.is_enabled,
                _ => true,
            }),
        )
        .toolbar(
            ToolbarAction::run(l!("currencies.add"), Icon::Plus, move || on_add.run(()))
                .require(permissions::SETTINGS)
                .primary(),
        )
        .action(
            RowAction::run(
                l!("common.edit"),
                Icon::Pencil,
                move |row: WorkspaceCurrency, _| on_edit.run(row),
            )
            .require(permissions::SETTINGS),
        )
        .action(
            RowAction::run(
                l!("currencies.disable"),
                Icon::EyeOff,
                |row: WorkspaceCurrency, grid| {
                    let code = row.currency.code().to_owned();

                    leptos::task::spawn_local(async move {
                        match set_currency_enabled(code.clone(), false).await {
                            Ok(_) => {
                                grid.report(l!("currencies.disabled", code = code));
                                grid.refresh();
                            }
                            // The server knows whether this was the base
                            // currency or a permission, and either is worth
                            // reading.
                            Err(err) => grid.warn(err.to_string()),
                        }
                    });
                },
            )
            // Offered only where it would do something. Disabling a disabled
            // currency is a button that reports success and changes nothing.
            .when(|row: &WorkspaceCurrency| row.is_enabled)
            .require(permissions::SETTINGS),
        )
        .action(
            RowAction::run(
                l!("currencies.enable"),
                Icon::Eye,
                |row: WorkspaceCurrency, grid| {
                    let code = row.currency.code().to_owned();

                    leptos::task::spawn_local(async move {
                        match set_currency_enabled(code.clone(), true).await {
                            Ok(_) => {
                                grid.report(l!("currencies.enabled", code = code));
                                grid.refresh();
                            }
                            Err(err) => grid.warn(err.to_string()),
                        }
                    });
                },
            )
            .when(|row: &WorkspaceCurrency| !row.is_enabled)
            .require(permissions::SETTINGS),
        )
}

/// The code, with the currency's own name under it.
fn code_cell(row: &WorkspaceCurrency) -> impl IntoView {
    let code = row.currency.code().to_owned();
    let name = row.currency.name().to_owned();

    view! {
        <div class="min-w-0">
            <span class="font-medium text-content">{code}</span>
            <div class="truncate-fade text-2xs text-content-subtle">{name}</div>
        </div>
    }
}

fn status_cell(is_enabled: bool) -> impl IntoView {
    view! {
        {if is_enabled {
            view! { <Badge label=l!("common.active") tone=Tone::Success /> }.into_any()
        } else {
            view! { <Badge label=l!("common.inactive") /> }.into_any()
        }}
    }
}

#[cfg(test)]
mod tests {
    use phonix_core::locale::Currency;

    use super::*;

    fn grid() -> GridConfig<WorkspaceCurrency> {
        Owner::new().with(|| currencies_grid(Callback::new(|_| {}), Callback::new(|()| {})))
    }

    fn currency(code: &str, is_enabled: bool, symbol: Option<&str>) -> WorkspaceCurrency {
        WorkspaceCurrency {
            currency: Currency::parse(code).expect("a real currency"),
            is_enabled,
            symbol: symbol.map(str::to_owned),
        }
    }

    #[test]
    fn there_is_no_delete_action() {
        // A currency the workspace has stopped using is disabled. Rates and
        // posted documents still have to resolve.
        let grid = grid();

        assert!(
            !grid.actions.iter().any(|a| a.label.contains("Delete")),
            "a currency must never be deletable from this screen",
        );
    }

    #[test]
    fn only_one_of_enable_and_disable_is_ever_offered() {
        // Disabling a disabled currency is a button that reports success and
        // changes nothing.
        let grid = grid();
        let enable = grid.actions.iter().find(|a| a.label == "Enable").unwrap();
        let disable = grid.actions.iter().find(|a| a.label == "Disable").unwrap();

        let on = currency("USD", true, None);
        let off = currency("EUR", false, None);

        assert!(disable.applies_to(&on) && !enable.applies_to(&on));
        assert!(enable.applies_to(&off) && !disable.applies_to(&off));
    }

    #[test]
    fn every_action_names_the_permission_the_service_requires() {
        let grid = grid();

        for action in &grid.actions {
            assert_eq!(action.permission, Some(permissions::SETTINGS));
        }
    }

    #[test]
    fn a_currency_with_no_symbol_shows_its_code() {
        let grid = grid();
        let symbol = grid.columns.iter().find(|c| c.field() == "symbol").unwrap();

        assert_eq!(symbol.value(&currency("JPY", true, None)).to_text(), "JPY");
        assert_eq!(
            symbol.value(&currency("JPY", true, Some("¥"))).to_text(),
            "¥"
        );
    }

    #[test]
    fn the_minor_units_come_from_the_compiled_table_and_not_from_a_column() {
        // A hundred and sixty rows per tenant database is a hundred and sixty
        // chances for one workspace to disagree with ISO 4217.
        let grid = grid();
        let units = grid
            .columns
            .iter()
            .find(|c| c.field() == "minor_units")
            .unwrap();

        assert_eq!(units.value(&currency("JPY", true, None)).to_text(), "0");
        assert_eq!(units.value(&currency("USD", true, None)).to_text(), "2");
    }
}
