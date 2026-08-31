//! The users grid.
//!
//! # The worked example
//!
//! This is what a module contributes: one function returning one
//! [`GridConfig`]. It uses every part of the kit, so it doubles as the
//! reference for the next entity:
//!
//! * columns with and without renderers, searchable, sortable, hidden
//! * an in-memory source, because a workspace's people fit in one fetch
//! * a link action and a destructive `run` action, each permission-gated
//! * an export, and a column menu
//!
//! # Why the list is in memory
//!
//! A workspace has as many accounts as it has people. Fetching all of them once
//! and filtering in the browser makes searching instant and costs one request;
//! paging it would cost a round trip per keystroke to save memory nobody is
//! short of. When a listing arrives that cannot make that claim - stock
//! movements, audit events over a year - it uses [`Source::paged`] instead, and
//! nothing else about its configuration changes.
//!
//! # Where the two "Person" values went
//!
//! The person column *shows* name, avatar and email together, because that is
//! how a row is recognised. It *reads* as the display name alone, so sorting by
//! it sorts by name. Email is its own column, hidden by default, so it can be
//! searched, sorted and exported without being drawn twice.

use std::sync::Arc;

use leptos::prelude::*;
use phonix_core::identity::{UserListing, UserStatus};
use phonix_core::permissions;
use phonix_core::query::Sort;

use super::GridConfig;
use crate::components::page::{Badge, Tone};
use crate::i18n::{Locale, t};
use crate::icons::Icon;
use crate::l;
use crate::server_fns::admin_fns::{list_users, resend_invitation, reset_user_mfa};
use crate::ui::table::{Align, Cell, Column, RowAction, Source, ToolbarAction};

/// Everyone in this workspace.
pub fn users_grid() -> GridConfig<UserListing> {
    // Held rather than read per row: a cell closure runs once per row and, on
    // the export path, with no reactive owner to read the context from.
    let catalog = Locale::get().shared();

    GridConfig::new("users", Source::in_memory(list_users))
        .searching(l!("users.search"))
        .exports_as("users")
        .sorted_by(Sort::ascending("display_name"))
        .min_width("sm:min-w-[48rem]")
        .empty(
            Icon::Users,
            l!("users.empty.title"),
            l!("users.empty.detail"),
        )
        .column(
            Column::new("display_name", l!("field.person"), |user: &UserListing| {
                Cell::text(&user.display_name)
            })
            .findable()
            // Without a name a row is an anonymous set of badges, so this one
            // is not on offer in the column menu.
            .pinned()
            // The one column worth a phone screen: it already carries the
            // email under the name.
            .essential()
            .render(|user| person_cell(user).into_any()),
        )
        .column(
            Column::new("email", l!("field.email"), |user: &UserListing| {
                Cell::text(&user.email)
            })
            .findable()
            // Already under the name in the person column; here so it can
            // be sorted and exported on its own.
            .hidden(),
        )
        .column(
            Column::new("roles", l!("field.roles"), |user: &UserListing| {
                Cell::list(user.roles.clone())
            })
            .searchable()
            .render(|user| roles_cell(user).into_any()),
        )
        .column(
            Column::new("status", l!("field.status"), {
                let words = Arc::clone(&catalog);
                move |user: &UserListing| Cell::text(words.render(&user.status.label()))
            })
            .findable()
            // The reason most people open this list on a phone: who is active,
            // who is locked out, who still has to verify.
            .essential()
            .render(|user| status_cell(user).into_any()),
        )
        .column(
            Column::new("mfa_enabled", l!("field.mfa"), |user: &UserListing| {
                Cell::bool(user.mfa_enabled)
            })
            .sortable()
            .align(Align::Center)
            .hidden(),
        )
        .column(
            Column::new("created_at", l!("field.added"), |user: &UserListing| {
                Cell::timestamp(user.created_at)
            })
            .sortable()
            .hidden(),
        )
        .column(
            Column::new(
                "last_login_at",
                l!("field.last_sign_in"),
                |user: &UserListing| {
                    // `Empty` rather than the word "Never": it sorts to one end
                    // instead of under N, and exports as blank.
                    user.last_login_at.map_or(Cell::Empty, Cell::timestamp)
                },
            )
            .sortable()
            .class("whitespace-nowrap text-xs text-content-muted"),
        )
        .toolbar(
            ToolbarAction::link(l!("users.invite"), Icon::UserPlus, "/admin/users/invite")
                .require(permissions::USERS_CREATE)
                .primary(),
        )
        .action(
            RowAction::link(l!("common.edit"), Icon::Pencil, |user: &UserListing| {
                format!("/admin/users/{}/edit", user.id)
            })
            .require(permissions::USERS_EDIT),
        )
        .action(
            RowAction::link(
                l!("permissions.title"),
                Icon::KeySquare,
                |user: &UserListing| format!("/admin/users/{}/permissions", user.id),
            )
            .require(permissions::USERS_CHANGE_PERMISSIONS),
        )
        .action(
            RowAction::run(l!("users.resend"), Icon::Mail, |user: UserListing, grid| {
                let name = user.display_name.clone();

                leptos::task::spawn_local(async move {
                    match resend_invitation(user.id).await {
                        Ok(issued) => {
                            // The relay's answer, not a house phrase: when it
                            // did not send, the link is the only way in and the
                            // grid has nowhere to show one.
                            match issued.delivery_note {
                                None => grid.report(l!("users.resend.sent", name = name)),
                                Some(note) => {
                                    grid.warn(l!("users.resend.undelivered", note = note))
                                }
                            }
                        }
                        Err(err) => grid.warn(err.to_string()),
                    }
                });
            })
            // Only where it would do something: an account that has accepted
            // has a password, and re-inviting it would mint a link that sets
            // one - which is an account takeover with a friendly name.
            .when(|user: &UserListing| user.status == UserStatus::Pending)
            .require(permissions::USERS_CREATE)
            .confirm(l!("users.resend.confirm")),
        )
        .action(
            RowAction::run(
                l!("users.reset_mfa"),
                Icon::ShieldOff,
                |user: UserListing, grid| {
                    let name = user.display_name.clone();

                    leptos::task::spawn_local(async move {
                        match reset_user_mfa(user.id).await {
                            Ok(_) => {
                                grid.report(l!("users.reset_mfa.done", name = name));
                                grid.refresh();
                            }
                            // The server's own words: it knows whether this was
                            // refused because of a policy or because of permission,
                            // and either is worth reading.
                            Err(err) => grid.warn(err.to_string()),
                        }
                    });
                },
            )
            // Offered only where it would do something - see the doc on
            // `RowAction::when`.
            .when(|user: &UserListing| user.mfa_enabled)
            .require(permissions::USERS_EDIT)
            .tone(Tone::Danger)
            .confirm(
                "Remove every second factor from this account? \
                 They will sign in with a password alone until they enrol again.",
            ),
        )
}

/// Avatar, name, owner badge, email.
fn person_cell(user: &UserListing) -> impl IntoView {
    let initials = user.initials();
    let display_name = user.display_name.clone();
    let email = user.email.clone();
    let is_owner = user.is_owner;

    view! {
        <div class="flex items-center gap-2">
            <span
                class="grid size-7 shrink-0 place-items-center rounded-full bg-surface-sunken text-2xs font-semibold text-content-muted"
                aria-hidden="true"
            >
                {initials}
            </span>
            <div class="min-w-0">
                <div class="flex items-center gap-1.5">
                    <span class="truncate-fade font-medium text-content">{display_name}</span>
                    {is_owner.then(|| view! { <Badge label=l!("account.badge.owner") tone=Tone::Brand /> })}
                </div>
                <div class="truncate-fade text-xs text-content-subtle">{email}</div>
            </div>
        </div>
    }
}

fn roles_cell(user: &UserListing) -> impl IntoView {
    let roles = user.roles.clone();

    view! {
        <div class="flex flex-wrap gap-1">
            {if roles.is_empty() {
                view! { <span class="text-xs text-content-subtle">{l!("users.no_roles")}</span> }
                    .into_any()
            } else {
                roles
                    .into_iter()
                    .map(|role| view! { <Badge label=role /> })
                    .collect::<Vec<_>>()
                    .into_any()
            }}
        </div>
    }
}

/// Status, plus the flags that change what the account can actually do.
fn status_cell(user: &UserListing) -> impl IntoView {
    let status = user.status;
    // Carried on the row, not compared against `Utc::now()` here. This decides
    // whether a badge exists, and a clock read during a render is read at two
    // different moments on the two sides of hydration - a lockout expiring in
    // that gap makes the node counts disagree, which freezes the page. See
    // `UserListing::locked`.
    let locked = user.locked;
    let mfa_enabled = user.mfa_enabled;
    let unverified = !user.email_verified;

    view! {
        <div class="flex flex-wrap items-center gap-1">
            <Badge label=t(&status.label()) tone=status_tone(status) />
            {locked
                .then(|| {
                    view! {
                        <Badge label=l!("users.badge.locked") tone=Tone::Warning icon=Icon::Lock />
                    }
                })}
            {mfa_enabled
                .then(|| {
                    view! {
                        // The acronym, which is the same one in all three
                        // languages this ships in - and still a key, so a
                        // fourth is free to disagree.
                        <Badge label=l!("field.mfa") tone=Tone::Success icon=Icon::ShieldCheck />
                    }
                })}
            {unverified
                .then(|| view! { <Badge label=l!("users.badge.unverified") tone=Tone::Warning /> })}
        </div>
    }
}

const fn status_tone(status: UserStatus) -> Tone {
    match status {
        UserStatus::Active => Tone::Success,
        UserStatus::Pending => Tone::Warning,
        UserStatus::Suspended | UserStatus::Deactivated => Tone::Danger,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The configuration, built inside a reactive owner.
    ///
    /// The "Reset 2FA" action stores a `Callback`, which leptos allocates in an
    /// arena belonging to the current owner. A component always has one; a test
    /// has to make one.
    fn grid() -> GridConfig<UserListing> {
        Owner::new().with(users_grid)
    }

    /// The configuration is data, so what it promises can simply be asserted.
    #[test]
    fn the_search_box_looks_in_the_columns_the_placeholder_names() {
        let grid = grid();
        let searchable: Vec<&str> = grid
            .columns
            .iter()
            .filter(|c| c.searchable)
            .map(|c| c.field())
            .collect();

        // "Filter by name, email or role".
        assert!(searchable.contains(&"display_name"));
        assert!(searchable.contains(&"email"));
        assert!(searchable.contains(&"roles"));
    }

    #[test]
    fn the_column_that_identifies_a_row_cannot_be_hidden() {
        let grid = grid();
        let person = grid
            .columns
            .iter()
            .find(|c| c.field() == "display_name")
            .unwrap();

        assert!(!person.hideable);
    }

    #[test]
    fn every_action_names_a_permission() {
        let grid = grid();

        for action in &grid.actions {
            assert!(action.permission.is_some(), "{} is ungated", action.label);
        }
    }

    #[test]
    fn the_destructive_action_asks_first() {
        let grid = grid();
        let reset = grid
            .actions
            .iter()
            .find(|a| a.label == "Reset 2FA")
            .unwrap();

        assert!(reset.confirm.is_some());
        assert_eq!(reset.tone, Tone::Danger);
    }

    #[test]
    fn it_opens_sorted_by_name() {
        assert_eq!(
            grid().initial_request().sort,
            Some(Sort::ascending("display_name"))
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
