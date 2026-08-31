//! The API keys grid.
//!
//! # What this list is read for
//!
//! Not "what keys exist" - a workspace with four keys knows that already. It is
//! read when something has changed and somebody has to decide what to stop:
//! after a laptop goes missing, when a contractor leaves, when a phone build is
//! retired. So the columns are the ones that answer *should this still be
//! here*: what it is called, who it acts as, how much it may do, and whether
//! anything has used it lately.
//!
//! # Revoked keys stay on the list
//!
//! A revoked row is history - "this credential existed, and it was stopped" -
//! and deleting it would take away the answer to a question somebody asks after
//! an incident rather than before one. So the list opens showing everything,
//! and the filter is how it is narrowed to what still works.
//!
//! # A key is never shown
//!
//! There is no column for the token and there cannot be: what is stored is a
//! digest, and [`ApiKeySummary`] carries four characters of hint so a person
//! can match a row against a configuration file they are holding.

use chrono::Utc;
use leptos::prelude::*;
use phonix_core::identity::{ApiKeySummary, KeyState};
use phonix_core::permissions;
use phonix_core::query::Sort;

use super::GridConfig;
use crate::components::page::{Badge, Tone};
use crate::icons::Icon;
use crate::server_fns::api_key_fns::{list_api_keys, revoke_api_key};
use crate::ui::table::{
    Align, Cell, Column, Filter, FilterChoice, RowAction, Source, ToolbarAction,
};
use crate::{l, lp};

/// Every credential that can reach this workspace from outside.
pub fn api_keys_grid() -> GridConfig<ApiKeySummary> {
    // Held rather than read per cell: a column's `read` runs wherever the
    // exporter calls it, with no reactive owner to take the context from.
    let catalog = crate::i18n::Locale::get().shared();

    GridConfig::new("api-keys", Source::paged(list_api_keys))
        .searching(l!("api_keys.search"))
        .exports_as("api-keys")
        // Newest first: the key somebody is asking about is usually the one
        // just made, and this is a column the server can order by.
        .sorted_by(Sort::descending("created_at"))
        .min_width("sm:min-w-[52rem]")
        .empty(
            Icon::KeySquare,
            l!("api_keys.empty.title"),
            l!("api_keys.empty.detail"),
        )
        // Answered in SQL - `phonix_db::identity::api_key::page` - because the
        // rows being narrowed are on the server. "Stopped" covers revoked and
        // expired together: to somebody tidying up they are one thing, a key
        // that no longer works.
        .filter(Filter::new(
            "revoked",
            l!("api_keys.filter.which"),
            vec![
                FilterChoice::all(l!("api_keys.filter.all")),
                FilterChoice::new("live", l!("api_keys.filter.live")),
                FilterChoice::new("revoked", l!("api_keys.filter.revoked")),
            ],
        ))
        .column(
            Column::new("name", l!("api_keys.field.name"), |key: &ApiKeySummary| {
                Cell::text(&key.name)
            })
            // Matched by the server's `ILIKE`, along with the owner's name.
            .searchable()
            .sortable()
            .pinned()
            // Without its name a row is a hint and a date.
            .essential()
            .render(|key| name_cell(key).into_any()),
        )
        .column({
            let words = catalog.clone();

            Column::new("state", l!("field.status"), move |key: &ApiKeySummary| {
                Cell::text(words.render(&key.state(Utc::now()).label()))
            })
            // Not sortable: the state is computed from two columns and a
            // clock, and there is no `ORDER BY` for it. The filter is how a
            // list is narrowed to one state.
            .align(Align::Center)
            .essential()
            .render(|key| state_cell(key).into_any())
        })
        .column(
            Column::new(
                "scopes",
                l!("api_keys.field.scopes"),
                |key: &ApiKeySummary| Cell::list(key.scopes.clone()),
            )
            .render(|key| scopes_cell(key).into_any()),
        )
        .column(
            Column::new("owner_name", l!("field.account"), |key: &ApiKeySummary| {
                Cell::text(&key.owner_name)
            })
            .searchable()
            .class("text-xs text-content-muted"),
        )
        .column(
            Column::new(
                "last_used_at",
                l!("field.last_used"),
                |key: &ApiKeySummary| {
                    // `Empty` rather than a dash: a key nothing has ever used
                    // is the one worth finding, and it sorts to one end
                    // instead of under the punctuation.
                    key.last_used_at.map_or(Cell::Empty, Cell::timestamp)
                },
            )
            .sortable()
            .class("whitespace-nowrap text-xs text-content-muted"),
        )
        .column(
            Column::new(
                "created_at",
                l!("field.created"),
                |key: &ApiKeySummary| Cell::timestamp(key.created_at),
            )
            .sortable()
            .class("whitespace-nowrap text-xs text-content-muted"),
        )
        .column(
            Column::new(
                "expires_at",
                l!("api_keys.field.expiry"),
                |key: &ApiKeySummary| key.expires_at.map_or(Cell::Empty, Cell::timestamp),
            )
            .sortable()
            .hidden(),
        )
        .toolbar(
            ToolbarAction::link(l!("api_keys.new"), Icon::Plus, "/admin/api-keys/new")
                .require(permissions::API_KEYS_CREATE)
                .primary(),
        )
        .action(
            RowAction::run(
                l!("api_keys.revoke"),
                Icon::Ban,
                |key: ApiKeySummary, grid| {
                    let label = key.name.clone();

                    leptos::task::spawn_local(async move {
                        match revoke_api_key(key.id).await {
                            Ok(()) => {
                                grid.report(l!("api_keys.revoked", name = label));
                                grid.refresh();
                            }
                            Err(err) => grid.warn(err.to_string()),
                        }
                    });
                },
            )
            // Offered only where it would do something. A key that is already
            // revoked cannot be revoked again, and a button for it is a button
            // that only ever produces an error.
            .when(ApiKeySummary::can_be_revoked)
            .require(permissions::API_KEYS_REVOKE)
            .tone(Tone::Danger)
            .confirm(l!("api_keys.revoke.confirm")),
        )
}

/// What it is called, and the four characters that identify it in a file.
fn name_cell(key: &ApiKeySummary) -> impl IntoView {
    let name = key.name.clone();
    let hint = key.hint.clone();

    view! {
        <div class="min-w-0">
            <span class="truncate-fade block font-medium text-content">{name}</span>
            // The only part of the token that exists anywhere after issue.
            <code class="text-2xs text-content-subtle">{format!("phx_...{hint}")}</code>
        </div>
    }
}

/// Working, expired or revoked - and only the first is unremarkable.
fn state_cell(key: &ApiKeySummary) -> impl IntoView {
    let state = key.state(Utc::now());

    let tone = match state {
        KeyState::Live => Tone::Success,
        // Expired is not a fault: it is a key that did what it was told. It
        // reads as neutral so that a revoked one stands out against it.
        KeyState::Expired => Tone::Neutral,
        KeyState::Revoked => Tone::Danger,
    };

    view! { <Badge label=crate::i18n::t(&state.label()) tone=tone /> }
}

/// How much this key may do, in words rather than as a list of dotted names.
///
/// A cell listing `Pages.Administration.Settings` reads as configuration
/// noise at a glance. The count answers the question the list is scanned for -
/// which of these can do a lot - and the names are in the export and on the
/// key's own row detail.
fn scopes_cell(key: &ApiKeySummary) -> impl IntoView {
    // `i64` because that is what a plural message counts in; a scope list long
    // enough to overflow it is not a thing.
    let count = key.scopes.len() as i64;
    let names = key.scopes.join(", ");

    view! {
        <span class="whitespace-nowrap" title=names>
            {if count == 0 {
                view! {
                    <span class="text-xs text-content-subtle">{l!("api_keys.scopes.none")}</span>
                }
                    .into_any()
            } else {
                view! { <span class="text-content-muted">{lp!("api_keys.scopes.count", count)}</span> }
                    .into_any()
            }}
        </span>
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    fn grid() -> GridConfig<ApiKeySummary> {
        Owner::new().with(api_keys_grid)
    }

    fn key(name: &str) -> ApiKeySummary {
        ApiKeySummary {
            id: uuid::Uuid::nil(),
            name: name.to_owned(),
            hint: "wxyz".to_owned(),
            scopes: Vec::new(),
            owner_name: "Ada Lovelace".to_owned(),
            created_at: Utc::now(),
            expires_at: None,
            last_used_at: None,
            revoked_at: None,
        }
    }

    #[test]
    fn every_sortable_column_is_one_the_server_can_order_by() {
        // The list is `phonix_db::identity::api_key::SORTABLE`. A sort naming
        // anything else is silently ignored, which reads as a column header
        // that does nothing when clicked.
        let orderable = ["name", "created_at", "last_used_at", "expires_at"];

        for column in grid().columns.iter().filter(|column| column.sortable) {
            assert!(
                orderable.contains(&column.field()),
                "{} is sortable here and not in SQL",
                column.field()
            );
        }
    }

    #[test]
    fn every_searchable_column_is_one_the_server_actually_matches() {
        // The query matches the key's name and its owner's display name. A
        // column marked searchable that the server ignores is a search box
        // that quietly drops what somebody typed.
        let matched = ["name", "owner_name"];

        for column in grid().columns.iter().filter(|column| column.searchable) {
            assert!(
                matched.contains(&column.field()),
                "{} is searchable here and not in SQL",
                column.field()
            );
        }
    }

    #[test]
    fn a_revoked_key_is_not_offered_a_revoke_button() {
        let grid = grid();
        let action = grid
            .actions
            .first()
            .expect("the grid offers revoking a key");

        let mut live = key("phone");
        assert!(action.applies_to(&live));

        live.revoked_at = Some(Utc::now() - Duration::hours(1));
        assert!(!action.applies_to(&live));
    }

    #[test]
    fn an_expired_key_can_still_be_stopped() {
        // Expiry is a clock, and a clock can move. Revoking an expired key is
        // what makes it permanently dead rather than dead until somebody
        // changes a date.
        let grid = grid();
        let action = grid.actions.first().expect("the revoke action");

        let mut expired = key("old build");
        expired.expires_at = Some(Utc::now() - Duration::days(1));

        assert!(action.applies_to(&expired));
    }

    #[test]
    fn the_state_a_row_reports_is_the_one_the_filter_offers() {
        // The filter's values reach `PageRequest.filters` and are answered in
        // SQL. If the vocabulary here drifted from `KeyState::as_str`, the
        // screen would narrow by a word the reader does not know.
        assert_eq!(KeyState::Live.as_str(), "live");
        assert_eq!(KeyState::Revoked.as_str(), "revoked");

        let filter = grid()
            .filters
            .first()
            .expect("the grid narrows by state")
            .clone();

        assert_eq!(filter.key(), "revoked");
    }
}
