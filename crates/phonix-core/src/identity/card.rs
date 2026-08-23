//! Who somebody is, for the places that only had their email address.
//!
//! # The problem this exists for
//!
//! An audit row records `actor_email`, and it is right that it does: the row
//! has to still name somebody after the account is deleted, so it stores the
//! address rather than joining to it. But an address is not a person. Reading a
//! trail full of `k.ndlovu@example.com` and working out who that is means
//! leaving the screen, going to the directory, searching, and coming back
//! having lost your place.
//!
//! A [`UserCard`] is the answer to "who is this", small enough to fetch on
//! demand for one row at a time and never as part of a list. Fetching it with
//! the page would be one query per row to answer a question about one of them.
//!
//! # Why it is not [`UserListing`](super::directory::UserListing)
//!
//! That is a row of a table: everything the users screen sorts, filters and
//! exports by, including things nobody wants in a hover card - failed sign-in
//! counts, lockout instants. This is what somebody wants to know when they ask
//! who edited a record, and no more. Two shapes because there are two
//! questions; sharing one would mean every future field on either being
//! argued about twice.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::user::{UserId, UserStatus};

/// Somebody, as a card beside their name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserCard {
    pub id: UserId,
    pub display_name: String,
    pub email: String,
    pub status: UserStatus,
    /// Created the workspace. Worth showing, because it explains why an account
    /// holds permissions nobody granted it.
    pub is_owner: bool,
    /// Role names, in the order the database returned them.
    pub roles: Vec<String>,
    /// The stored picture, if there is one.
    pub avatar_file_id: Option<Uuid>,
    /// Which part of the organization they belong to.
    ///
    /// Always `None` for now - nothing sets it yet. It is here rather than
    /// added later because the card is the screen that will want it first, and
    /// a field that renders as absent costs nothing until the day it is filled
    /// in. See [`Self::has_organizational_detail`].
    pub department: Option<String>,
    /// What they do. `None` for the same reason as [`Self::department`].
    pub job_title: Option<String>,
    pub last_login_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl UserCard {
    /// Initials for the avatar, matching
    /// [`UserListing::initials`](super::directory::UserListing::initials).
    ///
    /// The same rule, deliberately: two accounts of initials that disagree
    /// between the users table and this card would read as two people.
    pub fn initials(&self) -> String {
        let mut words = self
            .display_name
            .split_whitespace()
            .filter_map(|word| word.chars().next());

        match (words.next(), words.next()) {
            (Some(first), Some(second)) => format!("{first}{second}").to_uppercase(),
            (Some(first), None) => first.to_uppercase().to_string(),
            _ => self
                .email
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "?".to_owned()),
        }
    }

    /// Whether there is anything to draw under the "In the organization"
    /// heading yet.
    ///
    /// Nothing sets a department or a job title today, and a heading over two
    /// empty rows reads as data that failed to load rather than as data nobody
    /// has entered. When those fields are filled in, the section appears.
    pub fn has_organizational_detail(&self) -> bool {
        self.department.is_some() || self.job_title.is_some()
    }

    /// Where the account itself is edited.
    pub fn href(&self) -> String {
        format!("/admin/users/{}/edit", self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> UserCard {
        UserCard {
            id: UserId::nil(),
            display_name: "Amara Lekota".to_owned(),
            email: "amara@example.com".to_owned(),
            status: UserStatus::Active,
            is_owner: false,
            roles: vec!["Admin".to_owned()],
            avatar_file_id: None,
            department: None,
            job_title: None,
            last_login_at: None,
            created_at: chrono::DateTime::from_timestamp(1_770_000_000, 0).unwrap_or_default(),
        }
    }

    #[test]
    fn initials_come_from_the_display_name() {
        assert_eq!(card().initials(), "AL");
    }

    #[test]
    fn an_account_with_no_name_still_has_initials() {
        // An invited account that has not been through the form yet. A blank
        // circle where every other row has a letter reads as a broken row.
        let nameless = UserCard {
            display_name: String::new(),
            ..card()
        };

        assert_eq!(nameless.initials(), "A");
    }

    #[test]
    fn the_organization_section_stays_shut_until_something_fills_it() {
        // A heading over two empty rows reads as data that failed to load.
        assert!(!card().has_organizational_detail());

        let placed = UserCard {
            department: Some("Finance".to_owned()),
            ..card()
        };
        assert!(placed.has_organizational_detail());
    }
}
