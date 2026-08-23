//! What a use case can fail with.
//!
//! Three sources, kept apart because they mean different things to a caller:
//!
//! * [`ServiceError::Db`] - the storage failed or refused. Mostly not the
//!   user's problem.
//! * [`ServiceError::Rejected`] - the input was wrong, per field. Entirely the
//!   user's problem, and the form has to say which field.
//! * [`ServiceError::Crypto`] - a key or a stored hash is unusable. Nobody's
//!   problem but ours, and it never reaches a browser with any detail.
//!
//! Note what is *not* here: a wrong password, a wrong TOTP code, an unavailable
//! workspace name. Those are outcomes, not errors - they come back as `Ok` with
//! a `LoginResult`, an `MfaChallengeResult` or a `SignupResult`, because they
//! are the expected path through a form and modelling them as failures makes
//! every caller unwrap something that happens all day long.

use phonix_core::identity::FieldError;
use phonix_core::{Error as CoreError, PermissionDenied};
use phonix_db::DbError;

pub type ServiceResult<T> = Result<T, ServiceError>;

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error(transparent)]
    Db(#[from] DbError),

    /// Input that failed validation, field by field.
    #[error("{} field(s) rejected", .0.len())]
    Rejected(Vec<FieldError>),

    /// Argon2 or the secret vault could not do its job: a malformed key, a
    /// stored hash that is not a valid PHC string, a row sealed under a key
    /// this build does not hold.
    ///
    /// Never a wrong password or a failed decrypt-by-an-attacker - those are
    /// `Ok(false)`.
    #[error("credential processing failed: {0}")]
    Crypto(String),

    /// The caller is signed in but not permitted.
    #[error(transparent)]
    Forbidden(#[from] PermissionDenied),

    /// The caller is not signed in at all, on a path that requires it.
    #[error("not authenticated")]
    Unauthenticated,

    /// The place uploaded bytes live could not do what was asked.
    ///
    /// Separate from [`Self::Db`] because the two fail independently and for
    /// different reasons: a full disk is not a database problem, and a caller
    /// deciding whether to retry needs to know which one it was.
    #[error(transparent)]
    Storage(#[from] phonix_storage::StorageError),

    /// The thing being acted on is not there.
    ///
    /// A real answer rather than a fault: a file somebody deleted while
    /// somebody else had the page open is an ordinary Tuesday.
    #[error("{0} not found")]
    NotFound(&'static str),
}

impl ServiceError {
    /// One rejected field, for the common single-problem case.
    pub fn rejected(field: impl Into<String>, message: phonix_core::Message) -> Self {
        Self::Rejected(vec![FieldError::new(field, message)])
    }

    /// The rejected fields, if that is what this is.
    pub fn field_errors(&self) -> &[FieldError] {
        match self {
            Self::Rejected(errors) => errors,
            _ => &[],
        }
    }
}

impl From<ServiceError> for CoreError {
    /// Collapse to the coarse, safe error a browser is allowed to see.
    ///
    /// Field-level rejections survive, because the form needs them and they
    /// contain only what the caller submitted. Everything else is logged here
    /// and replaced by a label: a key description, a constraint name or a SQL
    /// fragment must not cross this boundary.
    fn from(err: ServiceError) -> Self {
        match err {
            ServiceError::Db(db) => db.into(),
            ServiceError::Rejected(errors) => {
                let detail = errors
                    .iter()
                    .map(|err| format!("{}: {}", err.field, err.message))
                    .collect::<Vec<_>>()
                    .join("; ");
                CoreError::Validation(detail)
            }
            ServiceError::Forbidden(_) => CoreError::Forbidden,
            ServiceError::Unauthenticated => CoreError::Unauthenticated,
            ServiceError::Crypto(detail) => {
                tracing::error!(error = %detail, "credential processing failed");
                CoreError::Internal
            }
            // A path, a mount point and a disk-full message all live inside
            // this error, and none of them crosses to a browser. What does
            // cross is whether it is worth trying again.
            ServiceError::Storage(err) => {
                tracing::error!(error = %err, "file storage failed");
                if err.is_retryable() {
                    CoreError::Unavailable("storage".to_owned())
                } else {
                    CoreError::Internal
                }
            }
            ServiceError::NotFound(what) => CoreError::NotFound(what.to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use phonix_core::msg;

    use super::*;

    #[test]
    fn a_field_rejection_reaches_the_form_intact() {
        let err =
            ServiceError::rejected("password", msg!("validation.password.too_short", min = 12));
        assert_eq!(err.field_errors().len(), 1);

        match CoreError::from(err) {
            CoreError::Validation(detail) => {
                assert!(detail.contains("password"));
                assert!(detail.contains("12 characters"));
            }
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    #[test]
    fn a_crypto_failure_tells_the_browser_nothing() {
        let err = ServiceError::Crypto(
            "security.mfa.encryption_key must decode to exactly 32 bytes".to_owned(),
        );

        match CoreError::from(err) {
            CoreError::Internal => {}
            other => panic!("a key description must not reach a browser: {other:?}"),
        }
    }

    #[test]
    fn a_missing_session_and_a_forbidden_one_are_different_answers() {
        assert_eq!(
            CoreError::from(ServiceError::Unauthenticated).status_code(),
            401
        );
        assert_eq!(
            CoreError::from(ServiceError::Forbidden(PermissionDenied::new(
                "Pages.Administration.Users"
            )))
            .status_code(),
            403
        );
    }
}
