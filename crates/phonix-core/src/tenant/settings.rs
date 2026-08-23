//! What one organization has decided for itself.
//!
//! Every workspace gets a row of these at creation, seeded from the system
//! defaults, and an administrator may then change them. Nothing here is a
//! deployment concern: Argon2 cost, TOTP digits and the session ceiling belong
//! to `[security]` in the config file, because they depend on the hardware and
//! on decisions an organization is not in a position to make.
//!
//! The split, stated once:
//!
//! ```text
//! config/*.toml      ..  how expensive, how long, what shape   (the operator)
//! workspace_settings ..  how strict                            (the customer)
//! compiled constants ..  the floor neither may go under        (us)
//! ```

use serde::{Deserialize, Serialize};

use crate::audit::AuditPolicy;
use crate::identity::mfa::MfaPolicy;
use crate::identity::password::PasswordPolicy;
use crate::identity::validation::FieldError;

/// The security settings of one workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceSecuritySettings {
    pub password: PasswordPolicy,
    pub mfa: MfaPolicy,
    /// What this workspace records about itself, and for how long.
    ///
    /// Here rather than in a settings module of its own because it is saved by
    /// the same form and stored in the same row - and because deciding how much
    /// of your own history to keep is a security decision, read by the same
    /// person who sets the other two.
    pub audit: AuditPolicy,
}

impl WorkspaceSecuritySettings {
    /// What a workspace starts with on the day it is created.
    pub const fn system_default() -> Self {
        Self {
            password: PasswordPolicy::system_default(),
            mfa: MfaPolicy::system_default(),
            audit: AuditPolicy::system_default(),
        }
    }

    /// Check settings an administrator submitted.
    ///
    /// Returns every problem from both policies at once - they are one form.
    pub fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut errors = Vec::new();

        if let Err(mut password) = self.password.validate() {
            errors.append(&mut password);
        }
        if let Err(mut mfa) = self.mfa.validate() {
            errors.append(&mut mfa);
        }
        if let Err(mut audit) = self.audit.validate() {
            errors.append(&mut audit);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::mfa::MfaEnforcement;

    #[test]
    fn the_default_is_usable_as_it_stands() {
        let settings = WorkspaceSecuritySettings::system_default();
        assert!(settings.validate().is_ok());
        assert_eq!(settings.mfa.enforcement, MfaEnforcement::Optional);
        assert_eq!(settings.password.min_length, 12);
    }

    #[test]
    fn problems_from_both_policies_arrive_together() {
        // One form, one round trip: an administrator who got two things wrong
        // should not have to submit twice to find that out.
        let settings = WorkspaceSecuritySettings {
            password: PasswordPolicy {
                min_length: 4,
                ..PasswordPolicy::system_default()
            },
            mfa: MfaPolicy {
                enforcement: MfaEnforcement::Required,
                allow_totp: false,
                ..MfaPolicy::system_default()
            },
            audit: AuditPolicy {
                retention_days: Some(1),
                ..AuditPolicy::system_default()
            },
        };

        let errors = settings.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.field == "min_length"));
        assert!(errors.iter().any(|e| e.field == "allow_totp"));
        assert!(errors.iter().any(|e| e.field == "audit_retention_days"));
    }

    #[test]
    fn settings_round_trip_through_json() {
        let settings = WorkspaceSecuritySettings {
            password: PasswordPolicy {
                min_length: 16,
                ..PasswordPolicy::system_default()
            },
            mfa: MfaPolicy {
                enforcement: MfaEnforcement::Required,
                ..MfaPolicy::system_default()
            },
            audit: AuditPolicy::system_default().with_kind(crate::audit::kinds::USER, false),
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(
            serde_json::from_str::<WorkspaceSecuritySettings>(&json).unwrap(),
            settings
        );
        assert_eq!(
            serde_json::from_str::<WorkspaceSecuritySettings>("{}").unwrap(),
            WorkspaceSecuritySettings::system_default()
        );
    }
}
