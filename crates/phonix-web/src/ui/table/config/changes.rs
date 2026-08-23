//! The change-trail grid.
//!
//! The other half of the audit screen. [`audit`](super::audit) lists what
//! *happened* - sign-ins, lockouts, spent recovery codes. This lists what
//! *changed*: one row per record created, edited or deleted, naming the record
//! rather than only the person.
//!
//! Everything the audit grid's docs say about a paged list applies here
//! unchanged: only columns `phonix_db::audit::SORTABLE` knows are
//! [`sortable`](Column::sortable), only the columns the `WHERE` actually
//! searches are [`findable`](Column::findable), and the date range is two
//! instants resolved in the browser rather than a preset name.
//!
//! # Two filters, because a reader narrows on two different axes
//!
//! "Show me role changes" and "show me deletions" are separate questions and
//! get asked separately - a single combined control would force somebody
//! looking for every deletion to pick a kind first. Both are answered where the
//! rows are, for the reason the audit grid gives: twenty-five rows out of a
//! year cannot be narrowed to the answer in the browser.
//!
//! The kind choices are built from [`ENTITY_KINDS`] rather than written out
//! here. A kind added to that list appears in this filter with no edit to this
//! file, which is the whole point of declaring the vocabulary in code.

use std::sync::Arc;

use leptos::prelude::*;
use phonix_core::audit::{ENTITY_KINDS, EntityAction, EntityChange};
use phonix_core::i18n::Catalog;
use phonix_core::permissions;
use phonix_core::query::Sort;

use super::GridConfig;
use crate::components::page::{Badge, Tone};
use crate::components::user_link::UserLink;
use crate::i18n::Locale;
use crate::icons::Icon;
use crate::l;
use crate::server_fns::admin_fns::entity_trail;
use crate::ui::table::{Cell, Column, DateFilter, Filter, FilterChoice, RowAction, Source};

/// Which verbs to show.
///
/// The *values* are what `phonix_db::audit::page` binds against the `action`
/// column, which is `EntityAction::as_str` - so they stay English whatever the
/// reader's language is. Only the labels move.
///
/// A function rather than a `const`, because a translated label is a `String`
/// and a `String` cannot live in a constant.
fn actions() -> Vec<FilterChoice> {
    vec![
        FilterChoice::all(l!("changes.filter.any")),
        FilterChoice::new("created", l!("audit.action.created")),
        FilterChoice::new("updated", l!("audit.action.updated")),
        FilterChoice::new("deleted", l!("audit.action.deleted")),
    ]
}

/// The change trail.
pub fn changes_grid() -> GridConfig<EntityChange> {
    // Read once, here, and moved into the cell closures. Each closure is called
    // per row and cannot reach the context itself: `Locale::get` wants a
    // reactive owner, and a column's `read` runs wherever the exporter happens
    // to call it from.
    let catalog = Locale::get().shared();

    GridConfig::new("changes", Source::paged(entity_trail))
        .searching(l!("changes.search"))
        .exports_as("change-log")
        // Newest first, like the security trail: it is the one order a trail is
        // read in, and the column it opens sorted by has to be one the server
        // can order by.
        .sorted_by(Sort::descending("occurred_at"))
        .min_width("sm:min-w-[52rem]")
        .empty(
            Icon::ClipboardList,
            l!("changes.empty.title"),
            l!("changes.empty.detail"),
        )
        .filter(Filter::new(
            "kind",
            l!("changes.filter.which"),
            kind_choices(&catalog),
        ))
        .filter(Filter::new("action", l!("changes.filter.what"), actions()))
        // No `at`: narrowed where the rows are. See the audit grid's docs -
        // `phonix_db::audit::OCCURRED` is this key.
        .date_filter(DateFilter::new("occurred", l!("field.when")))
        .column(
            Column::new("occurred_at", l!("field.when"), |change: &EntityChange| {
                Cell::timestamp(change.occurred_at)
            })
            .sortable()
            .essential()
            .class("whitespace-nowrap text-xs text-content-muted"),
        )
        .column({
            let words = Arc::clone(&catalog);
            let rendering = Arc::clone(&catalog);

            Column::new("label", l!("field.record"), move |change: &EntityChange| {
                Cell::text(change.record(&words))
            })
            .sortable()
            .findable()
            // The other half of what a change row is: which record, and what
            // was done to it. Kept on a phone for that reason.
            .essential()
            .render(move |change| record_cell(change, &rendering).into_any())
        })
        .column({
            let words = Arc::clone(&catalog);

            Column::new(
                "entity_type",
                l!("field.kind"),
                move |change: &EntityChange| Cell::text(change.kind_label(&words)),
            )
            .sortable()
            .findable()
            // Already the second line of the record cell. Here so it can be
            // sorted and exported on its own.
            .hidden()
        })
        .column({
            let words = Arc::clone(&catalog);

            Column::new(
                "action",
                l!("field.change"),
                move |change: &EntityChange| Cell::text(words.render(&change.action.name())),
            )
            .sortable()
            .hidden()
        })
        .column(
            Column::new("actor_email", l!("field.by"), |change: &EntityChange| {
                // `Empty` rather than a dash: a change with nobody behind it is
                // one the system made, and it is worth being able to find.
                Cell::maybe(change.actor_email.clone())
            })
            .sortable()
            .findable()
            // The address is what the row recorded and what it keeps saying;
            // the button beside it is how somebody finds out whose it is
            // without leaving the page. The export still gets the address
            // alone - `Cell::maybe` above is what a spreadsheet receives.
            .render(|change| {
                view! {
                    <UserLink
                        email=change.actor_email.clone()
                        user_id=change.actor_id
                        absent=l!("audit.actor.system")
                    />
                }
                .into_any()
            }),
        )
        .column(
            Column::new("summary", l!("field.detail"), |change: &EntityChange| {
                Cell::maybe(change.summary.clone())
            })
            // Rendered from a JSON object the server holds and the browser does
            // not, so it can be read and exported but not searched or sorted.
            .class("text-xs text-content-subtle"),
        )
        .action(
            RowAction::link(l!("common.open"), Icon::Eye, |change: &EntityChange| {
                format!("/admin/changes/{}", change.id)
            })
            .require(permissions::AUDIT_LOGS),
        )
}

/// The declared kinds as filter choices, in the order they are declared.
///
/// Built rather than written out, so adding a kind to `ENTITY_KINDS` puts it in
/// this control with no edit here. `plural` because the control reads as "which
/// records" - a list of one thing is still called by its plural in a filter.
///
/// It used to be a `OnceLock`, built once per process. It cannot be any more,
/// and that is the point: the values are stable but the labels are not, and a
/// cache keyed on nothing would hand the second reader the first reader's
/// language.
fn kind_choices(catalog: &Catalog) -> Vec<FilterChoice> {
    let mut choices = vec![FilterChoice::all(l!("filter.all_records"))];

    choices.extend(
        ENTITY_KINDS
            .iter()
            .map(|kind| FilterChoice::new(kind.name, catalog.render(&kind.plural()))),
    );

    choices
}

/// The record, what was done to it, and its kind underneath.
fn record_cell(change: &EntityChange, catalog: &Catalog) -> impl IntoView {
    let record = change.record(catalog);
    let kind = change.kind_label(catalog);
    let action = change.action;
    let badge = catalog.render(&action.name());

    view! {
        <div>
            <div class="flex flex-wrap items-center gap-1.5">
                <span class="font-medium text-content">{record}</span>
                <Badge label=badge tone=tone(action) />
            </div>
            <span class="text-2xs text-content-subtle">{kind}</span>
        </div>
    }
}

/// How loudly to draw the verb.
///
/// A deletion is the one that cannot be undone by opening the record and
/// looking, so it is the one that carries a colour.
const fn tone(action: EntityAction) -> Tone {
    match action {
        EntityAction::Created => Tone::Success,
        EntityAction::Updated => Tone::Neutral,
        EntityAction::Deleted => Tone::Danger,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> GridConfig<EntityChange> {
        Owner::new().with(changes_grid)
    }

    /// The columns the DBAL will order by. Kept here as a literal rather than
    /// imported: `phonix-web` does not depend on `phonix-db`, and the point of
    /// the test is that the two lists were written to agree. The source is
    /// `phonix_db::audit::SORTABLE`.
    const SERVER_SORTS: &[&str] = &[
        "occurred_at",
        "entity_type",
        "action",
        "label",
        "actor_email",
    ];

    #[test]
    fn every_sortable_column_is_one_the_server_can_order_by() {
        for column in grid().columns.iter().filter(|column| column.sortable) {
            assert!(
                SERVER_SORTS.contains(&column.field()),
                "{} offers a sort the server cannot answer",
                column.field(),
            );
        }
    }

    #[test]
    fn the_searchable_columns_are_the_ones_the_where_clause_names() {
        let grid = grid();
        let searchable: Vec<&str> = grid
            .columns
            .iter()
            .filter(|c| c.searchable)
            .map(|c| c.field())
            .collect();

        // `phonix_db::audit::page` searches `label`, `actor_email` and
        // `entity_type`, and the placeholder says so: "Filter by record,
        // account or kind".
        assert_eq!(searchable, ["label", "entity_type", "actor_email"]);
    }

    #[test]
    fn the_kind_filter_offers_every_declared_kind() {
        // The list is built from `ENTITY_KINDS` rather than written out, so a
        // kind added there reaches this control with no edit here. The test is
        // that nobody has since replaced it with a literal.
        let grid = grid();
        let kinds = grid
            .filters
            .iter()
            .find(|filter| filter.key() == "kind")
            .unwrap();

        for kind in ENTITY_KINDS {
            assert!(
                kinds.choices.iter().any(|choice| choice.value == kind.name),
                "{} is declared but cannot be filtered for",
                kind.name,
            );
        }
    }

    #[test]
    fn every_verb_the_trail_can_store_can_be_filtered_for() {
        // The stored values are `EntityAction::as_str`, and a choice sending
        // anything else is a control that returns an empty list.
        let stored: Vec<&str> = [
            EntityAction::Created,
            EntityAction::Updated,
            EntityAction::Deleted,
        ]
        .iter()
        .map(|action| action.as_str())
        .collect();

        for value in stored {
            assert!(
                actions().iter().any(|choice| choice.value == value),
                "{value} can be recorded but not filtered for",
            );
        }
    }

    #[test]
    fn both_filters_leave_the_answering_to_the_server() {
        // A closure here could only narrow the twenty-five rows already
        // fetched, which is not what "only deletions" means.
        for filter in &grid().filters {
            assert!(
                !filter.is_local(),
                "{} is answered in the wrong place",
                filter.key()
            );
            assert_eq!(filter.default_value(), "");
        }
    }

    #[test]
    fn the_range_is_answered_by_the_server_and_named_what_the_reader_reads() {
        let grid = grid();
        let range = grid
            .date_filters
            .first()
            .expect("the grid offers a date range");

        // `phonix_db::audit::OCCURRED`. The two crates do not depend on each
        // other, so the agreement is written down twice and checked here.
        assert_eq!(range.key(), "occurred");
        assert!(!range.is_local());
    }

    #[test]
    fn it_opens_newest_first() {
        assert_eq!(
            grid().initial_request().sort,
            Some(Sort::descending("occurred_at"))
        );
    }

    #[test]
    fn nothing_can_be_done_to_a_change() {
        // A trail an administrator can edit is not one. Every action must be a
        // link; a `Run` would be something that changes a row.
        for action in &grid().actions {
            assert!(
                matches!(action.kind, crate::ui::table::action::ActionKind::Link(_)),
                "{} can change a trail entry",
                action.label,
            );
        }
    }

    #[test]
    fn opening_a_change_needs_the_permission_that_lists_them() {
        let grid = grid();
        let open = grid.actions.iter().find(|a| a.label == "Open").unwrap();

        assert_eq!(open.permission, Some(permissions::AUDIT_LOGS));
    }
}
