//! The user edit form.
//!
//! The counterpart to [`ui::table::config::users`](crate::ui::table::config::users),
//! and the worked example for the next entity the way that one is for grids:
//! one function, one [`FormConfig`], no component written by hand.
//!
//! # Why the choices are a parameter
//!
//! `user_form` takes the workspace's roles rather than fetching them. A
//! configuration is built during render and cannot await anything, so a form
//! that fetched its own options would either block the render or draw an empty
//! select and fill it in later - which is a select that silently discards a
//! choice made in the first half-second. The screen resolves both the account
//! and the roles, then builds the configuration from what it has.
//!
//! # What this form deliberately cannot change
//!
//! Email, password and ownership - see [`UserEdit`] for why each one is a flow
//! of its own rather than a text box. Permissions are absent for a different
//! reason: roles are here, and individual overrides on top of them are their
//! own screen, because the effective set is computed and a flat list of tick
//! boxes would misrepresent where each answer came from.

use phonix_core::authorization::RoleSummary;
use phonix_core::identity::{UserEdit, UserStatus};
use phonix_core::permissions;

use super::FormConfig;
use crate::icons::Icon;
use crate::l;
use crate::server_fns::admin_fns::update_user;
use crate::ui::form::{Choice, Field, FieldValue, FormAction, Then};

/// Everything an administrator may change about somebody else's account.
pub fn user_form(roles: Vec<RoleSummary>) -> FormConfig<UserEdit> {
    FormConfig::new("user", update_user)
        .field(
            Field::text("first_name", l!("field.first_name"), |user: &UserEdit| {
                FieldValue::text(&user.first_name)
            })
            .writing(|user, value| user.first_name = value.as_input())
            .required()
            .require(permissions::USERS_EDIT),
        )
        .field(
            Field::text("last_name", l!("field.last_name"), |user: &UserEdit| {
                FieldValue::text(&user.last_name)
            })
            .writing(|user, value| user.last_name = value.as_input())
            .required()
            .require(permissions::USERS_EDIT),
        )
        .field(
            Field::select(
                "status",
                l!("field.status"),
                status_choices(),
                |user: &UserEdit| FieldValue::choice(user.status.as_str()),
            )
            // An unrecognised value keeps what was there rather than defaulting
            // to Active, which would be a suspension quietly lifted.
            .writing(|user, value| {
                if let Some(status) = value.as_choice().and_then(UserStatus::parse) {
                    user.status = status;
                }
            })
            .required()
            .require(permissions::USERS_EDIT)
            .help(l!("users.status_help")),
        )
        .field(
            Field::multi_select(
                "roles",
                l!("field.roles"),
                role_choices(roles),
                |user: &UserEdit| FieldValue::choices(user.roles.clone()),
            )
            .writing(|user, value| user.roles = value.as_set().into_iter().collect())
            // Roles are where most permissions come from, so changing them is a
            // permission change however it is spelled. The service requires the
            // same permission - this only stops the control being offered to
            // somebody who would be refused.
            .require(permissions::USERS_CHANGE_PERMISSIONS)
            .help(l!("users.roles_help")),
        )
        .action(
            FormAction::submit(l!("common.save"))
                .icon(Icon::Check)
                .then(Then::Say("Account saved."))
                // Does nothing on a page; refreshes the list when this same
                // configuration is opened in a modal over the grid.
                .then(Then::Refresh)
                .require(permissions::USERS_EDIT),
        )
}

/// The lifecycle states an administrator may choose.
///
/// `Pending` is offered because an invited account genuinely sits there, and
/// leaving it out would mean the select could not show the state the account is
/// actually in - a control that cannot represent its own value.
fn status_choices() -> Vec<Choice> {
    [
        (UserStatus::Active, l!("user.status.active.detail")),
        (UserStatus::Pending, l!("user.status.pending.detail")),
        (UserStatus::Suspended, l!("user.status.suspended.detail")),
        (
            UserStatus::Deactivated,
            l!("user.status.deactivated.detail"),
        ),
    ]
    .into_iter()
    // `as_str` is the stored value and stays English; `label` is the word.
    .map(|(status, detail)| {
        Choice::new(status.as_str(), crate::i18n::t(&status.label())).detail(detail)
    })
    .collect()
}

/// The workspace's roles, by name.
///
/// The *name* is the value, not the id: `UserEdit` holds names, the listing
/// shows names, and the grid searches them - so one spelling travels the whole
/// way and a form and a row cannot disagree about what somebody holds.
fn role_choices(roles: Vec<RoleSummary>) -> Vec<Choice> {
    roles
        .into_iter()
        .map(|role| {
            let choice = Choice::new(role.name, role.display_name);

            match role.description {
                Some(description) => choice.detail(description),
                None => choice,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use leptos::prelude::Owner;

    use super::*;
    use crate::ui::table::config::users::users_grid;

    fn roles() -> Vec<RoleSummary> {
        vec![RoleSummary {
            id: uuid::Uuid::nil(),
            name: "Admin".to_owned(),
            display_name: "Administrator".to_owned(),
            description: Some("Everything.".to_owned()),
            is_static: true,
            is_default: false,
            permission_count: 40,
            user_count: 1,
        }]
    }

    fn form() -> FormConfig<UserEdit> {
        Owner::new().with(|| user_form(roles()))
    }

    fn edit() -> UserEdit {
        UserEdit {
            id: phonix_core::identity::UserId::nil(),
            first_name: "Ada".to_owned(),
            last_name: "Lovelace".to_owned(),
            status: UserStatus::Active,
            roles: vec!["Admin".to_owned()],
        }
    }

    /// The identifiers the form and the grid share have to agree.
    ///
    /// Not *every* field: the grid shows one `display_name` where the form
    /// edits `first_name` and `last_name`, because a name is stored in two
    /// parts and read as one. The fields that do name a column are the ones a
    /// half-finished rename would break silently.
    #[test]
    fn the_fields_that_name_a_column_name_a_real_one() {
        let grid = Owner::new().with(users_grid);
        let columns: Vec<&str> = grid.columns.iter().map(|column| column.field()).collect();

        for shared in ["status", "roles"] {
            assert!(
                form().field_names().contains(&shared),
                "the form dropped {shared}"
            );
            assert!(columns.contains(&shared), "the grid dropped {shared}");
        }
    }

    #[test]
    fn every_field_reads_and_writes_the_same_place() {
        // A field whose writer does not put the value back where the reader
        // found it loses an edit on the next render, silently.
        for field in form().fields() {
            let mut draft = edit();
            let value = field.value(&draft);

            field.apply(&mut draft, &value);

            assert_eq!(
                field.value(&draft),
                value,
                "{} does not round-trip",
                field.name()
            );
        }
    }

    #[test]
    fn a_status_the_select_does_not_know_leaves_the_account_where_it_was() {
        // Otherwise an unrecognised value falls back to a default, and the
        // default that reads best - Active - is a suspension quietly lifted.
        let form = form();
        let status = form.fields().iter().find(|f| f.name() == "status").unwrap();

        let mut draft = edit();
        draft.status = UserStatus::Suspended;

        status.apply(&mut draft, &FieldValue::choice("not-a-status"));

        assert_eq!(draft.status, UserStatus::Suspended);
    }

    #[test]
    fn every_field_is_gated_and_the_roles_more_tightly_than_the_rest() {
        let form = form();

        for field in form.fields() {
            assert!(field.permission().is_some(), "{} is ungated", field.name());
        }

        let roles = form.fields().iter().find(|f| f.name() == "roles").unwrap();

        assert_eq!(
            roles.permission(),
            Some(permissions::USERS_CHANGE_PERMISSIONS)
        );
    }

    #[test]
    fn the_name_fields_are_required_and_the_roles_are_not() {
        // Somebody with no role is an account that can sign in and do nothing,
        // which is a legitimate thing to store. A nameless one is not.
        let form = form();
        let required: Vec<&str> = form
            .fields()
            .iter()
            .filter(|f| f.is_required())
            .map(|f| f.name())
            .collect();

        assert!(required.contains(&"first_name"));
        assert!(required.contains(&"last_name"));
        assert!(!required.contains(&"roles"));
    }
}
