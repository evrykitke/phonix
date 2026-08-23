//! Roles: named bundles of permissions, owned by each workspace.
//!
//! Roles are rows, not an enum. `Admin` and `User` ship with every workspace
//! and cannot be removed; an organization may define as many more as it likes
//! and wire them to any subset of the permission tree.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::identity::FieldError;

use super::permission_set::PermissionSet;
use crate::msg;

/// The roles every workspace is created with.
pub mod names {
    /// Holds every permission. Assigned to whoever creates the workspace.
    pub const ADMIN: &str = "Admin";
    /// The default for everyone else.
    pub const USER: &str = "User";

    /// Roles that exist in every workspace and cannot be deleted or renamed.
    ///
    /// Deleting `Admin` would leave a workspace nobody can administer, and
    /// renaming it would break the code that assigns it at signup.
    pub const STATIC: &[&str] = &[ADMIN, USER];

    pub fn is_static(name: &str) -> bool {
        STATIC.iter().any(|known| known.eq_ignore_ascii_case(name))
    }
}

/// Longest a role name may be.
pub const MAX_ROLE_NAME_LEN: usize = 64;

/// Longest a role description may be.
///
/// It is shown in full in a grid cell and in the tab strip's subtitle, so
/// this is a layout limit as much as a storage one.
pub const MAX_ROLE_DESCRIPTION_LEN: usize = 240;

/// A role as the UI lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSummary {
    pub id: Uuid,
    /// The stable key used in code and in `user_roles`.
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    /// Ships with the product; cannot be deleted or renamed.
    pub is_static: bool,
    /// Automatically assigned to every new user in this workspace.
    pub is_default: bool,
    pub permission_count: i64,
    pub user_count: i64,
}

/// A role together with the permissions it grants, for the role editor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleDetail {
    pub summary: RoleSummary,
    pub permissions: PermissionSet,
}

/// What the role details form submits.
///
/// # One type for creating and for editing
///
/// `id` absent means "make this role"; present means "change that one". The
/// form is the same form either way - four fields, the same rules - and two
/// types would be two places to add the fifth field to.
///
/// # Why there are no permissions in here
///
/// A role's grants are submitted by the permission tree, through
/// `set_role_permissions`, and that endpoint is where every rule about them
/// lives: the ancestors a tick pulls in, the refusal to edit `Admin`, the audit
/// entry that records the difference. Letting a second endpoint write them
/// would be a second copy of all of it, and the copy that is used less is the
/// one that goes wrong quietly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleInput {
    /// `None` for a role that does not exist yet.
    pub id: Option<Uuid>,
    /// The stable key, stored in `roles.name` and matched case-insensitively.
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    /// Given automatically to every account created from now on. Does not
    /// reach the accounts that already exist.
    pub is_default: bool,
}

impl RoleInput {
    /// What the create form opens on.
    pub fn blank() -> Self {
        Self {
            id: None,
            name: String::new(),
            display_name: String::new(),
            description: None,
            is_default: false,
        }
    }

    /// Check the fields, and return them trimmed.
    ///
    /// `name_is_fixed` is true when the role being edited is a built-in one.
    /// `Admin` and `User` keep their names, because code assigns roles by that
    /// string, so their key is not checked against the reserved list it is
    /// itself on - which would refuse every save of a form nobody could change.
    /// Their label and description are ordinary editable text.
    pub fn validate(&self, name_is_fixed: bool) -> Result<ValidRole, Vec<FieldError>> {
        let mut errors = Vec::new();

        let name = if name_is_fixed {
            self.name.trim().to_owned()
        } else {
            match validate_role_name(&self.name) {
                Ok(name) => name,
                Err(err) => {
                    errors.push(err);
                    String::new()
                }
            }
        };

        let display_name = self.display_name.trim();
        let display_name = if display_name.is_empty() {
            // Falls back to the key rather than failing: a role called
            // "Auditor" needs no separate display name.
            name.clone()
        } else if display_name.chars().count() > MAX_ROLE_NAME_LEN {
            errors.push(FieldError::new(
                "display_name",
                msg!("validation.role.label_too_long", max = MAX_ROLE_NAME_LEN),
            ));
            String::new()
        } else {
            display_name.to_owned()
        };

        let description = self
            .description
            .as_ref()
            .map(|description| description.trim().to_owned())
            .filter(|description| !description.is_empty());

        if let Some(description) = &description
            && description.chars().count() > MAX_ROLE_DESCRIPTION_LEN
        {
            errors.push(FieldError::new(
                "description",
                msg!(
                    "validation.role.description_too_long",
                    max = MAX_ROLE_DESCRIPTION_LEN
                ),
            ));
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        Ok(ValidRole {
            id: self.id,
            name,
            display_name,
            description,
            is_default: self.is_default,
        })
    }
}

/// A [`RoleInput`] that has passed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidRole {
    pub id: Option<Uuid>,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub is_default: bool,
}

impl ValidRole {
    /// The form's own view of what was stored, for the round trip back.
    pub fn into_input(self) -> RoleInput {
        RoleInput {
            id: self.id,
            name: self.name,
            display_name: self.display_name,
            description: self.description,
            is_default: self.is_default,
        }
    }
}

/// Check a role name supplied by an administrator. Returns the trimmed name.
pub fn validate_role_name(raw: &str) -> Result<String, FieldError> {
    const FIELD: &str = "name";

    let trimmed = raw.trim();

    if trimmed.is_empty() {
        return Err(FieldError::new(
            FIELD,
            msg!("validation.role.name_required"),
        ));
    }
    if trimmed.chars().count() > MAX_ROLE_NAME_LEN {
        return Err(FieldError::new(
            FIELD,
            msg!("validation.role.name_too_long", max = MAX_ROLE_NAME_LEN),
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(FieldError::new(FIELD, msg!("validation.role.name_charset")));
    }
    // Case-insensitive, because role names are matched case-insensitively
    // everywhere else; allowing "admin" alongside "Admin" would produce two
    // roles that look identical in a list and behave differently in code.
    if names::is_static(trimmed) {
        return Err(FieldError::new(
            FIELD,
            msg!("validation.role.name_reserved", name = trimmed),
        ));
    }

    Ok(trimmed.to_owned())
}

/// Why an action was refused, for the audit trail and the error page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("missing permission: {required}")]
pub struct PermissionDenied {
    pub required: String,
}

impl PermissionDenied {
    pub fn new(required: impl Into<String>) -> Self {
        Self {
            required: required.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> RoleInput {
        RoleInput {
            id: None,
            name: "Auditor".into(),
            display_name: "Auditor".into(),
            description: None,
            is_default: false,
        }
    }

    #[test]
    fn static_roles_are_reserved() {
        assert!(names::is_static("Admin"));
        assert!(names::is_static("admin"), "matched case-insensitively");
        assert!(names::is_static("User"));
        assert!(!names::is_static("Auditor"));

        assert!(validate_role_name("Auditor").is_ok());
        assert_eq!(validate_role_name("  Auditor  ").unwrap(), "Auditor");
        // Would produce two roles that look the same in a list.
        assert!(validate_role_name("admin").is_err());
        assert!(validate_role_name("").is_err());
        assert!(validate_role_name(&"r".repeat(MAX_ROLE_NAME_LEN + 1)).is_err());
    }

    #[test]
    fn a_role_cannot_be_named_after_a_static_one() {
        let errors = RoleInput {
            name: "Admin".into(),
            ..draft()
        }
        .validate(false)
        .unwrap_err();

        assert_eq!(errors[0].field, "name");
    }

    #[test]
    fn a_built_in_role_is_not_refused_the_name_it_already_has() {
        // Its name is not editable, so checking it against the reserved list -
        // which it is itself on - would refuse every save of the form.
        let stored = RoleInput {
            name: "Admin".into(),
            display_name: "Owner".into(),
            ..draft()
        };

        assert_eq!(stored.validate(true).unwrap().display_name, "Owner");
    }

    #[test]
    fn an_empty_label_falls_back_to_the_key_rather_than_failing() {
        // A role called "Auditor" needs no separate display name.
        let role = RoleInput {
            display_name: "  ".into(),
            ..draft()
        }
        .validate(false)
        .unwrap();

        assert_eq!(role.name, "Auditor");
        assert_eq!(role.display_name, "Auditor");
    }

    #[test]
    fn a_blank_description_is_no_description_rather_than_an_empty_one() {
        let role = RoleInput {
            description: Some("   ".into()),
            ..draft()
        }
        .validate(false)
        .unwrap();

        assert_eq!(role.description, None);
    }

    #[test]
    fn everything_is_trimmed_on_the_way_through() {
        let role = RoleInput {
            name: "  Auditor  ".into(),
            display_name: "  Read only  ".into(),
            description: Some("  Sees the trail.  ".into()),
            ..draft()
        }
        .validate(false)
        .unwrap();

        assert_eq!(role.name, "Auditor");
        assert_eq!(role.display_name, "Read only");
        assert_eq!(role.description.as_deref(), Some("Sees the trail."));
    }

    #[test]
    fn an_overlong_description_is_reported_against_its_own_field() {
        let errors = RoleInput {
            description: Some("d".repeat(MAX_ROLE_DESCRIPTION_LEN + 1)),
            ..draft()
        }
        .validate(false)
        .unwrap_err();

        assert_eq!(errors[0].field, "description");
    }

    #[test]
    fn a_validated_role_round_trips_back_into_the_form() {
        // What the service returns is what the form re-opens on, so the two
        // shapes have to agree about every field.
        let input = RoleInput {
            id: Some(Uuid::nil()),
            is_default: true,
            ..draft()
        };

        assert_eq!(input.clone().validate(false).unwrap().into_input(), input);
    }
}
