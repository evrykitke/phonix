//! The invoices grid.
//!
//! # Why "overdue" is a filter and not a badge
//!
//! Whether a document is overdue depends on today, and this grid renders twice:
//! once on the server and once in the browser at hydration. Near midnight those
//! two have different dates, and a badge that appears on one side and not the
//! other is the fatal kind of hydration mismatch - see `phonix_web::recovery`
//! for what a wasm panic costs.
//!
//! A filter is safe because its predicate does not run until somebody chooses
//! it, which is after hydration. So the due date is drawn plainly and "show me
//! what is late" is one click.
//!
//! # A draft says so rather than showing a blank
//!
//! It has no number, because a number is taken at post and never before. An
//! empty cell would read as missing data; "Draft" reads as what it is.

use app_books::invoice::{InvoiceStatus, InvoiceSummary};
use leptos::prelude::*;
use phonix_core::permissions;
use phonix_core::query::Sort;

use super::GridConfig;
use crate::components::page::{Badge, Tone};
use crate::icons::Icon;
use crate::l;
use crate::server_fns::books_fns::{InvoiceQuery, list_invoices};
use crate::ui::table::{
    Align, Cell, Column, Filter, FilterChoice, RowAction, Source, ToolbarAction,
};

/// Everything this workspace has invoiced.
pub fn invoices_grid() -> GridConfig<InvoiceSummary> {
    GridConfig::new(
        "invoices",
        // Unfiltered: the grid narrows in the browser, which is what makes the
        // status tabs instant. A workspace that outgrows that has the query
        // type already waiting.
        Source::in_memory(|| list_invoices(InvoiceQuery::default())),
    )
    .searching(l!("invoices.search"))
    .exports_as("invoices")
    .sorted_by(Sort::descending("issued_on"))
    .min_width("sm:min-w-[52rem]")
    .empty(
        Icon::FileText,
        l!("invoices.empty.title"),
        l!("invoices.empty.detail"),
    )
    .column(
        Column::new("number", l!("invoices.number"), |row: &InvoiceSummary| {
            // Sorts and exports as the number, or as nothing for a draft - so a
            // column sorted by number puts the drafts together rather than
            // scattering them under the word "Draft".
            Cell::maybe(row.number.clone())
        })
        .findable()
        // Without it a row is a name and an amount.
        .pinned()
        .essential()
        .render(|row| number_cell(row).into_any()),
    )
    .column(
        Column::new(
            "party_name",
            l!("invoices.customer"),
            |row: &InvoiceSummary| Cell::text(&row.party_name),
        )
        .findable()
        // The second thing somebody scans for.
        .essential(),
    )
    .column(
        Column::new(
            "issued_on",
            l!("invoices.issued"),
            |row: &InvoiceSummary| Cell::text(row.issued_on.to_string()),
        )
        .sortable()
        .class("whitespace-nowrap tabular-nums text-content-muted"),
    )
    .column(
        Column::new("due_on", l!("invoices.due"), |row: &InvoiceSummary| {
            Cell::maybe(row.due_on.map(|due| due.to_string()))
        })
        .sortable()
        .class("whitespace-nowrap tabular-nums text-content-muted"),
    )
    .column(
        Column::new("status", l!("field.status"), |row: &InvoiceSummary| {
            Cell::text(row.status.as_str())
        })
        .sortable()
        .essential()
        .render(|row| status_cell(row.status).into_any()),
    )
    .column(
        Column::new("net", l!("invoices.net"), |row: &InvoiceSummary| {
            // The scaled integer, so sorting is numeric: "9.00" and "10.00"
            // sort the wrong way round as strings, and a list of money sorted
            // like that is one nobody trusts.
            Cell::number(row.net.scaled() as f64)
        })
        .sortable()
        .align(Align::End)
        .hidden()
        .render(|row| amount_cell(&row.net).into_any()),
    )
    .column(
        Column::new("tax", l!("invoices.tax"), |row: &InvoiceSummary| {
            Cell::number(row.tax.scaled() as f64)
        })
        .sortable()
        .align(Align::End)
        .hidden()
        .render(|row| amount_cell(&row.tax).into_any()),
    )
    .column(
        Column::new("gross", l!("invoices.total"), |row: &InvoiceSummary| {
            Cell::number(row.gross.scaled() as f64)
        })
        .sortable()
        .align(Align::End)
        .render(|row| amount_cell(&row.gross).into_any()),
    )
    .column(
        Column::new(
            "line_count",
            l!("invoices.lines"),
            |row: &InvoiceSummary| Cell::number(row.line_count as f64),
        )
        .sortable()
        .align(Align::End)
        .hidden(),
    )
    .filter(
        Filter::new(
            "status",
            l!("field.status"),
            vec![
                FilterChoice::all(l!("common.all")),
                FilterChoice::new("draft", l!("books.status.draft")),
                FilterChoice::new("posted", l!("books.status.posted")),
                FilterChoice::new("voided", l!("books.status.voided")),
                // Not the default, which is what makes it safe: the predicate
                // reads today's date and does not run until somebody chooses
                // this, which is after hydration.
                FilterChoice::new("overdue", l!("invoices.overdue")),
            ],
        )
        .matching(|row: &InvoiceSummary, wanted| match wanted {
            "overdue" => row.is_overdue(chrono::Local::now().date_naive()),
            other => row.status.as_str() == other,
        }),
    )
    .toolbar(
        ToolbarAction::link(l!("invoices.new"), Icon::Plus, "/sales/invoices/new")
            .require(permissions::INVOICES_CREATE)
            .primary(),
    )
    .action(
        RowAction::link(l!("common.open"), Icon::Eye, |row: &InvoiceSummary| {
            format!("/sales/invoices/{}", row.id)
        })
        .require(permissions::INVOICES),
    )
    .action(
        RowAction::link(l!("common.edit"), Icon::Pencil, |row: &InvoiceSummary| {
            format!("/sales/invoices/{}", row.id)
        })
        // Only a draft can be edited. Offering it on a posted document would be
        // offering a button that only ever produces a refusal.
        .when(|row: &InvoiceSummary| row.status.is_editable())
        .require(permissions::INVOICES_EDIT),
    )
}

/// The number, or the word for a draft.
fn number_cell(row: &InvoiceSummary) -> impl IntoView {
    let number = row.number.clone();

    view! {
        {match number {
            Some(number) => {
                view! { <span class="font-medium tabular-nums text-content">{number}</span> }
                    .into_any()
            }
            // Never the number it is *going* to get: promising before the post
            // promises something that may not be kept.
            None => {
                view! {
                    <span class="text-xs italic text-content-subtle">
                        {l!("books.status.draft")}
                    </span>
                }
                    .into_any()
            }
        }}
    }
}

fn status_cell(status: InvoiceStatus) -> impl IntoView {
    let label = crate::i18n::t(&status.label());
    let tone = match status {
        InvoiceStatus::Draft => Tone::Neutral,
        InvoiceStatus::Posted => Tone::Success,
        // Not danger: withdrawing a document is an ordinary correction, and
        // painting it red every time somebody opens the list is noise.
        InvoiceStatus::Voided => Tone::Warning,
    };

    view! { <Badge label=label tone=tone /> }
}

/// An amount with its currency, right-aligned and monospaced so the columns
/// line up under each other.
fn amount_cell(amount: &phonix_core::money::Money) -> impl IntoView {
    let text = amount.to_display_string();
    let code = amount.currency().code().to_owned();

    view! {
        <span class="whitespace-nowrap tabular-nums">
            <span class="text-2xs text-content-subtle">{code}</span>
            " "
            <span class="text-content">{text}</span>
        </span>
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use phonix_core::locale::Currency;
    use phonix_core::money::Money;
    use phonix_core::query::PageRequest;
    use uuid::Uuid;

    use super::*;

    fn grid() -> GridConfig<InvoiceSummary> {
        Owner::new().with(invoices_grid)
    }

    fn usd() -> Currency {
        Currency::parse("USD").unwrap()
    }

    fn invoice(number: Option<&str>, status: InvoiceStatus, gross: &str) -> InvoiceSummary {
        InvoiceSummary {
            id: Uuid::nil(),
            number: number.map(str::to_owned),
            status,
            party_id: Uuid::nil(),
            party_name: "Acme".to_owned(),
            issued_on: NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
            due_on: NaiveDate::from_ymd_opt(2026, 7, 1),
            currency: usd(),
            net: Money::parse(usd(), gross).unwrap(),
            tax: Money::zero(usd()),
            gross: Money::parse(usd(), gross).unwrap(),
            line_count: 1,
        }
    }

    #[test]
    fn a_phone_keeps_the_number_the_customer_and_the_state() {
        let grid = grid();
        let essential: Vec<&str> = grid
            .columns
            .iter()
            .filter(|c| c.essential)
            .map(|c| c.field())
            .collect();

        assert_eq!(essential, vec!["number", "party_name", "status"]);
    }

    #[test]
    fn money_columns_sort_numerically_rather_than_by_their_printed_text() {
        // "9.00" and "10.00" sort the wrong way round as strings.
        let grid = grid();
        let total = grid.columns.iter().find(|c| c.field() == "gross").unwrap();

        let small = total.value(&invoice(None, InvoiceStatus::Draft, "9.00"));
        let large = total.value(&invoice(None, InvoiceStatus::Draft, "10.00"));
        assert_eq!(small.compare(&large), std::cmp::Ordering::Less);
    }

    #[test]
    fn overdue_is_not_the_filter_the_grid_opens_on() {
        // Its predicate reads today's date. If it ran during the first render
        // the server and the browser could disagree near midnight, and a row
        // that appears on one side and not the other is a hydration mismatch.
        let grid = grid();
        let filter = grid.filters.iter().find(|f| f.key() == "status").unwrap();

        assert_ne!(filter.default_value(), "overdue");
        assert!(
            grid.initial_request().filter("status").is_none(),
            "the grid must not open on a date-dependent filter",
        );
    }

    #[test]
    fn the_status_filter_finds_each_state() {
        let grid = grid();
        let filter = grid.filters.iter().find(|f| f.key() == "status").unwrap();
        let asking = |value: &str| PageRequest::first(25).filtered_by("status", value);

        let draft = invoice(None, InvoiceStatus::Draft, "10.00");
        let posted = invoice(Some("INV-2026-00001"), InvoiceStatus::Posted, "10.00");

        assert!(filter.accepts(&draft, &asking("draft")));
        assert!(!filter.accepts(&draft, &asking("posted")));
        assert!(filter.accepts(&posted, &asking("posted")));
    }

    #[test]
    fn only_a_draft_is_offered_an_edit_button() {
        // A posted invoice cannot be edited, so the button could only ever
        // produce a refusal.
        let grid = grid();
        let edit = grid.actions.iter().find(|a| a.label == "Edit").unwrap();

        assert!(edit.applies_to(&invoice(None, InvoiceStatus::Draft, "10.00")));
        assert!(!edit.applies_to(&invoice(
            Some("INV-2026-00001"),
            InvoiceStatus::Posted,
            "10.00"
        )));
    }

    #[test]
    fn there_is_no_delete_and_no_post_on_a_row() {
        // Both are decisions with consequences - one destroys a draft, the
        // other takes a number nobody can hand back. They belong on the
        // document, where the whole thing is in front of the reader, not in a
        // menu at the end of a row.
        let grid = grid();

        for label in ["Delete", "Post", "Void"] {
            assert!(
                !grid.actions.iter().any(|a| a.label == label),
                "{label} must not be a row action",
            );
        }
    }

    #[test]
    fn every_action_names_a_permission() {
        let grid = grid();

        for action in &grid.actions {
            assert!(action.permission.is_some(), "{} is ungated", action.label);
        }
    }
}
