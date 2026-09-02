//! `/api/v1/settings` - what a workspace has decided about itself.
//!
//! The administration area's settings screen is four tabs, and this is those
//! four tabs. They are **four sub-resources rather than one document**, and
//! that is the one design decision here worth stating:
//!
//! ```text
//! /settings/security      the password, MFA and audit policies   read: ungated
//! /settings/organization  who the workspace legally is           read: Settings
//! /settings/mail          this workspace's own relay             read: Settings
//! /settings/api           whether the API-key surface is sold    read: ungated
//! ```
//!
//! Merging them would force the strictest gate onto the loosest part. The
//! password policy is deliberately readable by everybody - a person filling in
//! a change-password form needs to be told the rules - and folding it into a
//! document that also carries the organization's registered address would mean
//! only administrators could find out how long their password has to be.
//!
//! # The gate is the service's, never this file's
//!
//! Every handler below calls its use case and lets `Caller::require` inside it
//! answer. Where a read is ungated here it is because the *service* is ungated,
//! for a reason recorded next to it. ADR 0002 §"Consequences": a use case that
//! authorizes in its adapter is a bug, because the other adapter will not do
//! it - and the inverse is just as true. An adapter that adds a gate the
//! service does not have is a surface that refuses what the browser allows,
//! and nothing would ever report the difference.
//!
//! # A PUT is the whole document
//!
//! Every field is required on the way in. Defaulting an omitted `enabled` to
//! `false` would let a caller who sent half a policy switch the audit trail off
//! without ever naming it, and "I did not mention it" must not mean "turn it
//! off". A client changing one field reads, edits and writes back - which is
//! also what makes two administrators saving at once produce one of the two
//! documents rather than a mixture of both.

use axum::Json;
use phonix_core::WorkspaceSecuritySettings;
use phonix_core::audit::AuditPolicy;
use phonix_core::form::Submission;
use phonix_core::identity::mfa::{MfaEnforcement, MfaPolicy};
use phonix_core::identity::password::PasswordPolicy;
use phonix_core::locale::{Country, Currency, Timezone};
use phonix_core::mail::{MailEncryption, MailSettings, MailSettingsInput};
use phonix_core::organization::OrganizationProfile;
use phonix_services::identity::api_key;
use phonix_services::mail;
use phonix_services::workspace::{profile, settings};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::auth::ApiCaller;
use super::json::ApiJson;
use super::problem::Problem;

// ---------------------------------------------------------------------------
// The security policy
// ---------------------------------------------------------------------------

/// What this workspace requires of a password.
///
/// One type for both directions, unlike the currency resource. The shapes are
/// genuinely identical - this is a policy that is read and written whole - and
/// declaring two would be two places to add the next field to, one of which
/// would eventually be forgotten.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[schema(as = PasswordPolicy)]
pub struct PasswordPolicyResource {
    #[schema(example = 12, minimum = 8)]
    pub min_length: u32,
    #[schema(example = 128)]
    pub max_length: u32,
    pub require_lowercase: bool,
    pub require_uppercase: bool,
    pub require_digit: bool,
    pub require_symbol: bool,
    /// Refuse the passwords that are guessed first.
    pub forbid_common: bool,
    /// Refuse passwords built out of the person's own name or address.
    pub forbid_personal_information: bool,
    /// Force a change after this many days. `null` is the default and the
    /// recommendation - routine expiry produces `Summer2026!` then
    /// `Autumn2026!` - and is available because some auditors still ask.
    pub expiry_days: Option<u32>,
    /// How many previous passwords may not be reused. `0` disables the check.
    #[schema(example = 0)]
    pub history_depth: u8,
}

/// Whether a second factor is asked for, and what may answer it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[schema(as = MfaPolicy)]
pub struct MfaPolicyResource {
    pub enforcement: MfaEnforcementResource,
    /// Authenticator apps - the only method this build implements.
    pub allow_totp: bool,
    /// One-time recovery codes, for the phone that fell in the sea. Off means
    /// an administrator resets a locked-out person by hand.
    pub allow_recovery_codes: bool,
    /// Days somebody may keep signing in without enrolling after `required` is
    /// turned on. Without it, switching to `required` locks out everybody who
    /// is not at their desk with their phone - including whoever flipped it.
    #[schema(example = 7)]
    pub grace_period_days: u32,
    /// Days a browser may skip the challenge after passing it once. `0`
    /// disables it, and is the default.
    #[schema(example = 0)]
    pub remember_device_days: u32,
}

/// How hard a second factor is asked for.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = MfaEnforcement)]
pub enum MfaEnforcementResource {
    /// Nobody may enrol, and existing factors stop being asked for. For an
    /// organization whose people authenticate through an upstream provider.
    Disabled,
    /// Enrol if you want to. The default.
    Optional,
    /// Everybody must hold a confirmed factor.
    Required,
}

impl From<MfaEnforcement> for MfaEnforcementResource {
    fn from(enforcement: MfaEnforcement) -> Self {
        match enforcement {
            MfaEnforcement::Disabled => Self::Disabled,
            MfaEnforcement::Optional => Self::Optional,
            MfaEnforcement::Required => Self::Required,
        }
    }
}

impl From<MfaEnforcementResource> for MfaEnforcement {
    fn from(enforcement: MfaEnforcementResource) -> Self {
        match enforcement {
            MfaEnforcementResource::Disabled => Self::Disabled,
            MfaEnforcementResource::Optional => Self::Optional,
            MfaEnforcementResource::Required => Self::Required,
        }
    }
}

/// How much of its own history this workspace keeps.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[schema(as = AuditPolicy)]
pub struct AuditPolicyResource {
    /// The master switch. `false` writes nothing from then on and deletes
    /// nothing that is already there.
    pub enabled: bool,
    /// The entity kinds this workspace does *not* record, by their stored
    /// names - the same strings `filter[kind]` on `/audit/changes` matches.
    /// A name this build has never heard of is kept rather than dropped, so a
    /// rollback cannot silently switch a kind back on.
    #[schema(example = json!(["user"]))]
    pub excluded: Vec<String>,
    /// Delete entries older than this many days. `null` keeps them forever,
    /// which is the default.
    pub retention_days: Option<i32>,
}

/// The three policies, which are one row and one save.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[schema(as = SecurityPolicy)]
pub struct SecurityPolicyResource {
    pub password: PasswordPolicyResource,
    pub mfa: MfaPolicyResource,
    pub audit: AuditPolicyResource,
}

impl From<&WorkspaceSecuritySettings> for SecurityPolicyResource {
    fn from(settings: &WorkspaceSecuritySettings) -> Self {
        Self {
            password: PasswordPolicyResource {
                // `usize` in core because it is compared against a character
                // count; a password long enough to overflow `u32` is not a
                // password. Saturating rather than unwrapping so a nonsense
                // stored row answers with a number instead of a 500.
                min_length: u32::try_from(settings.password.min_length).unwrap_or(u32::MAX),
                max_length: u32::try_from(settings.password.max_length).unwrap_or(u32::MAX),
                require_lowercase: settings.password.require_lowercase,
                require_uppercase: settings.password.require_uppercase,
                require_digit: settings.password.require_digit,
                require_symbol: settings.password.require_symbol,
                forbid_common: settings.password.forbid_common,
                forbid_personal_information: settings.password.forbid_personal_information,
                expiry_days: settings.password.expiry_days,
                history_depth: settings.password.history_depth,
            },
            mfa: MfaPolicyResource {
                enforcement: settings.mfa.enforcement.into(),
                allow_totp: settings.mfa.allow_totp,
                allow_recovery_codes: settings.mfa.allow_recovery_codes,
                grace_period_days: settings.mfa.grace_period_days,
                remember_device_days: settings.mfa.remember_device_days,
            },
            audit: AuditPolicyResource {
                enabled: settings.audit.enabled,
                excluded: settings.audit.excluded.clone(),
                retention_days: settings.audit.retention_days,
            },
        }
    }
}

impl From<SecurityPolicyResource> for WorkspaceSecuritySettings {
    fn from(body: SecurityPolicyResource) -> Self {
        Self {
            password: PasswordPolicy {
                // Widening on every target this builds for; `try_from`
                // rather than `as` so a target where it is not stops
                // compiling instead of silently truncating a policy.
                min_length: usize::try_from(body.password.min_length).unwrap_or(usize::MAX),
                max_length: usize::try_from(body.password.max_length).unwrap_or(usize::MAX),
                require_lowercase: body.password.require_lowercase,
                require_uppercase: body.password.require_uppercase,
                require_digit: body.password.require_digit,
                require_symbol: body.password.require_symbol,
                forbid_common: body.password.forbid_common,
                forbid_personal_information: body.password.forbid_personal_information,
                expiry_days: body.password.expiry_days,
                history_depth: body.password.history_depth,
            },
            mfa: MfaPolicy {
                enforcement: body.mfa.enforcement.into(),
                allow_totp: body.mfa.allow_totp,
                allow_recovery_codes: body.mfa.allow_recovery_codes,
                grace_period_days: body.mfa.grace_period_days,
                remember_device_days: body.mfa.remember_device_days,
            },
            audit: AuditPolicy {
                enabled: body.audit.enabled,
                excluded: body.audit.excluded,
                retention_days: body.audit.retention_days,
            },
        }
    }
}

/// What this workspace requires of the people in it.
///
/// **Ungated**, because the service is: somebody about to choose a password has
/// to be told the rules, and somebody being sent to enrolment has to be told
/// why. Neither is a secret from the person it applies to.
#[utoipa::path(
    get,
    path = "/settings/security",
    tag = "settings",
    operation_id = "getSecurityPolicy",
    responses(
        (status = 200, description = "The three policies", body = SecurityPolicyResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The workspace has no API access", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn get_security(caller: ApiCaller) -> Result<Json<SecurityPolicyResource>, Problem> {
    let stored = settings::load(&caller.pool).await?;

    Ok(Json(SecurityPolicyResource::from(&stored)))
}

/// Replace the three policies.
///
/// Whole-document, and validated field by field by the service - so a minimum
/// length below the compiled floor comes back as a 422 naming `min_length`
/// rather than as a refusal of the whole body with nothing to act on. Requires
/// `Pages.Administration.Settings`.
#[utoipa::path(
    put,
    path = "/settings/security",
    tag = "settings",
    operation_id = "saveSecurityPolicy",
    request_body = SecurityPolicyResource,
    responses(
        (status = 200, description = "The policies as they now stand", body = SecurityPolicyResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Settings", body = Problem),
        (status = 415, description = "The body was not sent as JSON", body = Problem),
        (status = 422, description = "A policy field was refused", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn save_security(
    caller: ApiCaller,
    ApiJson(body): ApiJson<SecurityPolicyResource>,
) -> Result<Json<SecurityPolicyResource>, Problem> {
    let desired = WorkspaceSecuritySettings::from(body);

    settings::save(&caller.pool, &caller.caller, &desired).await?;

    tracing::info!(key = ?caller.key_id, "security policy saved through the api");

    // Read back rather than echoed: the same rule `currencies::save` follows.
    // What was stored is what the caller should see, and the two differ the
    // moment a rule normalises something.
    let stored = settings::load(&caller.pool).await?;

    Ok(Json(SecurityPolicyResource::from(&stored)))
}

// ---------------------------------------------------------------------------
// The organization
// ---------------------------------------------------------------------------

/// Who this workspace legally is.
///
/// What appears on everything it issues, which is why the change trail records
/// it as a `{from, to}` pair and why `GET /audit/changes?filter[kind]=organization`
/// is worth knowing about.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = Organization)]
pub struct OrganizationResource {
    /// The registered entity name. The one required field.
    #[schema(example = "Ada Computing Ltd")]
    pub legal_name: String,
    /// What it trades as, when that differs.
    pub trading_name: Option<String>,
    /// Companies-house number, EIN, CR number - whatever the jurisdiction
    /// calls it. Free text: there are as many formats as there are registries.
    pub registration_number: Option<String>,
    /// VAT, GST, PIN, TIN. Free text for the same reason.
    pub tax_id: Option<String>,
    pub industry: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub website: Option<String>,
    pub address_line1: Option<String>,
    pub address_line2: Option<String>,
    pub city: Option<String>,
    /// State, province, county or region - one field, because the level that
    /// matters differs by country.
    pub region: Option<String>,
    pub postal_code: Option<String>,
    /// ISO 3166-1 alpha-2.
    #[schema(example = "GB")]
    pub country: Option<String>,
    /// ISO 4217. What amounts are denominated in.
    #[schema(example = "GBP")]
    pub currency: String,
    /// IANA name. What "today" means here.
    #[schema(example = "Europe/London")]
    pub timezone: String,
    /// The month the financial year opens, 1-12.
    #[schema(example = 4, minimum = 1, maximum = 12)]
    pub fiscal_year_start_month: u8,
    /// The uploaded logo, if one is set. **Read-only here**: it is changed by
    /// the upload endpoints, because attaching one has to check that the
    /// upload finished and has to delete the image it replaces.
    pub logo_file_id: Option<Uuid>,
}

impl From<&OrganizationProfile> for OrganizationResource {
    fn from(profile: &OrganizationProfile) -> Self {
        Self {
            legal_name: profile.legal_name.clone(),
            trading_name: profile.trading_name.clone(),
            registration_number: profile.registration_number.clone(),
            tax_id: profile.tax_id.clone(),
            industry: profile.industry.clone(),
            email: profile.email.clone(),
            phone: profile.phone.clone(),
            website: profile.website.clone(),
            address_line1: profile.address_line1.clone(),
            address_line2: profile.address_line2.clone(),
            city: profile.city.clone(),
            region: profile.region.clone(),
            postal_code: profile.postal_code.clone(),
            country: profile.country.map(|country| country.code().to_owned()),
            currency: profile.currency.code().to_owned(),
            timezone: profile.timezone.as_str().to_owned(),
            fiscal_year_start_month: profile.fiscal_year_start_month,
            logo_file_id: profile.logo_file_id,
        }
    }
}

/// What `PUT /settings/organization` accepts.
///
/// The read type minus `logo_file_id`, and the difference is real rather than
/// cosmetic: a draft opened before somebody else replaced the logo would put
/// the old one back on every document the workspace issues, without anybody
/// having chosen that. The field is therefore not something this endpoint can
/// be handed at all.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[schema(as = OrganizationSave)]
pub struct SaveOrganization {
    #[schema(example = "Ada Computing Ltd")]
    pub legal_name: String,
    #[serde(default)]
    pub trading_name: Option<String>,
    #[serde(default)]
    pub registration_number: Option<String>,
    #[serde(default)]
    pub tax_id: Option<String>,
    #[serde(default)]
    pub industry: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub address_line1: Option<String>,
    #[serde(default)]
    pub address_line2: Option<String>,
    #[serde(default)]
    pub city: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub postal_code: Option<String>,
    /// ISO 3166-1 alpha-2, or `null`.
    #[schema(example = "GB")]
    #[serde(default)]
    pub country: Option<String>,
    /// ISO 4217.
    #[schema(example = "GBP")]
    pub currency: String,
    /// IANA name.
    #[schema(example = "Europe/London")]
    pub timezone: String,
    #[schema(example = 4, minimum = 1, maximum = 12)]
    pub fiscal_year_start_month: u8,
}

/// Who this workspace says it is.
///
/// Gated on `Pages.Administration.Settings`, like the rest of the settings
/// screen. The ungated read the outbox and the mailer use is a different
/// function and is not on this surface.
#[utoipa::path(
    get,
    path = "/settings/organization",
    tag = "settings",
    operation_id = "getOrganization",
    responses(
        (status = 200, description = "The organization profile", body = OrganizationResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Settings", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn get_organization(caller: ApiCaller) -> Result<Json<OrganizationResource>, Problem> {
    let stored = profile::load(&caller.pool, &caller.caller).await?;

    Ok(Json(OrganizationResource::from(&stored)))
}

/// Replace the organization profile.
///
/// The logo is not touched, whatever else changes - see [`SaveOrganization`].
/// Requires `Pages.Administration.Settings`.
#[utoipa::path(
    put,
    path = "/settings/organization",
    tag = "settings",
    operation_id = "saveOrganization",
    request_body = SaveOrganization,
    responses(
        (status = 200, description = "The profile as it now stands", body = OrganizationResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Settings", body = Problem),
        (status = 415, description = "The body was not sent as JSON", body = Problem),
        (status = 422, description = "A field was refused", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn save_organization(
    caller: ApiCaller,
    ApiJson(body): ApiJson<SaveOrganization>,
) -> Result<Json<OrganizationResource>, Problem> {
    // Read first, for the logo. `save` does not write it, but the profile it
    // takes has a field for it, and building one with `None` would say
    // "no logo" where the honest statement is "not this endpoint's business".
    let current = profile::load(&caller.pool, &caller.caller).await?;

    let currency = Currency::parse(&body.currency).map_err(|_| {
        Problem::invalid(
            "currency",
            "request.currency.unknown",
            format!("{} is not an ISO 4217 code.", body.currency),
        )
    })?;

    let timezone = Timezone::parse(&body.timezone).map_err(|_| {
        Problem::invalid(
            "timezone",
            "request.timezone.unknown",
            format!("{} is not an IANA timezone name.", body.timezone),
        )
    })?;

    // `None` is a country nobody has named; `Some` that will not parse is a
    // country somebody named wrongly. Collapsing the two would store "no
    // country" for a typo, which is a silent answer to a real mistake.
    let country = match body
        .country
        .as_deref()
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
    {
        None => None,
        Some(raw) => Some(Country::parse(raw).map_err(|_| {
            Problem::invalid(
                "country",
                "request.country.unknown",
                format!("{raw} is not an ISO 3166-1 alpha-2 code."),
            )
        })?),
    };

    let desired = OrganizationProfile {
        legal_name: body.legal_name,
        trading_name: body.trading_name,
        registration_number: body.registration_number,
        tax_id: body.tax_id,
        industry: body.industry,
        email: body.email,
        phone: body.phone,
        website: body.website,
        address_line1: body.address_line1,
        address_line2: body.address_line2,
        city: body.city,
        region: body.region,
        postal_code: body.postal_code,
        country,
        currency,
        timezone,
        fiscal_year_start_month: body.fiscal_year_start_month,
        // Carried through rather than cleared. `save` ignores it either way.
        logo_file_id: current.logo_file_id,
    };

    let saved = profile::save(&caller.pool, &caller.caller, desired).await?;

    match saved {
        Submission::Saved(stored) => {
            tracing::info!(key = ?caller.key_id, "organization profile saved through the api");
            Ok(Json(OrganizationResource::from(&stored)))
        }
        // Through the one conversion, so a field refused here is
        // indistinguishable on the wire from a field refused anywhere else.
        Submission::Rejected(errors) => Err(Problem::from(
            phonix_services::ServiceError::Rejected(errors),
        )),
    }
}

// ---------------------------------------------------------------------------
// The mail relay
// ---------------------------------------------------------------------------

/// Where this workspace's mail goes.
///
/// **No password field, on either the read or the audit trail.** The only path
/// that reads the stored password back hands it straight to the SMTP client;
/// nothing between there and the socket sees it as a string, and nothing on
/// this surface ever will.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = MailSettings)]
pub struct MailResource {
    /// Whether this workspace sends through its own relay at all. `false`
    /// falls back to the system default, whatever else is stored here - and
    /// "no relay at all" is an ordinary answer rather than a misconfiguration.
    pub enabled: bool,
    #[schema(example = "smtp.example.com")]
    pub host: String,
    #[schema(example = 587)]
    pub port: u16,
    pub username: String,
    #[schema(example = "billing@example.com")]
    pub from_address: String,
    pub from_name: String,
    pub reply_to: Option<String>,
    pub encryption: MailEncryptionResource,
    /// Whether a password is stored. Never the password.
    pub has_password: bool,
}

/// How the connection to the relay is protected.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = MailEncryption)]
pub enum MailEncryptionResource {
    /// Connect in clear, then upgrade. What 587 and 2525 expect.
    StartTls,
    /// TLS from the first byte. What 465 expects.
    Implicit,
    /// None at all. The password crosses the wire in clear, so this is for a
    /// relay on localhost and for nothing else.
    None,
}

impl From<MailEncryption> for MailEncryptionResource {
    fn from(encryption: MailEncryption) -> Self {
        match encryption {
            MailEncryption::StartTls => Self::StartTls,
            MailEncryption::Implicit => Self::Implicit,
            MailEncryption::None => Self::None,
        }
    }
}

impl From<MailEncryptionResource> for MailEncryption {
    fn from(encryption: MailEncryptionResource) -> Self {
        match encryption {
            MailEncryptionResource::StartTls => Self::StartTls,
            MailEncryptionResource::Implicit => Self::Implicit,
            MailEncryptionResource::None => Self::None,
        }
    }
}

impl From<&MailSettings> for MailResource {
    fn from(settings: &MailSettings) -> Self {
        Self {
            enabled: settings.enabled,
            host: settings.host.clone(),
            port: settings.port,
            username: settings.username.clone(),
            from_address: settings.from_address.clone(),
            from_name: settings.from_name.clone(),
            reply_to: settings.reply_to.clone(),
            encryption: settings.encryption.into(),
            has_password: settings.has_password,
        }
    }
}

/// What `PUT /settings/mail` accepts.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[schema(as = MailSettingsSave)]
pub struct SaveMail {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// **Three states, and they are not the same.** Omit the field (or send
    /// `null`) to leave the stored password exactly as it is - which is what
    /// lets a caller change a host without ever having been given the secret
    /// it is not changing. Send `""` to remove it, because some relays
    /// authenticate on the username alone. Send anything else to replace it.
    ///
    /// It is sealed on arrival and never read back by anything but the SMTP
    /// client.
    #[serde(default)]
    pub password: Option<String>,
    pub from_address: String,
    pub from_name: String,
    #[serde(default)]
    pub reply_to: Option<String>,
    pub encryption: MailEncryptionResource,
}

/// This workspace's relay.
///
/// Gated on `Pages.Administration.Settings`. A host and a username are not a
/// secret from an administrator; the password is not returned by this or any
/// other read.
#[utoipa::path(
    get,
    path = "/settings/mail",
    tag = "settings",
    operation_id = "getMailSettings",
    responses(
        (status = 200, description = "The relay, without its password", body = MailResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Settings", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn get_mail(caller: ApiCaller) -> Result<Json<MailResource>, Problem> {
    let stored = mail::settings::load(&caller.pool, &caller.caller).await?;

    Ok(Json(MailResource::from(&stored)))
}

/// Replace the relay.
///
/// The fields are only validated when `enabled` is true: a workspace switching
/// its override *off* is not asked to fix the host it is about to stop using.
/// Requires `Pages.Administration.Settings`.
#[utoipa::path(
    put,
    path = "/settings/mail",
    tag = "settings",
    operation_id = "saveMailSettings",
    request_body = SaveMail,
    responses(
        (status = 200, description = "The relay as it now stands", body = MailResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Settings", body = Problem),
        (status = 415, description = "The body was not sent as JSON", body = Problem),
        (status = 422, description = "A field was refused", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn save_mail(
    caller: ApiCaller,
    ApiJson(body): ApiJson<SaveMail>,
) -> Result<Json<MailResource>, Problem> {
    let input = MailSettingsInput {
        enabled: body.enabled,
        host: body.host,
        port: body.port,
        username: body.username,
        password: body.password,
        from_address: body.from_address,
        from_name: body.from_name,
        reply_to: body.reply_to,
        encryption: body.encryption.into(),
    };

    let saved =
        mail::settings::save(&caller.pool, &caller.caller, &caller.state.vault, input).await?;

    match saved {
        Submission::Saved(stored) => {
            tracing::info!(
                key = ?caller.key_id,
                host = %stored.host,
                enabled = stored.enabled,
                "mail settings saved through the api"
            );
            Ok(Json(MailResource::from(&stored)))
        }
        Submission::Rejected(errors) => Err(Problem::from(
            phonix_services::ServiceError::Rejected(errors),
        )),
    }
}

// ---------------------------------------------------------------------------
// The API licence
// ---------------------------------------------------------------------------

/// Whether this workspace has the API-key surface.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[schema(as = ApiAccess)]
pub struct ApiAccessResource {
    /// A **licence, not a permission**: it says whether the workspace has the
    /// feature, which is why an administrator cannot grant it to themselves in
    /// the permission tree. Off, and every call presenting an API key answers
    /// `403 api_disabled` whatever the key is.
    ///
    /// It does not gate a session bearer. Somebody signed in on their own phone
    /// is using the product, not integrating with it - see ADR 0003 §3.
    pub enabled: bool,
}

/// Whether the API-key surface is switched on here.
///
/// **Ungated**, because the service is: the screen that manages keys has to be
/// able to tell the person looking at it that the API is off.
#[utoipa::path(
    get,
    path = "/settings/api",
    tag = "settings",
    operation_id = "getApiAccess",
    responses(
        (status = 200, description = "Whether API keys work here", body = ApiAccessResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The workspace has no API access", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn get_api_access(caller: ApiCaller) -> Result<Json<ApiAccessResource>, Problem> {
    let enabled = api_key::api_enabled(&caller.pool).await?;

    Ok(Json(ApiAccessResource { enabled }))
}

/// Turn the API-key surface on or off.
///
/// Requires `Pages.Administration.Settings` - the licence sits with the other
/// decisions about the workspace itself, not with the credentials issued
/// inside it.
///
/// # Switching it off through a key switches that key off
///
/// Deliberately allowed, and worth being clear about rather than special-casing:
/// this is the same act an administrator performs on the screen, and refusing
/// it here would mean the surface cannot be used to administer itself. The way
/// back is a browser, or `POST /auth/token` and a session bearer, which the
/// flag does not gate.
#[utoipa::path(
    put,
    path = "/settings/api",
    tag = "settings",
    operation_id = "setApiAccess",
    request_body = ApiAccessResource,
    responses(
        (status = 200, description = "The licence as it now stands", body = ApiAccessResource),
        (status = 401, description = "No usable key", body = Problem),
        (status = 403, description = "The key does not carry Settings", body = Problem),
        (status = 415, description = "The body was not sent as JSON", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn set_api_access(
    caller: ApiCaller,
    ApiJson(body): ApiJson<ApiAccessResource>,
) -> Result<Json<ApiAccessResource>, Problem> {
    api_key::set_api_enabled(&caller.pool, &caller.caller, body.enabled).await?;

    if !body.enabled {
        tracing::warn!(
            key = ?caller.key_id,
            "api key access switched off through the api"
        );
    }

    let enabled = api_key::api_enabled(&caller.pool).await?;

    Ok(Json(ApiAccessResource { enabled }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_security_policy_survives_a_round_trip() {
        // The conversion is hand-written on both sides, which is the whole
        // point of the wire types - and a hand-written pair is exactly the
        // kind that loses a field. A caller that reads, edits one value and
        // writes back must not silently reset the nine it did not touch.
        let stored = WorkspaceSecuritySettings {
            password: PasswordPolicy {
                min_length: 16,
                require_symbol: true,
                expiry_days: Some(90),
                history_depth: 5,
                ..PasswordPolicy::system_default()
            },
            mfa: MfaPolicy {
                enforcement: MfaEnforcement::Required,
                allow_recovery_codes: false,
                grace_period_days: 3,
                remember_device_days: 30,
                ..MfaPolicy::system_default()
            },
            audit: AuditPolicy {
                enabled: true,
                excluded: vec!["user".to_owned()],
                retention_days: Some(365),
            },
        };

        let there_and_back = WorkspaceSecuritySettings::from(SecurityPolicyResource::from(&stored));

        assert_eq!(there_and_back, stored);
    }

    #[test]
    fn every_enforcement_maps_both_ways() {
        for enforcement in [
            MfaEnforcement::Disabled,
            MfaEnforcement::Optional,
            MfaEnforcement::Required,
        ] {
            let round_tripped = MfaEnforcement::from(MfaEnforcementResource::from(enforcement));
            assert_eq!(round_tripped, enforcement);
        }
    }

    #[test]
    fn every_encryption_maps_both_ways() {
        for encryption in [
            MailEncryption::StartTls,
            MailEncryption::Implicit,
            MailEncryption::None,
        ] {
            let round_tripped = MailEncryption::from(MailEncryptionResource::from(encryption));
            assert_eq!(round_tripped, encryption);
        }
    }

    #[test]
    fn the_mail_resource_has_nowhere_to_put_a_password() {
        // Asserted on the serialised shape rather than trusted to the struct,
        // because the failure this guards against is somebody adding the field
        // to make a screen easier and nothing noticing.
        let settings = MailSettings {
            host: "smtp.example.com".to_owned(),
            has_password: true,
            ..MailSettings::unset()
        };

        let json = serde_json::to_string(&MailResource::from(&settings)).expect("it serialises");

        assert!(json.contains("has_password"));
        assert!(
            !json.contains("\"password\""),
            "a password reached the wire"
        );
    }

    #[test]
    fn an_absent_mail_password_is_not_the_same_as_an_empty_one() {
        // Three states, and the two that look alike are the ones that matter:
        // absent leaves the stored password alone, empty removes it. A
        // `#[serde(default)]` that turned an omitted field into `Some("")`
        // would quietly unauthenticate every workspace that saved a host.
        let untouched: SaveMail = serde_json::from_str(
            r#"{"enabled":true,"host":"h","port":587,"username":"u",
                "from_address":"a@b.c","from_name":"n","encryption":"start_tls"}"#,
        )
        .expect("the body parses without a password");
        let cleared: SaveMail = serde_json::from_str(
            r#"{"enabled":true,"host":"h","port":587,"username":"u","password":"",
                "from_address":"a@b.c","from_name":"n","encryption":"start_tls"}"#,
        )
        .expect("the body parses with an empty password");

        assert_eq!(untouched.password, None);
        assert_eq!(cleared.password.as_deref(), Some(""));
    }

    #[test]
    fn the_organization_save_body_cannot_carry_a_logo() {
        // `deny_unknown_fields` is not on the type - a later release must be
        // able to add an optional field without breaking older clients - so
        // this asserts the field is simply not read, which is what stops a
        // stale draft putting yesterday's logo back on every document.
        let body: SaveOrganization = serde_json::from_str(
            r#"{"legal_name":"Ada Computing Ltd","currency":"GBP",
                "timezone":"Europe/London","fiscal_year_start_month":4,
                "logo_file_id":"00000000-0000-0000-0000-000000000001"}"#,
        )
        .expect("an unknown field is ignored rather than refused");

        assert_eq!(body.legal_name, "Ada Computing Ltd");
        assert_eq!(body.fiscal_year_start_month, 4);
    }

    #[test]
    fn the_organization_resource_spells_its_codes_rather_than_its_types() {
        let profile = OrganizationProfile {
            legal_name: "Ada Computing Ltd".to_owned(),
            country: Country::parse("GB").ok(),
            currency: Currency::parse("GBP").expect("a real code"),
            timezone: Timezone::parse("Europe/London").expect("a real zone"),
            ..OrganizationProfile::empty()
        };

        let resource = OrganizationResource::from(&profile);

        assert_eq!(resource.country.as_deref(), Some("GB"));
        assert_eq!(resource.currency, "GBP");
        assert_eq!(resource.timezone, "Europe/London");
    }
}
