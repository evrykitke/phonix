//! Identity: accounts, credentials and the forms that create them.
//!
//! Everything here compiles to wasm, which is the point: the signup form runs
//! exactly the rules the server enforces, so the client cannot show a green
//! field that the server will later reject. The server never *trusts* this -
//! it re-runs every check - but the two can no longer drift.
//!
//! What deliberately does **not** live here:
//!
//! * password hashing, session tokens, anything touching a database - those are
//!   in `phonix_db::identity`;
//! * permissions and roles - those are in [`crate::authorization`], because
//!   "who you are" and "what you may do" are separate questions and conflating
//!   them is how role checks end up hard-coded.

pub mod audit;
pub mod card;
pub mod directory;
pub mod edit;
pub mod invitation;
pub mod login;
pub mod mfa;
pub mod password;
pub mod signup;
pub mod user;
pub mod validation;

pub use audit::{
    AuditEvent, AuditEventDetail, Change, ChangeKind, Fact, FieldChange, NOTABLE_EVENTS,
};
pub use card::UserCard;
pub use directory::UserListing;
pub use edit::UserEdit;
pub use invitation::{InvitationIssued, UserInvite};
pub use login::{
    Credentials, DASHBOARD_PATH, HALF_AUTHENTICATED_PREFIX, INVITATION_ACCEPT_PATH, Landing,
    LoginResult, MFA_CHALLENGE_PATH, MFA_ENROLMENT_PATH, PASSWORD_CHANGE_PATH, PASSWORD_RESET_PATH,
    SIGN_IN_PATH, SIGN_UP_PATH, is_public_path, is_signed_out_chrome, landing,
};
pub use mfa::{
    MfaChallenge, MfaChallengeResult, MfaEnforcement, MfaFactorKind, MfaFactorSummary, MfaPolicy,
    MfaStatus, RecoveryCodes, TotpEnrolment,
};
pub use password::{
    ABSOLUTE_MIN_LENGTH, DEFAULT_MIN_LENGTH, MAX_PASSWORD_LEN, PasswordPolicy, PasswordStrength,
    password_strength, password_strength_for, validate_password,
};
pub use signup::{SignupInput, SignupOutcome, SignupResult, SlugAvailability, ValidSignup};
pub use user::{AuthUser, UserId, UserStatus};
pub use validation::{
    FieldError, MAX_NAME_LEN, MAX_ORGANIZATION_NAME_LEN, slug_from_organization_name,
    validate_email, validate_organization_name, validate_person_name, validate_workspace_slug,
};
