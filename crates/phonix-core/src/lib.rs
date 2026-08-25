//! Shared domain vocabulary for Phonix.
//!
//! Compiled for both the server and the WebAssembly client, so this crate must
//! stay free of `tokio`, `sqlx`, `redis` and `lapin`.
//!
//! Organised by scope rather than by file:
//!
//! | Module          | Question it answers                       |
//! | --------------- | ----------------------------------------- |
//! | [`tenant`]      | Which workspace does this request belong to? |
//! | [`identity`]    | Who is signed in?                         |
//! | [`audit`]       | What happened, and what changed?           |
//! | [`authorization`] | What may they do?                       |
//! | [`error`]       | What crosses back to the browser on failure? |
//! | [`query`]       | Which slice of a list is being asked for? |
//! | [`locale`]      | Which currency, country and time zone does it work in? |
//! | [`money`]       | How much, in what, and how was it converted? |
//! | [`numbering`]   | What number does this document get, and in what format? |
//! | [`i18n`]        | What does it say, and in which language?    |
//! | [`organization`] | Who is the legal entity behind this workspace? |
//!
//! # This crate may not panic
//!
//! Everything here is compiled into the WebAssembly bundle as well as into the
//! server, and `wasm32-unknown-unknown` aborts rather than unwinds: one panic
//! anywhere stops every handler, effect and pending request in the tab at once,
//! and the page simply freezes. There is nothing to catch it with - see
//! `phonix_web::recovery`, which can only report the freeze after the fact.
//!
//! So the same denial `phonix-web` carries applies here. A fallible thing
//! returns a `Result` or an `Option`; an invariant the compiler cannot see is
//! expressed by destructuring rather than by unwrapping something a comment
//! promises is `Some`. Tests are exempt: a failing assertion is the point.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

pub mod audit;
pub mod authorization;
pub mod error;
pub mod files;
pub mod form;
pub mod i18n;
pub mod identity;
pub mod locale;
pub mod mail;
pub mod money;
pub mod numbering;
pub mod organization;
pub mod query;
pub mod tenant;

pub use error::{Error, Result};

pub use audit::{EntityAction, EntityChange, EntityChangeDetail, EntityKind};
pub use authorization::{
    PermissionDenied, PermissionSet, RoleSummary, names as permissions, roles,
};
pub use files::{
    BucketPolicy, FileCategory, FileId, FileSummary, FileType, Rejection, UploadResult,
    UploadStatus,
};
pub use form::Submission;
pub use i18n::{Language, Message};
pub use identity::{
    AuthUser, Credentials, FieldError, LoginResult, MfaEnforcement, MfaPolicy, PasswordPolicy,
    PasswordStrength, SignupInput, SignupOutcome, SignupResult, SlugAvailability, UserId,
    UserStatus, ValidSignup,
};
pub use locale::{Country, Currency, Timezone};
pub use mail::{MailEncryption, MailSettings, MailSettingsInput, RelayInUse};
pub use money::{Conversion, ExchangeRate, Money, MoneyError, Rate, RateError, Rounding};
pub use numbering::{NumberContext, Pattern, PatternError, ResetPeriod};
pub use organization::OrganizationProfile;
pub use query::{Page, PageRequest, Sort, SortDirection};
pub use tenant::{TenantId, TenantSlug, TenantStatus, TenantSummary, WorkspaceSecuritySettings};
