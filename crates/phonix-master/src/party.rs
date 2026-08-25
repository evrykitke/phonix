//! A party: an organization or a person the workspace trades with.
//!
//! # One table, many meanings
//!
//! A customer, a supplier, an agent and a carrier are the same row wearing
//! different hats, and in real trade they are routinely the same
//! *organization*: a company that buys from you and also delivers for you. Two
//! tables would mean two addresses to keep in step, two tax registrations, and
//! a document that cannot say the two are one party.
//!
//! So there is one table, and [`PartyRole`] is what an app claims about a row.
//! Books marks a party `"customer"`; Procurement marks the same party
//! `"supplier"`. Neither has to know the other exists, and neither owns the
//! party.
//!
//! # `kind` is not a role
//!
//! [`PartyKind`] is organization or person, and that is a fact about the party
//! rather than a claim about it: it decides whether "legal name" means anything,
//! what a tax registration looks like, and how the name is printed. It never
//! changes because an app started using the row.

use phonix_core::Message;
use phonix_core::locale::{Country, Currency};
use phonix_core::msg;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::address::{PartyAddress, PostalAddress};
use crate::contact::PartyContact;

/// Longest code the column holds.
pub const MAX_PARTY_CODE_LEN: usize = 30;

/// Longest name the column holds.
pub const MAX_PARTY_NAME_LEN: usize = 160;

/// Longest a role name may be.
pub const MAX_ROLE_LEN: usize = 32;

/// The roles this build's own apps claim.
///
/// An open vocabulary, so these are the well-known spellings rather than the
/// permitted set: an app added later claims its own without a migration here.
/// Written down so that two apps meaning the same thing spell it the same way.
pub mod roles {
    /// Somebody the workspace sells to.
    pub const CUSTOMER: &str = "customer";
    /// Somebody the workspace buys from.
    pub const SUPPLIER: &str = "supplier";
    /// Somebody who moves the goods.
    pub const CARRIER: &str = "carrier";
    /// Somebody who acts for the workspace, on commission.
    pub const AGENT: &str = "agent";
}

/// Whether the party is an organization or a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartyKind {
    /// A company, charity, authority or partnership.
    #[default]
    Organization,
    /// A natural person. A sole trader is one of these with a tax id.
    Person,
}

impl PartyKind {
    pub const ALL: &'static [Self] = &[Self::Organization, Self::Person];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Organization => "organization",
            Self::Person => "person",
        }
    }

    /// Read a stored value back.
    ///
    /// `None` rather than a default: the kind decides how a name is printed on
    /// a document and whether a legal name means anything, and guessing would
    /// put the wrong name on an invoice.
    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.as_str() == raw)
    }

    pub fn label(self) -> Message {
        match self {
            Self::Organization => msg!("party.kind.organization"),
            Self::Person => msg!("party.kind.person"),
        }
    }
}

/// A claim an app makes about a party.
///
/// A validated string rather than an enum, because the set is open: the whole
/// point is that an app added in two years' time claims its own role without a
/// migration in `master`. Validated because it reaches a `WHERE` clause and a
/// URL, and because a role with a space in it is a typo nobody can search for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PartyRole(String);

impl PartyRole {
    /// Build one, refusing anything that is not a bare snake_case word.
    pub fn parse(raw: &str) -> Result<Self, PartyError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(PartyError::RoleShape);
        }
        if raw.chars().count() > MAX_ROLE_LEN {
            return Err(PartyError::RoleShape);
        }
        if !raw.starts_with(|c: char| c.is_ascii_lowercase())
            || !raw
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(PartyError::RoleShape);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PartyRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One party, whole: everything a detail screen shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Party {
    pub id: Uuid,
    /// Short, unique case-insensitively, and what appears on a document beside
    /// the name. Assigned by the workspace, not generated - an accounts
    /// department that has used `ACME01` for twenty years is not going to stop.
    pub code: String,
    pub kind: PartyKind,
    /// What to call them. For a person, their name as they give it.
    pub name: String,
    /// The registered name, when it differs from what they are called.
    pub legal_name: Option<String>,
    pub tax_id: Option<String>,
    /// Where they are, for tax purposes. Not the same as any address: a party
    /// can be registered in one country and shipped to in another.
    pub country: Option<Country>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub website: Option<String>,
    /// What they are normally invoiced in. `None` means the workspace's own.
    pub currency: Option<Currency>,
    /// The tax treatment a new document reaches for. `None` means whatever the
    /// document's own default is - an exemption is a group, not an absence.
    pub tax_group_id: Option<Uuid>,
    pub is_active: bool,
    /// What the apps claim about this party. Sorted, so two reads of the same
    /// row compare equal.
    pub roles: Vec<PartyRole>,
    pub addresses: Vec<PartyAddress>,
    pub contacts: Vec<PartyContact>,
}

impl Party {
    /// The name to print on a document: the registered one where there is one.
    ///
    /// An invoice is a legal instrument, so it names the entity rather than the
    /// entity's trading style. The screen shows [`name`](Self::name); the
    /// document shows this.
    pub fn document_name(&self) -> &str {
        self.legal_name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&self.name)
    }

    /// Whether an app's claim is on this party.
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|held| held.as_str() == role)
    }

    /// The address a new document should use, for a purpose.
    ///
    /// The primary one for that purpose, or the first of that purpose, or - for
    /// shipping - the billing one, because "send it where the invoice goes" is
    /// the right default and a blank delivery address is not.
    pub fn address_for(&self, purpose: crate::address::AddressPurpose) -> Option<&PartyAddress> {
        let of_purpose = |wanted: crate::address::AddressPurpose| {
            self.addresses
                .iter()
                .filter(move |address| address.purpose == wanted)
        };

        of_purpose(purpose)
            .find(|address| address.is_primary)
            .or_else(|| of_purpose(purpose).next())
            .or_else(|| {
                (purpose == crate::address::AddressPurpose::Shipping)
                    .then(|| self.address_for(crate::address::AddressPurpose::Billing))
                    .flatten()
            })
    }

    /// The address to copy onto a document, as a value.
    pub fn postal_address(&self, purpose: crate::address::AddressPurpose) -> PostalAddress {
        self.address_for(purpose)
            .map(|address| address.address.clone())
            .unwrap_or_default()
    }
}

/// One party as a list row: what the grid shows, and nothing more.
///
/// A separate type from [`Party`] rather than a slimmer read of it, for the
/// reason `UserListing` is separate from `UserEdit`: a row is *rendered*, and a
/// list that carried every address would be a list that fetches four tables to
/// draw a name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartySummary {
    pub id: Uuid,
    pub code: String,
    pub kind: PartyKind,
    pub name: String,
    pub country: Option<Country>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub currency: Option<Currency>,
    pub is_active: bool,
    pub roles: Vec<PartyRole>,
}

/// A party being created or edited on a screen.
///
/// Addresses and contacts are not on it: they are their own rows, added from
/// their own panels, for the reason the organization's logo is not on the
/// organization form. A draft opened before somebody else corrected a postcode
/// would otherwise put the old one back as a side effect of a phone number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartyInput {
    pub id: Option<Uuid>,
    pub code: String,
    pub kind: PartyKind,
    pub name: String,
    pub legal_name: Option<String>,
    pub tax_id: Option<String>,
    pub country: Option<Country>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub website: Option<String>,
    pub currency: Option<Currency>,
    pub tax_group_id: Option<Uuid>,
    pub is_active: bool,
    pub roles: Vec<PartyRole>,
}

impl PartyInput {
    pub fn blank() -> Self {
        Self {
            id: None,
            code: String::new(),
            kind: PartyKind::Organization,
            name: String::new(),
            legal_name: None,
            tax_id: None,
            country: None,
            email: None,
            phone: None,
            website: None,
            currency: None,
            tax_group_id: None,
            is_active: true,
            roles: Vec::new(),
        }
    }

    /// A blank form for somebody an app already knows it wants.
    ///
    /// The "new customer" button on an invoice opens this, so the role is
    /// ticked before anybody sees the form rather than being one more thing to
    /// remember on a screen they did not come here to fill in.
    pub fn for_role(role: &str) -> Self {
        Self {
            roles: PartyRole::parse(role).into_iter().collect(),
            ..Self::blank()
        }
    }

    pub fn from_party(party: &Party) -> Self {
        Self {
            id: Some(party.id),
            code: party.code.clone(),
            kind: party.kind,
            name: party.name.clone(),
            legal_name: party.legal_name.clone(),
            tax_id: party.tax_id.clone(),
            country: party.country,
            email: party.email.clone(),
            phone: party.phone.clone(),
            website: party.website.clone(),
            currency: party.currency,
            tax_group_id: party.tax_group_id,
            is_active: party.is_active,
            roles: party.roles.clone(),
        }
    }

    /// Trim, sort the roles, and say what is still wrong.
    pub fn check(&self) -> Result<PartyInput, PartyError> {
        let code = self.code.trim();
        let name = self.name.trim();

        if code.is_empty() {
            return Err(PartyError::CodeRequired);
        }
        if code.chars().count() > MAX_PARTY_CODE_LEN {
            return Err(PartyError::CodeTooLong);
        }
        // A code reaches a printed document, a URL and a search box. Letters,
        // digits and the two separators; anything else is a paste gone wrong.
        if !code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(PartyError::CodeShape);
        }
        if name.is_empty() {
            return Err(PartyError::NameRequired);
        }
        if name.chars().count() > MAX_PARTY_NAME_LEN {
            return Err(PartyError::NameTooLong);
        }

        let optional = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };

        let email = optional(&self.email);
        // The same shape check the rest of the application uses. A party's
        // address is what an invitation to pay is sent to, so an address that
        // cannot receive one is worth catching where it is typed.
        if let Some(address) = email.as_deref()
            && !is_email_shaped(address)
        {
            return Err(PartyError::EmailShape);
        }

        // Sorted and de-duplicated: two reads of the same row have to compare
        // equal, or the audit trail records an edit every time somebody opens
        // the screen.
        let mut roles = self.roles.clone();
        roles.sort();
        roles.dedup();

        Ok(Self {
            id: self.id,
            code: code.to_owned(),
            kind: self.kind,
            name: name.to_owned(),
            legal_name: optional(&self.legal_name),
            tax_id: optional(&self.tax_id),
            country: self.country,
            email,
            phone: optional(&self.phone),
            website: optional(&self.website),
            currency: self.currency,
            tax_group_id: self.tax_group_id,
            is_active: self.is_active,
            roles,
        })
    }
}

/// The shape an address has to have to be worth sending to.
///
/// Deliberately loose - one `@`, something either side, a dot in the domain.
/// Anything stricter refuses addresses that work, and the only real test of an
/// address is sending to it.
pub(crate) fn is_email_shaped(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !value.contains(' ')
        && value.chars().count() <= 254
}

/// What can be wrong with a party somebody typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PartyError {
    #[error("a party needs a code")]
    CodeRequired,
    #[error("a party code is at most 30 characters")]
    CodeTooLong,
    #[error("a party code may contain only letters, digits, hyphens and underscores")]
    CodeShape,
    #[error("a party needs a name")]
    NameRequired,
    #[error("a party name is at most 160 characters")]
    NameTooLong,
    #[error("that does not look like an email address")]
    EmailShape,
    #[error("a role is a lowercase word, at most 32 characters")]
    RoleShape,
}

impl PartyError {
    /// Which control to attach the message to.
    pub fn field(self) -> &'static str {
        match self {
            Self::CodeRequired | Self::CodeTooLong | Self::CodeShape => "code",
            Self::NameRequired | Self::NameTooLong => "name",
            Self::EmailShape => "email",
            Self::RoleShape => "roles",
        }
    }

    pub fn message(self) -> Message {
        match self {
            Self::CodeRequired => msg!("party.error.code_required"),
            Self::CodeTooLong => msg!("party.error.code_too_long"),
            Self::CodeShape => msg!("party.error.code_shape"),
            Self::NameRequired => msg!("party.error.name_required"),
            Self::NameTooLong => msg!("party.error.name_too_long"),
            Self::EmailShape => msg!("party.error.email_shape"),
            Self::RoleShape => msg!("party.error.role_shape"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::{AddressPurpose, PostalAddress};

    fn input() -> PartyInput {
        PartyInput {
            code: "ACME01".to_owned(),
            name: "Acme Trading".to_owned(),
            ..PartyInput::blank()
        }
    }

    fn address(purpose: AddressPurpose, city: &str, is_primary: bool) -> PartyAddress {
        PartyAddress {
            id: Uuid::from_bytes([city.len() as u8; 16]),
            party_id: Uuid::nil(),
            purpose,
            label: None,
            address: PostalAddress {
                city: Some(city.to_owned()),
                ..PostalAddress::empty()
            },
            is_primary,
        }
    }

    fn party(addresses: Vec<PartyAddress>) -> Party {
        Party {
            id: Uuid::nil(),
            code: "ACME01".to_owned(),
            kind: PartyKind::Organization,
            name: "Acme".to_owned(),
            legal_name: Some("Acme Trading Limited".to_owned()),
            tax_id: None,
            country: None,
            email: None,
            phone: None,
            website: None,
            currency: None,
            tax_group_id: None,
            is_active: true,
            roles: vec![PartyRole::parse(roles::CUSTOMER).unwrap()],
            addresses,
            contacts: Vec::new(),
        }
    }

    #[test]
    fn a_document_names_the_entity_rather_than_the_trading_style() {
        // An invoice is a legal instrument. The screen shows "Acme"; the
        // document has to show "Acme Trading Limited".
        assert_eq!(party(Vec::new()).document_name(), "Acme Trading Limited");
    }

    #[test]
    fn a_party_with_no_registered_name_is_printed_as_what_it_is_called() {
        let party = Party {
            legal_name: None,
            ..party(Vec::new())
        };
        assert_eq!(party.document_name(), "Acme");

        // A blank one is not a name either.
        let party = Party {
            legal_name: Some("   ".to_owned()),
            ..party
        };
        assert_eq!(party.document_name(), "Acme");
    }

    #[test]
    fn a_role_is_a_bare_lowercase_word() {
        assert_eq!(
            PartyRole::parse("customer").unwrap().as_str(),
            roles::CUSTOMER
        );
        assert!(PartyRole::parse("sales_agent").is_ok());

        for bad in ["", "Customer", "sales agent", "customer;", "1customer"] {
            assert_eq!(PartyRole::parse(bad), Err(PartyError::RoleShape), "{bad:?}");
        }
        assert!(PartyRole::parse(&"a".repeat(MAX_ROLE_LEN + 1)).is_err());
    }

    #[test]
    fn one_party_wears_several_hats_at_once() {
        // The reason there is one table and not two: a company that buys from
        // you and also delivers for you is one party, and a document has to be
        // able to say so.
        let party = Party {
            roles: vec![
                PartyRole::parse(roles::CUSTOMER).unwrap(),
                PartyRole::parse(roles::CARRIER).unwrap(),
            ],
            ..party(Vec::new())
        };

        assert!(party.has_role(roles::CUSTOMER));
        assert!(party.has_role(roles::CARRIER));
        assert!(!party.has_role(roles::SUPPLIER));
    }

    #[test]
    fn roles_are_sorted_and_deduplicated_so_two_reads_compare_equal() {
        // Otherwise the audit trail records an edit every time somebody opens
        // the screen and presses save.
        let checked = PartyInput {
            roles: vec![
                PartyRole::parse(roles::SUPPLIER).unwrap(),
                PartyRole::parse(roles::CUSTOMER).unwrap(),
                PartyRole::parse(roles::SUPPLIER).unwrap(),
            ],
            ..input()
        }
        .check()
        .unwrap();

        let names: Vec<&str> = checked.roles.iter().map(PartyRole::as_str).collect();
        assert_eq!(names, vec![roles::CUSTOMER, roles::SUPPLIER]);
    }

    #[test]
    fn a_new_party_opened_from_an_invoice_arrives_already_a_customer() {
        let input = PartyInput::for_role(roles::CUSTOMER);
        assert_eq!(input.roles.len(), 1);
        assert!(input.roles.iter().any(|r| r.as_str() == roles::CUSTOMER));
    }

    #[test]
    fn the_two_required_fields_are_the_two_that_appear_on_a_document() {
        assert_eq!(
            PartyInput {
                code: "  ".to_owned(),
                ..input()
            }
            .check(),
            Err(PartyError::CodeRequired)
        );
        assert_eq!(
            PartyInput {
                name: String::new(),
                ..input()
            }
            .check(),
            Err(PartyError::NameRequired)
        );
    }

    #[test]
    fn a_code_that_would_print_badly_on_a_document_is_refused() {
        for bad in ["ACME 01", "ACME/01", "ACME'01"] {
            assert_eq!(
                PartyInput {
                    code: bad.to_owned(),
                    ..input()
                }
                .check(),
                Err(PartyError::CodeShape),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn an_address_that_cannot_receive_an_invoice_is_caught_where_it_is_typed() {
        assert_eq!(
            PartyInput {
                email: Some("acme.example".to_owned()),
                ..input()
            }
            .check(),
            Err(PartyError::EmailShape)
        );

        assert!(
            PartyInput {
                email: Some("billing@acme.example".to_owned()),
                ..input()
            }
            .check()
            .is_ok()
        );
    }

    #[test]
    fn a_blank_optional_field_is_stored_as_nothing() {
        let checked = PartyInput {
            tax_id: Some("   ".to_owned()),
            ..input()
        }
        .check()
        .unwrap();

        assert_eq!(checked.tax_id, None);
    }

    #[test]
    fn shipping_falls_back_to_where_the_invoice_goes() {
        // "Send it where the invoice goes" is the right default; a blank
        // delivery address is not.
        let party = party(vec![address(AddressPurpose::Billing, "Mombasa", true)]);

        let shipping = party.address_for(AddressPurpose::Shipping).unwrap();
        assert_eq!(shipping.address.city.as_deref(), Some("Mombasa"));
    }

    #[test]
    fn the_primary_address_wins_over_the_first_one() {
        let party = party(vec![
            address(AddressPurpose::Billing, "Nairobi", false),
            address(AddressPurpose::Billing, "Mombasa!", true),
        ]);

        let billing = party.address_for(AddressPurpose::Billing).unwrap();
        assert_eq!(billing.address.city.as_deref(), Some("Mombasa!"));
    }

    #[test]
    fn a_party_with_no_address_copies_a_blank_one_rather_than_failing() {
        let copied = party(Vec::new()).postal_address(AddressPurpose::Billing);
        assert!(copied.is_empty());
    }

    #[test]
    fn every_kind_round_trips_and_an_unknown_one_is_refused() {
        for kind in PartyKind::ALL {
            assert_eq!(PartyKind::parse(kind.as_str()), Some(*kind));
        }
        assert_eq!(PartyKind::parse("household"), None);
    }

    #[test]
    fn every_error_names_a_field_the_form_actually_has() {
        for error in [
            PartyError::CodeRequired,
            PartyError::CodeTooLong,
            PartyError::CodeShape,
            PartyError::NameRequired,
            PartyError::NameTooLong,
            PartyError::EmailShape,
            PartyError::RoleShape,
        ] {
            assert!(
                ["code", "name", "email", "roles"].contains(&error.field()),
                "{error:?} points at a field the form does not have",
            );
        }
    }
}
