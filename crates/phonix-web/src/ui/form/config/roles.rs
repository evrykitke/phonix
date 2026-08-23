//! The role details form - the "Details" half of the role screen.
//!
//! # What this form is not
//!
//! It does not touch permissions. Those are the other tab, and they are saved
//! by their own button through `save_role_permissions`, which is where every
//! rule about a grant already lives. A form that wrote both would be a second
//! place for the tree's rules to be got wrong.
//!
//! # Two functions, one set of fields
//!
//! [`new_role_form`] and [`edit_role_form`] differ only in what happens after
//! the save: a role that has just been defined grants nothing, so its form
//! hands the reader straight to the permission tree, while an edit stays where
//! it is. The fields themselves are one private builder, because a field added
//! to one and not the other is the kind of difference nobody notices until the
//! column is empty.

use phonix_core::authorization::RoleInput;
use phonix_core::permissions;

use super::FormConfig;
use crate::icons::Icon;
use crate::l;
use crate::server_fns::admin_fns::save_role;
use crate::ui::alert::Channel;
use crate::ui::form::{Field, FieldValue, FormAction, Then};

/// The form for a role that does not exist yet.
pub fn new_role_form() -> FormConfig<RoleInput> {
    fields(FormConfig::new("role", save_role), false)
        .note(
            "A new role grants nothing until you choose its permissions, which is the next \
             step.",
        )
        .action(
            FormAction::submit(l!("roles.create"))
                .icon(Icon::Check)
                .then(Then::Say("Role created. Now choose what it grants."))
                // Straight to the tree. A role that grants nothing is not
                // finished, and leaving somebody on a saved form with no hint
                // of that is how empty roles get assigned to people.
                .then(Then::navigate(|role: &RoleInput| match role.id {
                    Some(id) => format!("/admin/roles/{id}?tab=permissions"),
                    // Unreachable - the service always returns the stored row -
                    // but the list is a better place to land than nowhere.
                    None => "/admin/roles".to_owned(),
                }))
                .require(permissions::ROLES_CREATE),
        )
}

/// The form for a role that exists.
///
/// `name_is_fixed` is true for `Admin` and `User`. Their key is what code
/// assigns them by, so the box is shown and disabled rather than hidden: a
/// missing field reads as a role that has no key, and the disabled one says
/// what it is and that it cannot change.
pub fn edit_role_form(name_is_fixed: bool) -> FormConfig<RoleInput> {
    fields(FormConfig::new("role", save_role), name_is_fixed)
        // A message box rather than the default toast, and the same one for a
        // failure: a role reaches everybody holding it the moment it is saved,
        // so "did that work?" is a question worth a click to answer rather than
        // a card that has already faded by the time the reader looks up from
        // the permissions tab.
        .reports(Channel::MessageBox)
        .action(
            FormAction::submit(l!("common.save"))
                .icon(Icon::Check)
                .then(Then::Say("Role saved."))
                .then(Then::Refresh)
                .require(permissions::ROLES_EDIT),
        )
}

/// The four fields, identical on both forms.
fn fields(config: FormConfig<RoleInput>, name_is_fixed: bool) -> FormConfig<RoleInput> {
    let name = Field::text("name", l!("field.key"), |role: &RoleInput| {
        FieldValue::text(&role.name)
    })
    .writing(|role, value| role.name = value.as_input())
    .required()
    .placeholder("Auditor")
    .help(l!("roles.key_help"));

    let name = if name_is_fixed { name.fixed() } else { name };

    config
        .single_column()
        .field(name)
        .field(
            Field::text("display_name", l!("roles.label"), |role: &RoleInput| {
                FieldValue::text(&role.display_name)
            })
            .writing(|role, value| role.display_name = value.as_input())
            .placeholder("Read only")
            .help(l!("roles.label_help"))
            .require(permissions::ROLES_EDIT),
        )
        .field(
            Field::multiline(
                "description",
                l!("field.description"),
                2,
                |role: &RoleInput| FieldValue::text(role.description.clone().unwrap_or_default()),
            )
            // A blank box is no description rather than an empty one, so the
            // list does not render a row of nothing under a name.
            .writing(|role, value| {
                let description = value.as_input();

                role.description =
                    Some(description).filter(|description| !description.trim().is_empty());
            })
            .full_width()
            .help(l!("roles.description_help"))
            .require(permissions::ROLES_EDIT),
        )
        .field(
            Field::toggle(
                "is_default",
                l!("roles.is_default"),
                // `Bool`, not the word "false" as text: an unticked box read
                // back as a non-empty string is a box that cannot be unticked.
                |role: &RoleInput| FieldValue::Bool(role.is_default),
            )
            .writing(|role, value| role.is_default = value.as_bool())
            .help(l!("roles.is_default_help"))
            .require(permissions::ROLES_EDIT),
        )
}

#[cfg(test)]
mod tests {
    use leptos::prelude::Owner;

    use super::*;
    use crate::ui::table::config::roles::roles_grid;

    fn draft() -> RoleInput {
        RoleInput {
            id: Some(uuid::Uuid::nil()),
            name: "Auditor".to_owned(),
            display_name: "Read only".to_owned(),
            description: Some("Sees the trail.".to_owned()),
            is_default: false,
        }
    }

    fn form() -> FormConfig<RoleInput> {
        Owner::new().with(|| edit_role_form(false))
    }

    /// The identifiers the form and the grid share have to agree.
    #[test]
    fn the_fields_that_name_a_column_name_a_real_one() {
        let grid = Owner::new().with(roles_grid);
        let columns: Vec<&str> = grid.columns.iter().map(|column| column.field()).collect();

        for shared in ["name", "display_name", "description", "is_default"] {
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
            let mut role = draft();
            let value = field.value(&role);

            field.apply(&mut role, &value);

            assert_eq!(
                field.value(&role),
                value,
                "{} does not round-trip",
                field.name()
            );
        }
    }

    #[test]
    fn a_description_typed_and_then_cleared_is_gone_rather_than_empty() {
        let form = form();
        let description = form
            .fields()
            .iter()
            .find(|f| f.name() == "description")
            .unwrap();

        let mut role = draft();
        description.apply(&mut role, &FieldValue::text("   "));

        assert_eq!(role.description, None);
    }

    #[test]
    fn the_key_is_the_one_field_that_is_always_required() {
        // A role with no key cannot be assigned; a role with no label falls
        // back to its key, and one with no description is ordinary.
        let form = form();
        let required: Vec<&str> = form
            .fields()
            .iter()
            .filter(|f| f.is_required())
            .map(|f| f.name())
            .collect();

        assert_eq!(required, ["name"]);
    }

    #[test]
    fn a_built_in_role_shows_its_key_and_cannot_change_it() {
        // Hidden instead would read as a role with no key at all.
        let fixed = Owner::new().with(|| edit_role_form(true));

        assert!(fixed.field_names().contains(&"name"));
        assert!(
            !fixed
                .fields()
                .iter()
                .find(|f| f.name() == "name")
                .unwrap()
                .editable_by(None)
        );
    }

    #[test]
    fn creating_a_role_hands_the_reader_to_the_permission_tree() {
        // A role that grants nothing is not finished, and a form that just
        // says "saved" is how an empty role gets assigned to somebody.
        let form = Owner::new().with(new_role_form);
        let submit = form
            .buttons()
            .into_iter()
            .find(|action| action.submits())
            .unwrap();

        let destination = submit
            .chain()
            .iter()
            .find_map(|then| match then {
                Then::Navigate(to) => Some(to(&draft())),
                _ => None,
            })
            .expect("the create form does not go anywhere");

        assert!(destination.contains("tab=permissions"), "{destination}");
    }

    #[test]
    fn editing_a_role_stays_where_it_is() {
        let form = form();
        let submit = form
            .buttons()
            .into_iter()
            .find(|action| action.submits())
            .unwrap();

        assert!(
            !submit
                .chain()
                .iter()
                .any(|then| matches!(then, Then::Navigate(_))),
            "the edit form walks away from itself",
        );
    }
}
