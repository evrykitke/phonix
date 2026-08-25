//! The document number series a workspace has.
//!
//! # The counter is shown and never edited
//!
//! It is the series' own record of what it has handed out. A screen that let
//! somebody type a new one is a screen that issues a number twice, so it is a
//! column here and not a field on the form. Moving a series on is `start_at`,
//! which the allocation honours the next time it runs.
//!
//! # There is no "new series" button
//!
//! A series belongs to a document type, and document types come from apps -
//! `config/numbering/<app_id>.toml`, inserted when the app is installed. A
//! series somebody created by hand would be a series no app ever asks for.

use leptos::prelude::*;
use phonix_core::numbering::NumberSeries;
use phonix_core::permissions;
use phonix_core::query::Sort;

use super::GridConfig;
use crate::components::page::{Badge, Tone};
use crate::icons::Icon;
use crate::l;
use crate::server_fns::numbering_fns::list_number_series;
use crate::ui::table::{Align, Cell, Column, Filter, FilterChoice, RowAction, Source};

/// Every series, and where each has got to.
///
/// `on_edit` opens the panel below the grid: a settings tab has nowhere to
/// navigate to, so the form is a panel on the same screen.
pub fn number_series_grid(on_edit: Callback<NumberSeries>) -> GridConfig<NumberSeries> {
    GridConfig::new("number-series", Source::in_memory(list_number_series))
        .searching(l!("numbering.search"))
        .exports_as("number-series")
        .sorted_by(Sort::ascending("doc_type"))
        .min_width("sm:min-w-[48rem]")
        .empty(
            Icon::FileText,
            l!("numbering.empty.title"),
            l!("numbering.empty.detail"),
        )
        .column(
            Column::new(
                "doc_type",
                l!("numbering.doc_type"),
                |row: &NumberSeries| Cell::text(row.key()),
            )
            .findable()
            .pinned()
            .essential()
            .render(|row| document_cell(row).into_any()),
        )
        .column(
            Column::new("app_id", l!("numbering.app"), |row: &NumberSeries| {
                Cell::text(&row.app_id)
            })
            .findable()
            // Already under the document type; here so a workspace with several
            // apps can group by it.
            .hidden(),
        )
        .column(
            Column::new("pattern", l!("numbering.format"), |row: &NumberSeries| {
                Cell::text(row.pattern.as_str())
            })
            .findable()
            // The question the screen is opened to answer.
            .essential()
            .class("font-mono text-xs"),
        )
        .column(
            Column::new(
                "reset_period",
                l!("numbering.reset"),
                |row: &NumberSeries| Cell::text(row.reset_period.as_str()),
            )
            .sortable()
            .render(|row| {
                let label = crate::i18n::t(&row.reset_period.label());
                view! { <Badge label=label /> }.into_any()
            }),
        )
        .column(
            Column::new("counter", l!("numbering.issued"), |row: &NumberSeries| {
                Cell::number(row.counter as f64)
            })
            .sortable()
            .align(Align::End)
            .class("tabular-nums text-content-muted")
            .render(|row| issued_cell(row).into_any()),
        )
        .column(
            Column::new(
                "start_at",
                l!("numbering.start_at"),
                |row: &NumberSeries| Cell::number(row.start_at as f64),
            )
            .sortable()
            .align(Align::End)
            .class("tabular-nums text-content-muted")
            .hidden(),
        )
        .column(
            Column::new("is_active", l!("field.status"), |row: &NumberSeries| {
                Cell::bool(row.is_active)
            })
            .sortable()
            .render(|row| {
                if row.is_active {
                    view! { <Badge label=l!("common.active") tone=Tone::Success /> }.into_any()
                } else {
                    view! { <Badge label=l!("common.inactive") /> }.into_any()
                }
            }),
        )
        .filter(
            Filter::new(
                "state",
                l!("field.status"),
                vec![
                    FilterChoice::all(l!("common.all")),
                    FilterChoice::new("active", l!("common.active")),
                    FilterChoice::new("inactive", l!("common.inactive")),
                    // Worth finding: a series that has never issued can have
                    // its format changed freely, and one that has cannot.
                    FilterChoice::new("unused", l!("numbering.never_issued")),
                ],
            )
            .matching(|row: &NumberSeries, wanted| match wanted {
                "active" => row.is_active,
                "inactive" => !row.is_active,
                "unused" => !row.has_issued(),
                _ => true,
            }),
        )
        .action(
            RowAction::run(
                l!("common.edit"),
                Icon::Pencil,
                move |row: NumberSeries, _| on_edit.run(row),
            )
            .require(permissions::SETTINGS),
        )
}

/// The document type, with the app that declared it under it.
fn document_cell(row: &NumberSeries) -> impl IntoView {
    let doc_type = row.doc_type.replace('_', " ");
    let app_id = row.app_id.clone();
    let scope = row.scope_key.clone();

    view! {
        <div class="min-w-0">
            <div class="flex flex-wrap items-center gap-1.5">
                <span class="font-medium capitalize text-content">{doc_type}</span>
                {(!scope.is_empty()).then(|| view! { <Badge label=scope tone=Tone::Brand /> })}
            </div>
            <code class="text-2xs text-content-subtle">{app_id}</code>
        </div>
    }
}

/// How far the series has got.
///
/// "None yet" rather than a zero: a series that has never issued is one whose
/// format can still be changed freely, which is the fact somebody reading this
/// column actually wants.
fn issued_cell(row: &NumberSeries) -> impl IntoView {
    let counter = row.counter;
    let has_issued = row.has_issued();

    view! {
        <span class="whitespace-nowrap">
            {if has_issued {
                view! { <span class="tabular-nums text-content">{counter}</span> }.into_any()
            } else {
                view! {
                    <span class="text-xs text-content-subtle">{l!("numbering.never_issued")}</span>
                }
                    .into_any()
            }}
        </span>
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use phonix_core::numbering::{Pattern, ResetPeriod};
    use uuid::Uuid;

    use super::*;

    fn grid() -> GridConfig<NumberSeries> {
        Owner::new().with(|| number_series_grid(Callback::new(|_| {})))
    }

    fn series(counter: i64, scope: &str) -> NumberSeries {
        NumberSeries {
            id: Uuid::nil(),
            app_id: "books".to_owned(),
            doc_type: "sales_invoice".to_owned(),
            scope_key: scope.to_owned(),
            pattern: Pattern::parse("INV-{YYYY}-#####").expect("a valid mask"),
            reset_period: ResetPeriod::FiscalYear,
            period_key: String::new(),
            counter,
            start_at: 1,
            is_active: true,
            updated_at: DateTime::<Utc>::from_timestamp(0, 0).expect("the epoch"),
            updated_by: None,
        }
    }

    #[test]
    fn there_is_no_way_to_create_or_delete_a_series_here() {
        // A series belongs to a document type, and document types come from an
        // app's configuration file. One created by hand is one no app asks for.
        let grid = grid();

        assert!(grid.toolbar.is_empty(), "a series is not created by hand");
        assert!(
            !grid.actions.iter().any(|a| a.label.contains("Delete")),
            "a series that has issued numbers must stay explicable",
        );
    }

    #[test]
    fn the_counter_is_a_column_and_never_a_field() {
        // A screen that let somebody type a new counter is a screen that
        // issues a number twice.
        let grid = grid();

        assert!(grid.columns.iter().any(|c| c.field() == "counter"));
    }

    #[test]
    fn a_series_that_has_never_issued_is_findable() {
        // Its format can still be changed freely; one that has issued cannot.
        let grid = grid();
        let filter = grid.filters.iter().find(|f| f.key() == "state").unwrap();
        let unused = phonix_core::query::PageRequest::first(25).filtered_by("state", "unused");

        assert!(filter.accepts(&series(0, ""), &unused));
        assert!(!filter.accepts(&series(41, ""), &unused));
    }

    #[test]
    fn a_scoped_series_is_named_apart_from_an_unscoped_one() {
        // Otherwise a workspace numbering per branch has four identical rows.
        let grid = grid();
        let column = grid
            .columns
            .iter()
            .find(|c| c.field() == "doc_type")
            .unwrap();

        assert_eq!(
            column.value(&series(0, "")).to_text(),
            "books.sales_invoice"
        );
        assert_eq!(
            column.value(&series(0, "NBO")).to_text(),
            "books.sales_invoice@NBO"
        );
    }

    #[test]
    fn a_phone_keeps_the_document_type_and_the_format() {
        let grid = grid();
        let essential: Vec<&str> = grid
            .columns
            .iter()
            .filter(|c| c.essential)
            .map(|c| c.field())
            .collect();

        assert_eq!(essential, vec!["doc_type", "pattern"]);
    }
}
