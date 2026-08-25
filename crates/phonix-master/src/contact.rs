//! A person at a party: who to actually write to.
//!
//! # Why this is not just an email column on the party
//!
//! Because "accounts payable" and "the person who signs the order" are two
//! different addresses at one organization, and sending a payment reminder to
//! the second is how a reminder gets ignored. A party's own `email` stays - it
//! is the organization's front door - and these are the people behind it.
//!
//! # Deliberately small
//!
//! A name, an address, a phone, a note of what they do. Not a CRM: the moment
//! this grows opportunities and last-contacted dates it has stopped being
//! master data and started being an app.

use phonix_core::Message;
use phonix_core::msg;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Longest a contact's name or job title may be.
pub const MAX_CONTACT_LEN: usize = 120;

/// One person at a party.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartyContact {
    pub id: Uuid,
    pub party_id: Uuid,
    pub name: String,
    /// What they do there: "Accounts payable", "Site manager".
    pub job_title: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    /// Who a document goes to when nobody has said otherwise.
    pub is_primary: bool,
}

/// A contact being added or edited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartyContactInput {
    pub id: Option<Uuid>,
    pub name: String,
    pub job_title: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub is_primary: bool,
}

impl PartyContactInput {
    pub fn blank() -> Self {
        Self {
            id: None,
            name: String::new(),
            job_title: None,
            email: None,
            phone: None,
            is_primary: false,
        }
    }

    pub fn from_contact(contact: &PartyContact) -> Self {
        Self {
            id: Some(contact.id),
            name: contact.name.clone(),
            job_title: contact.job_title.clone(),
            email: contact.email.clone(),
            phone: contact.phone.clone(),
            is_primary: contact.is_primary,
        }
    }

    /// Trim, and say what is still wrong.
    pub fn check(&self) -> Result<Self, ContactError> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(ContactError::NameRequired);
        }
        if name.chars().count() > MAX_CONTACT_LEN {
            return Err(ContactError::NameTooLong);
        }

        let optional = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };

        let email = optional(&self.email);
        if let Some(address) = email.as_deref()
            && !crate::party::is_email_shaped(address)
        {
            return Err(ContactError::EmailShape);
        }

        let job_title = optional(&self.job_title);
        if job_title.is_some_and(|title| title.chars().count() > MAX_CONTACT_LEN) {
            return Err(ContactError::NameTooLong);
        }

        Ok(Self {
            id: self.id,
            name: name.to_owned(),
            job_title: optional(&self.job_title),
            email,
            phone: optional(&self.phone),
            is_primary: self.is_primary,
        })
    }
}

/// What can be wrong with a contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ContactError {
    #[error("a contact needs a name")]
    NameRequired,
    #[error("a contact's name is at most 120 characters")]
    NameTooLong,
    #[error("that does not look like an email address")]
    EmailShape,
}

impl ContactError {
    pub fn field(self) -> &'static str {
        match self {
            Self::NameRequired | Self::NameTooLong => "name",
            Self::EmailShape => "email",
        }
    }

    pub fn message(self) -> Message {
        match self {
            Self::NameRequired => msg!("party.error.contact_name_required"),
            Self::NameTooLong => msg!("party.error.contact_name_too_long"),
            Self::EmailShape => msg!("party.error.email_shape"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> PartyContactInput {
        PartyContactInput {
            name: "Ada Nyong'o".to_owned(),
            ..PartyContactInput::blank()
        }
    }

    #[test]
    fn a_contact_needs_a_name_and_nothing_else() {
        assert!(input().check().is_ok());
        assert_eq!(
            PartyContactInput {
                name: "  ".to_owned(),
                ..input()
            }
            .check(),
            Err(ContactError::NameRequired)
        );
    }

    #[test]
    fn an_address_that_cannot_receive_a_reminder_is_refused() {
        assert_eq!(
            PartyContactInput {
                email: Some("ada at acme".to_owned()),
                ..input()
            }
            .check(),
            Err(ContactError::EmailShape)
        );
    }

    #[test]
    fn a_blank_job_title_is_no_job_title() {
        let checked = PartyContactInput {
            job_title: Some("   ".to_owned()),
            ..input()
        }
        .check()
        .unwrap();

        assert_eq!(checked.job_title, None);
    }
}
