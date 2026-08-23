//! Reading and changing who the organization says it is.
//!
//! Two things happen here that a repository must not do on its own: the profile
//! is validated field by field before it is written, and the change is recorded
//! on the change trail as a `{from, to}` pair. The legal name, the registration
//! number and the address are what appear on everything this workspace issues,
//! so "who changed it, and to what" is the question asked after a document goes
//! out wrong - and a bare `UPDATE` cannot answer it.
//!
//! # The logo is not saved here
//!
//! [`save`] does not touch it. Attaching a logo is
//! [`files::set_logo`](crate::files::set_logo), because it is a file operation:
//! it has to check that the upload finished and was accepted, and it has to
//! delete the image it replaces. What this module does is *read* it back, so
//! the screen can show what is currently set.

use phonix_core::form::{Submission, rejected};
use phonix_core::organization::OrganizationProfile;
use phonix_core::permissions;
use phonix_db::organization as store;
use phonix_db::sqlx::PgPool;

use crate::audit::{self, Target, kinds};
use crate::caller::{Caller, acting_user};
use crate::error::ServiceResult;

/// Who this workspace is, as the settings screen reads it.
///
/// Gated on `Settings`, like the security policy and the mail relay: this is
/// the administration screen's own data, and everything on it is edited from
/// the same place.
pub async fn load(pool: &PgPool, caller: &Caller) -> ServiceResult<OrganizationProfile> {
    caller.require(permissions::SETTINGS)?;
    Ok(store::load(pool).await?.profile)
}

/// Who this workspace is, for anything that has to render it.
///
/// Ungated, and deliberately so - the same reasoning as the password policy in
/// [`super::settings::load`]. A document header, an email footer or a page
/// title needs the organization's name and logo, and none of those is a secret
/// from the people inside the workspace. It takes no [`Caller`] because the
/// callers are the outbox relay and the mailer, which are not people.
///
/// Not exposed as a server function: the browser reaches this through
/// [`load`], which is gated, and adding an ungated route would make the
/// organization's address readable by anyone who can reach the server.
pub async fn current(pool: &PgPool) -> ServiceResult<OrganizationProfile> {
    Ok(store::load(pool).await?.profile)
}

/// Store a profile an administrator submitted.
///
/// Returns the profile as it now stands, so the screen re-renders from what was
/// written rather than from what was typed - which is what puts the trimming
/// and the lower-cased email in front of the person who saved it.
///
/// A [`Submission`] rather than a bare value, for the reason the rest of the
/// codebase gives: a form that fails validation is the expected path through a
/// form, and modelling it as `Err` collapses the per-field detail into one
/// string on the way across the wire.
pub async fn save(
    pool: &PgPool,
    caller: &Caller,
    profile: OrganizationProfile,
) -> ServiceResult<Submission<OrganizationProfile>> {
    caller.require(permissions::SETTINGS)?;
    let changed_by = acting_user(caller)?;

    let profile = profile.normalised();

    // Validated here rather than left to the CHECK constraints: a constraint
    // can only refuse the whole row, and it arrives as a constraint name that
    // nobody outside this codebase can read - where the form needs to know
    // *which* field is wrong.
    if let Some(rejection) = rejected(profile.validate()) {
        return Ok(rejection);
    }

    let before = store::load(pool).await?.profile;

    store::save(
        pool,
        store::ProfileUpdate {
            legal_name: &profile.legal_name,
            trading_name: profile.trading_name.as_deref(),
            registration_number: profile.registration_number.as_deref(),
            tax_id: profile.tax_id.as_deref(),
            industry: profile.industry.as_deref(),
            email: profile.email.as_deref(),
            phone: profile.phone.as_deref(),
            website: profile.website.as_deref(),
            address_line1: profile.address_line1.as_deref(),
            address_line2: profile.address_line2.as_deref(),
            city: profile.city.as_deref(),
            region: profile.region.as_deref(),
            postal_code: profile.postal_code.as_deref(),
            country: profile.country,
            currency: profile.currency,
            timezone: &profile.timezone,
            fiscal_year_start_month: profile.fiscal_year_start_month,
            updated_by: Some(changed_by),
        },
    )
    .await?;

    let after = store::load(pool).await?.profile;

    // The whole profile on each side. `updated` writes nothing when the two are
    // equal, so opening the screen and pressing save without typing leaves the
    // trail alone - which is what keeps the entries somebody is looking for
    // next to each other.
    audit::updated(
        pool,
        caller,
        Target::singleton(kinds::ORGANIZATION).named(&after.legal_name),
        &before,
        &after,
    )
    .await;

    if before != after {
        tracing::info!(
            legal_name = %after.legal_name,
            currency = %after.currency,
            %changed_by,
            "organization profile changed",
        );
    }

    Ok(Submission::Saved(after))
}
