//! The audit-log grid.
//!
//! # The worked example for a paged list
//!
//! [`users`](super::users) is the in-memory case: a workspace has as many
//! accounts as it has people, so all of them are fetched once and the browser
//! does the searching. The trail is the other case. Nothing ever deletes from
//! it, so there is no number of rows at which fetching all of it stops being
//! wrong - only a date at which it becomes obvious. It uses [`Source::paged`],
//! and **nothing else about the configuration changes**: the same columns, the
//! same toolbar, the same export, the same column menu.
//!
//! Two things do follow from the choice, and both are visible below:
//!
//! * Only columns the server can actually order by are [`sortable`], because a
//!   sort here becomes an `ORDER BY` and one naming a column the reader does
//!   not know is silently ignored. The list is
//!   `phonix_db::identity::audit::SORTABLE`.
//! * Only columns the server actually searches are [`searchable`], for the
//!   same reason - and the placeholder says which, because a search box that
//!   quietly ignores the field somebody is typing into is worse than none.
//!
//! # Why the trail is the screen the calendar was built for
//!
//! An audit is read by narrowing to a day: "what happened on the fourth", "show
//! me the week of the incident". The list is also the one that grows without
//! limit, so a range is not a convenience here but the only way to reach last
//! March without turning twenty pages.
//!
//! It is a [`DateFilter`] with no `at` closure, for the same reason the scope
//! filter has no `matching`: twenty-five rows out of a year cannot be narrowed
//! to a week in the browser. The two instants cross the wire and become two
//! more lines of the `WHERE`.
//!
//! # Why there is a filter and not a checkbox
//!
//! "Only failures and permission changes" cannot be a search: `notable` is not
//! a word in any column, it is a question about the event name and the outcome.
//! It also cannot be answered in the browser, which holds twenty-five rows out
//! of a year. So it is a [`Filter`] with no `matching` closure: the key crosses
//! the wire and `AuditScope` turns it into a `WHERE` clause.

use std::sync::Arc;

use leptos::prelude::*;
use phonix_core::i18n::Catalog;
use phonix_core::identity::AuditEvent;
use phonix_core::permissions;
use phonix_core::query::Sort;

use super::GridConfig;
use crate::components::page::{Badge, Tone};
use crate::components::user_link::UserLink;
use crate::i18n::Locale;
use crate::icons::Icon;
use crate::l;
use crate::server_fns::admin_fns::audit_trail;
use crate::ui::table::{Cell, Column, DateFilter, Filter, FilterChoice, RowAction, Source};

/// Which entries to show. The values are what
/// `phonix_db::identity::audit::AuditScope` reads, and the two lists have to
/// agree - a value this build sends that the reader does not know is treated
/// as "everything", so a disagreement shows up as a control that does nothing.
///
/// A function rather than a `const`: only the *values* are fixed, and a
/// translated label is a `String`.
fn scopes() -> Vec<FilterChoice> {
    vec![
        FilterChoice::all(l!("audit.filter.all")),
        FilterChoice::new("notable", l!("audit.filter.notable")),
        FilterChoice::new("failures", l!("audit.filter.failures")),
    ]
}

/// The security trail.
pub fn audit_grid() -> GridConfig<AuditEvent> {
    // Held rather than fetched per cell: a column's `read` runs wherever the
    // exporter calls it, with no reactive owner to read the context from.
    let catalog = Locale::get().shared();

    GridConfig::new("audit", Source::paged(audit_trail))
        .searching(l!("audit.search"))
        .exports_as("audit-log")
        // Newest first. The one order an audit log is read in, and the column
        // it opens sorted by has to be one the server can order by.
        .sorted_by(Sort::descending("occurred_at"))
        .min_width("sm:min-w-[52rem]")
        .empty(
            Icon::ScrollText,
            l!("audit.empty.title"),
            l!("audit.empty.detail"),
        )
        .filter(Filter::new("kind", l!("audit.filter.which"), scopes()))
        // No `at`: a paged grid is narrowed where the rows are. The browser
        // resolves "last week" into two instants and `phonix_db` turns them
        // into a `WHERE`, so neither end owns a calendar - see
        // `phonix_db::identity::audit::OCCURRED`, which is this key.
        .date_filter(DateFilter::new("occurred", l!("field.when")))
        .column(
            Column::new("occurred_at", l!("field.when"), |event: &AuditEvent| {
                Cell::timestamp(event.occurred_at)
            })
            .sortable()
            // Half of what an audit row is: a thing, and when. Kept on a
            // phone for that reason.
            .essential()
            .class("whitespace-nowrap text-xs text-content-muted"),
        )
        .column({
            let words = Arc::clone(&catalog);
            let rendering = Arc::clone(&catalog);

            Column::new("event", l!("field.event"), move |event: &AuditEvent| {
                Cell::text(words.render(&event.label()))
            })
            .findable()
            .essential()
            .render(move |event| event_cell(event, &rendering).into_any())
        })
        .column(
            Column::new("email", l!("field.account"), |event: &AuditEvent| {
                // `Empty` rather than a dash: a failed sign-in for an address
                // with no account is a row worth finding, and it sorts to one
                // end instead of under the punctuation.
                Cell::maybe(event.email.clone())
            })
            .findable()
            // `user_id` is absent on exactly the rows where there is nobody to
            // look up: a failed sign-in for an address that has no account.
            // The address still renders; there is simply no button.
            .render(|event| {
                view! { <UserLink email=event.email.clone() user_id=event.user_id /> }.into_any()
            }),
        )
        .column(
            Column::new("ip", l!("field.from"), |event: &AuditEvent| {
                Cell::maybe(event.ip.clone())
            })
            .findable()
            .class("whitespace-nowrap text-xs text-content-muted"),
        )
        .column({
            let succeeded = l!("audit.outcome.succeeded");
            let failed = l!("audit.outcome.failed");

            Column::new(
                "succeeded",
                l!("field.result"),
                move |event: &AuditEvent| {
                    Cell::text(if event.succeeded {
                        succeeded.clone()
                    } else {
                        failed.clone()
                    })
                },
            )
            .sortable()
            // Already a badge in the event cell. Here so it can be sorted and
            // exported on its own, which is what "show me the failures" looks
            // like in a spreadsheet.
            .hidden()
        })
        .column(
            Column::new("summary", l!("field.detail"), |event: &AuditEvent| {
                Cell::maybe(event.summary.clone())
            })
            // Rendered from a JSON object the server holds and the browser does
            // not, so it can be read and exported but not searched or sorted.
            .class("text-xs text-content-subtle"),
        )
        .action(
            // The whole reason somebody is on this screen is to read one of
            // these, so the row itself opens as well as the menu entry. The
            // entry stays: it is the keyboard's way in, it is what a middle
            // click opens in a tab, and it is the same URL either way because
            // there is only one of it written down.
            RowAction::link(l!("common.open"), Icon::Eye, |event: &AuditEvent| {
                format!("/admin/audit-logs/{}", event.id)
            })
            .on_row_click()
            .require(permissions::AUDIT_LOGS),
        )
}

/// The event, its stored name, and a badge when it did not succeed.
fn event_cell(event: &AuditEvent, catalog: &Catalog) -> impl IntoView {
    let label = catalog.render(&event.label());
    let name = event.event.clone();
    let failed = !event.succeeded;
    // Notable *and* successful: a failure already has the louder badge, and
    // two on one row is one too many.
    let notable = event.is_notable() && event.succeeded;

    view! {
        <div>
            <div class="flex flex-wrap items-center gap-1.5">
                <span class="font-medium text-content">{label}</span>
                {failed
                    .then(|| {
                        view! {
                            <Badge
                                label=l!("audit.outcome.failed")
                                tone=Tone::Danger
                                icon=Icon::CircleAlert
                            />
                        }
                    })}
                {notable
                    .then(|| {
                        view! { <Badge label=l!("audit.outcome.notable") tone=Tone::Warning /> }
                    })}
            </div>
            <code class="text-2xs text-content-subtle">{name}</code>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> GridConfig<AuditEvent> {
        Owner::new().with(audit_grid)
    }

    /// The columns the DBAL will order by. Kept here as a literal rather than
    /// imported: `phonix-web` does not depend on `phonix-db`, and the point of
    /// the test is that the two lists were written to agree.
    const SERVER_SORTS: &[&str] = &["occurred_at", "event", "email", "succeeded", "ip"];

    #[test]
    fn every_sortable_column_is_one_the_server_can_order_by() {
        // A paged grid turns a click on a header into an ORDER BY. A column
        // that offers a sort the reader ignores is a header that looks
        // clickable and does nothing.
        for column in grid().columns.iter().filter(|column| column.sortable) {
            assert!(
                SERVER_SORTS.contains(&column.field()),
                "{} offers a sort the server cannot answer",
                column.field(),
            );
        }
    }

    #[test]
    fn the_searchable_columns_are_the_ones_the_placeholder_names() {
        let grid = grid();
        let searchable: Vec<&str> = grid
            .columns
            .iter()
            .filter(|c| c.searchable)
            .map(|c| c.field())
            .collect();

        // "Filter by event, account or address".
        assert_eq!(searchable, ["event", "email", "ip"]);
    }

    #[test]
    fn the_detail_column_promises_neither_a_search_nor_a_sort() {
        let grid = grid();
        let summary = grid
            .columns
            .iter()
            .find(|c| c.field() == "summary")
            .unwrap();

        // It is rendered from a JSON object the browser never receives.
        assert!(!summary.searchable);
        assert!(!summary.sortable);
    }

    #[test]
    fn it_opens_newest_first() {
        assert_eq!(
            grid().initial_request().sort,
            Some(Sort::descending("occurred_at"))
        );
    }

    #[test]
    fn the_range_is_answered_by_the_server_and_named_the_same_thing_it_is_read_as() {
        let grid = grid();
        let range = grid
            .date_filters
            .first()
            .expect("the grid offers a date range");

        // `phonix_db::identity::audit::OCCURRED`. The two crates do not depend
        // on each other, so the agreement is written down twice and checked
        // here - a disagreement is a calendar that changes nothing.
        assert_eq!(range.key(), "occurred");
        // A closure could only narrow the twenty-five rows already fetched,
        // which is not what "last week" means.
        assert!(!range.is_local());
    }

    #[test]
    fn the_scope_filter_leaves_the_answering_to_the_server() {
        let grid = grid();
        let scope = grid
            .filters
            .first()
            .expect("the grid offers a scope filter");

        // A closure here could only narrow the twenty-five rows already
        // fetched, which is not what "only failures" means.
        assert_eq!(scope.key(), "kind");
        assert!(!scope.is_local());
        assert_eq!(scope.default_value(), "");
    }

    #[test]
    fn opening_an_entry_needs_the_permission_that_lists_them() {
        let grid = grid();
        let open = grid.actions.iter().find(|a| a.label == "Open").unwrap();

        assert_eq!(open.permission, Some(permissions::AUDIT_LOGS));
    }

    #[test]
    fn nothing_can_be_done_to_an_entry() {
        // An audit log an administrator can edit is not one. Every action here
        // must be a link; a `Run` would be something that changes a row.
        for action in &grid().actions {
            assert!(
                matches!(action.kind, crate::ui::table::action::ActionKind::Link(_)),
                "{} can change an audit entry",
                action.label,
            );
        }
    }
}
