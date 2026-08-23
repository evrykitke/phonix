//! The invitation form.
//!
//! Four fields and no password box - see
//! [`UserInvite`](phonix_core::identity::UserInvite) for why an administrator
//! never types somebody else's password.
//!
//! # This one saves into a different type than it edits
//!
//! Every other configuration is a `FormConfig<T>` that submits a `T` and gets a
//! `T` back. This one edits a [`UserInvite`] and the server returns an
//! [`InvitationIssued`] - a link, an expiry, and whether the email actually
//! went. So the submit closure maps the outcome back onto the draft and the
//! screen reads the result from its own signal instead.
//!
//! That is a seam worth noticing rather than papering over: a form whose result
//! is not the thing it edited is a real shape, and the alternative - inventing
//! a combined type so the generic parameter lines up - would put the link on
//! the draft, where the form would happily submit it back.

use leptos::prelude::Callable;
use phonix_core::authorization::RoleSummary;
use phonix_core::identity::UserInvite;
use phonix_core::permissions;

use super::FormConfig;
use crate::icons::Icon;
use crate::l;
use crate::ui::alert::Channel;
use crate::ui::form::{Choice, Field, FieldValue, FormAction, Then};

/// Add somebody to this workspace.
///
/// `on_issued` receives what the server minted, because the link is shown once
/// and the form has nowhere to put it.
pub fn invite_form(
    roles: Vec<RoleSummary>,
    on_issued: leptos::prelude::Callback<phonix_core::identity::InvitationIssued>,
) -> FormConfig<UserInvite> {
    FormConfig::new("invite", move |invite: UserInvite| {
        let on_issued = on_issued;

        async move {
            let outcome = crate::server_fns::admin_fns::invite_user(invite.clone()).await;

            match outcome {
                Ok(phonix_core::form::Submission::Saved(issued)) => {
                    on_issued.run(issued);

                    // The draft comes back unchanged. The form has served its
                    // purpose and the screen is now showing the link.
                    Ok(phonix_core::form::Submission::Saved(invite))
                }
                Ok(phonix_core::form::Submission::Rejected(errors)) => {
                    Ok(phonix_core::form::Submission::Rejected(errors))
                }
                Err(err) => Err(err),
            }
        }
    })
    .single_column()
    // Short enough to be on screen whole, so the outcome belongs beside the
    // button that caused it rather than in the corner of the window.
    .reports(Channel::Inline)
    .note(l!("invite.note"))
    .field(
        Field::email("email", l!("auth.signin.email"), |i: &UserInvite| {
            FieldValue::text(&i.email)
        })
        .writing(|i, value| i.email = value.as_input())
        .placeholder("someone@example.com")
        .help(l!("invite.email_help"))
        .require(permissions::USERS_CREATE)
        .required(),
    )
    .field(
        Field::text("first_name", l!("field.first_name"), |i: &UserInvite| {
            FieldValue::text(&i.first_name)
        })
        .writing(|i, value| i.first_name = value.as_input())
        .require(permissions::USERS_CREATE)
        .required(),
    )
    .field(
        Field::text("last_name", l!("field.last_name"), |i: &UserInvite| {
            FieldValue::text(&i.last_name)
        })
        .writing(|i, value| i.last_name = value.as_input())
        .require(permissions::USERS_CREATE)
        .required(),
    )
    .field(
        Field::multi_select(
            "roles",
            l!("field.roles"),
            role_choices(roles),
            |i: &UserInvite| FieldValue::choices(i.roles.clone()),
        )
        .writing(|i, value| i.roles = value.as_set().into_iter().collect())
        .help(l!("invite.roles_help"))
        // Same rule as the edit form: choosing roles is choosing permissions,
        // and the service asks for this too rather than trusting the control
        // being hidden.
        .require(permissions::USERS_CHANGE_PERMISSIONS),
    )
    .action(
        FormAction::submit(l!("invite.submit"))
            .icon(Icon::UserPlus)
            // Reset, not Close: "invite another" is the common next act, and a
            // form left sitting on a sent invitation invites sending it twice.
            .then(Then::Reset)
            .then(Then::Refresh)
            .require(permissions::USERS_CREATE),
    )
}

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
    use leptos::prelude::{Callback, Owner};

    use super::*;

    fn form() -> FormConfig<UserInvite> {
        Owner::new().with(|| invite_form(Vec::new(), Callback::new(|_| {})))
    }

    #[test]
    fn the_form_asks_for_an_address_a_name_and_nothing_resembling_a_password() {
        let names = form().field_names();

        assert_eq!(names, ["email", "first_name", "last_name", "roles"]);
        assert!(!names.iter().any(|name| name.contains("password")));
    }

    #[test]
    fn the_address_and_the_name_are_required_and_the_roles_are_not() {
        // No role is legitimate: the workspace's defaults apply, and an account
        // that can sign in and do nothing is a reasonable starting point.
        let form = form();
        let required: Vec<&str> = form
            .fields()
            .iter()
            .filter(|f| f.is_required())
            .map(|f| f.name())
            .collect();

        assert_eq!(required, ["email", "first_name", "last_name"]);
    }

    #[test]
    fn choosing_roles_needs_more_than_creating_the_account() {
        let form = form();
        let roles = form.fields().iter().find(|f| f.name() == "roles").unwrap();
        let email = form.fields().iter().find(|f| f.name() == "email").unwrap();

        assert_eq!(email.permission(), Some(permissions::USERS_CREATE));
        assert_eq!(
            roles.permission(),
            Some(permissions::USERS_CHANGE_PERMISSIONS)
        );
    }

    #[test]
    fn sending_resets_the_form_rather_than_leaving_it_on_a_sent_invitation() {
        // A form still holding the last invitation is one that gets sent twice.
        let form = form();
        let send = form.buttons().into_iter().next().unwrap();

        assert!(send.chain().iter().any(|then| matches!(then, Then::Reset)));
    }

    #[test]
    fn it_is_one_column_because_it_is_meant_to_be_narrow() {
        assert_eq!(form().columns, 1);
    }
}
