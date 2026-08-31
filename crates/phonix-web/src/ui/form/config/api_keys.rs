//! The form that mints an API key.
//!
//! Three fields, and the middle one is the whole point: a key acts as the
//! person who issued it, so the only decision worth making carefully is how
//! much of that person it carries.
//!
//! # This one saves into a different type than it edits
//!
//! The same shape as [`invitations`](super::invitations), and for the same
//! reason: it edits an [`ApiKeyDraft`] and the server answers with an
//! [`ApiKeyIssued`] - a token that is shown once and never again. So the submit
//! closure hands the result to the screen through a callback and returns the
//! draft unchanged. Putting the token on the draft would put it somewhere the
//! form could submit it back.
//!
//! # The scope choices are what the issuer holds
//!
//! Not the whole tree. Offering `Users.Delete` to somebody who cannot delete a
//! user is offering a choice the service will refuse - and the refusal, though
//! correct, reads as a bug in the screen. The service asks anyway: this is a
//! narrowed list, not a control.

use leptos::prelude::Callable;
use phonix_core::authorization::DEFINITIONS;
use phonix_core::form::Submission;
use phonix_core::identity::{ApiKeyDraft, ApiKeyIssued, AuthUser};
use phonix_core::permissions;

use super::FormConfig;
use crate::icons::Icon;
use crate::l;
use crate::ui::alert::Channel;
use crate::ui::form::{Choice, Field, FieldValue, FormAction, Then};

/// Mint a key.
///
/// `on_issued` receives what the server minted, because the token is shown once
/// and the form has nowhere to put it.
pub fn api_key_form(
    scopes: Vec<Choice>,
    on_issued: leptos::prelude::Callback<ApiKeyIssued>,
) -> FormConfig<ApiKeyDraft> {
    FormConfig::new("api-key", move |draft: ApiKeyDraft| {
        let on_issued = on_issued;

        async move {
            match crate::server_fns::api_key_fns::issue_api_key(draft.clone()).await {
                Ok(Submission::Saved(issued)) => {
                    on_issued.run(issued);

                    // The draft comes back unchanged. The form has done its
                    // job and the screen is now showing the key.
                    Ok(Submission::Saved(draft))
                }
                Ok(Submission::Rejected(errors)) => Ok(Submission::Rejected(errors)),
                Err(err) => Err(err),
            }
        }
    })
    .single_column()
    // Short enough to be on screen whole, so the outcome belongs beside the
    // button that caused it rather than in the corner of the window.
    .reports(Channel::Inline)
    .note(l!("api_keys.new.note"))
    .field(
        Field::text("name", l!("api_keys.field.name"), |draft: &ApiKeyDraft| {
            FieldValue::text(&draft.name)
        })
        .writing(|draft, value| draft.name = value.as_input())
        .placeholder("iOS app")
        .help(l!("api_keys.field.name_help"))
        .require(permissions::API_KEYS_CREATE)
        .required(),
    )
    .field(
        Field::multi_select(
            "scopes",
            l!("api_keys.field.scopes"),
            scopes,
            |draft: &ApiKeyDraft| FieldValue::choices(draft.scopes.clone()),
        )
        .writing(|draft, value| draft.scopes = value.as_set().into_iter().collect())
        .help(l!("api_keys.field.scopes_help"))
        .require(permissions::API_KEYS_CREATE),
    )
    .field(
        Field::select(
            "expires_in_days",
            l!("api_keys.field.expiry"),
            expiry_choices(),
            |draft: &ApiKeyDraft| {
                FieldValue::choice(
                    draft
                        .expires_in_days
                        .map_or_else(|| NEVER.to_owned(), |days| days.to_string()),
                )
            },
        )
        // Anything that is not a number is "never", which is what `NEVER` is
        // and what a control from an older build would send.
        .writing(|draft, value| {
            draft.expires_in_days = value.as_choice().and_then(|choice| choice.parse().ok())
        })
        .help(l!("api_keys.field.expiry_help"))
        .require(permissions::API_KEYS_CREATE),
    )
    .action(
        FormAction::submit(l!("api_keys.new.submit"))
            .icon(Icon::KeySquare)
            // Reset, not Close: the key is now on screen beside the form, and
            // a form that closed itself would take the reader away from the
            // one thing they have to copy before leaving.
            .then(Then::Reset)
            .require(permissions::API_KEYS_CREATE),
    )
}

/// The value that means "no expiry".
///
/// A word rather than an empty string, because a select's empty value is
/// already what "nothing chosen" looks like and these are different answers.
const NEVER: &str = "never";

fn expiry_choices() -> Vec<Choice> {
    vec![
        Choice::new(NEVER, l!("api_keys.expiry.never")),
        Choice::new("30", l!("api_keys.expiry.30")),
        Choice::new("90", l!("api_keys.expiry.90")),
        Choice::new("365", l!("api_keys.expiry.365")),
    ]
}

/// The permissions this person could put on a key, as choices.
///
/// Every node they hold, in tree order, labelled by the dotted name it will be
/// stored as. The display name alone would be ambiguous - there are four
/// permissions called "Create" - and the dotted name is what a person reading
/// the key's row later will see.
pub fn scope_choices(user: &AuthUser) -> Vec<Choice> {
    DEFINITIONS
        .iter()
        .filter(|definition| user.can(definition.name))
        .map(|definition| {
            let choice = Choice::new(definition.name, definition.name);

            match definition.description {
                Some(description) => choice.detail(description),
                None => choice.detail(definition.display_name),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use leptos::prelude::{Callback, Owner};
    use phonix_core::PermissionSet;
    use phonix_core::identity::UserStatus;
    use phonix_core::permissions as names;

    use super::*;

    fn form() -> FormConfig<ApiKeyDraft> {
        Owner::new().with(|| api_key_form(Vec::new(), Callback::new(|_| {})))
    }

    fn user_holding(granted: &[&str]) -> AuthUser {
        let mut permissions = PermissionSet::new();
        for name in granted {
            permissions.grant(name);
        }

        AuthUser {
            id: uuid::Uuid::nil(),
            email: "ada@example.com".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            display_name: "Ada Lovelace".into(),
            roles: vec!["Admin".into()],
            permissions,
            is_owner: false,
            status: UserStatus::Active,
            mfa_enabled: false,
            mfa_satisfied: true,
            email_verified: true,
        }
    }

    #[test]
    fn the_form_writes_every_field_of_the_draft() {
        assert_eq!(
            form().field_names(),
            vec!["name", "scopes", "expires_in_days"]
        );
    }

    #[test]
    fn an_expiry_round_trips_through_the_control() {
        let form = form();
        let expiry = form
            .fields()
            .iter()
            .find(|field| field.name() == "expires_in_days")
            .expect("the expiry field");

        let mut draft = ApiKeyDraft::blank();
        assert_eq!(expiry.value(&draft).as_choice(), Some(NEVER));

        draft.expires_in_days = Some(90);
        assert_eq!(expiry.value(&draft).as_choice(), Some("90"));

        // And back the other way, including the value a stale control sends.
        expiry.apply(&mut draft, &FieldValue::choice(NEVER));
        assert_eq!(draft.expires_in_days, None);

        expiry.apply(&mut draft, &FieldValue::choice("30"));
        assert_eq!(draft.expires_in_days, Some(30));

        expiry.apply(&mut draft, &FieldValue::choice("a fortnight"));
        assert_eq!(draft.expires_in_days, None, "nonsense is not an expiry");
    }

    #[test]
    fn a_person_is_only_offered_what_they_hold() {
        let choices = scope_choices(&user_holding(&[names::SETTINGS]));
        let offered: Vec<&str> = choices.iter().map(|choice| choice.value.as_str()).collect();

        // `grant` carries the ancestors, so holding Settings means holding the
        // path down to it - and every one of those is a scope worth offering.
        assert!(offered.contains(&names::SETTINGS));
        assert!(offered.contains(&names::ADMINISTRATION));
        // Nothing else, and this is the half that matters: a key cannot be
        // given a permission its issuer does not have, and the screen should
        // not pretend otherwise.
        assert!(!offered.contains(&names::USERS_DELETE));
    }

    #[test]
    fn a_half_authenticated_person_is_offered_nothing() {
        // `AuthUser::can` is false for everything until the second factor is
        // satisfied, and this list is built from `can` rather than from the
        // permission set for exactly that reason.
        let mut user = user_holding(&[names::SETTINGS]);
        user.mfa_enabled = true;
        user.mfa_satisfied = false;

        assert!(scope_choices(&user).is_empty());
    }
}
