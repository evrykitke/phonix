//! `/api/v1/auth` - signing a person in from a device that has no cookie jar.
//!
//! See `docs/adr/0003-mobile-authentication.md`. The whole design in one line:
//! **a mobile sign-in produces a session, not a new kind of credential.** The
//! same row in `sessions`, the same two deadlines, the same `revoked_at`, the
//! same `mfa_satisfied`; the token comes back in a JSON body instead of a
//! `Set-Cookie` header, and it lives by `[security.session.mobile]`.
//!
//! Nothing here re-implements a sign-in. `authentication::sign_in` is called
//! with [`Delivery::Bearer`] and reaches the lockout check, the
//! timing-equalised dummy verify, the audit entry and every `LoginResult`
//! outcome by exactly the code a browser reaches them by. A second sign-in path
//! is a second place for a control to be forgotten, and there is not one.
//!
//! # Four endpoints, and the fifth that is missing
//!
//! There is no refresh endpoint. The session slides on use - `session::resume`
//! already does that - so an application opened inside the idle window never
//! needs one, and one that is not needs a real sign-in. A refresh/access pair
//! exists to bound the damage of a long-lived credential on a server that
//! cannot revoke; this one can, in a single statement.

use axum::Json;
use axum::http::StatusCode;
use phonix_core::identity::{AuthUser, Credentials, LoginResult, MfaChallengeResult};
use phonix_db::identity::session::ClientFacts;
use phonix_services::identity::{authentication, mfa, session as session_service};
use phonix_services::Delivery;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::auth::{ApiCaller, ApiWorkspace, bearer_of};
use super::json::ApiJson;
use super::problem::Problem;

/// What `POST /auth/token` accepts.
///
/// No `remember_me`. That is a checkbox on a browser's sign-in form; a mobile
/// application has no such form, and its ceiling comes from
/// `[security.session.mobile]` rather than from something the client asks for.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[schema(as = SignIn)]
pub struct SignInBody {
    #[schema(example = "ada@example.com")]
    pub email: String,
    pub password: String,
}

/// How far a sign-in got.
///
/// A field of its own rather than a `code` on an error body, because the three
/// middle values are **successes**: they return a real token, and the client's
/// next step is to use it against the endpoint this names. Only a refusal is a
/// problem document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = SessionStatus)]
pub enum SessionStatus {
    /// Fully signed in. The token reaches everything the person may do.
    SignedIn,
    /// A second factor is owed. The token reaches `POST /auth/mfa` and nothing
    /// else, exactly as a browser's half-authenticated session reaches only the
    /// challenge screen.
    MfaRequired,
    /// The workspace requires a second factor this person has not enrolled and
    /// their grace period has run out. Enrolment is not yet on this surface, so
    /// today this means "finish in a browser".
    MfaEnrolmentRequired,
    /// The password has aged past the workspace's policy, or was flagged for a
    /// forced change. Same as above: finish in a browser.
    PasswordChangeRequired,
}

impl SessionStatus {
    /// The outcome as this surface reports it, or `None` for a refusal - which
    /// is a problem document rather than a status.
    fn of(result: &LoginResult) -> Option<Self> {
        match result {
            LoginResult::Success(_) => Some(Self::SignedIn),
            LoginResult::MfaRequired { .. } => Some(Self::MfaRequired),
            LoginResult::MfaEnrolmentRequired { .. } => Some(Self::MfaEnrolmentRequired),
            LoginResult::PasswordChangeRequired { .. } => Some(Self::PasswordChangeRequired),
            LoginResult::Rejected | LoginResult::Locked { .. } => None,
        }
    }

}

/// A session, as the only response that ever carries its token.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = Session)]
pub struct SessionResource {
    /// Present it as `Authorization: Bearer <token>`. Not recoverable: this is
    /// the only response it appears in.
    pub token: String,
    /// Always `Bearer`, so a client can build the header without special-casing
    /// this API.
    #[schema(example = "Bearer")]
    pub token_type: &'static str,
    /// Seconds until the session's *absolute* deadline - the one activity
    /// cannot extend. The idle deadline is longer than any sensible client's
    /// gap between requests and slides on every one, so it is not worth a
    /// client tracking; this is the moment a real sign-in becomes necessary.
    #[schema(example = 7_776_000)]
    pub expires_in: i64,
    pub status: SessionStatus,
}

/// Who a token belongs to, and what they may do.
///
/// Answered from the account on every call rather than baked into the token at
/// sign-in, which is the reason there is no claim to decode: a grant removed
/// this morning is gone from this response this afternoon.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = Viewer)]
pub struct ViewerResource {
    pub id: uuid::Uuid,
    pub email: String,
    pub display_name: String,
    /// For display. Authority comes from `permissions`; Phonix has no active
    /// role to switch between.
    pub roles: Vec<String>,
    /// Every permission this person currently holds, flattened. For a key, the
    /// owner's grants narrowed to the key's scopes - so a client can hide what
    /// this credential in particular cannot do.
    pub permissions: Vec<String>,
}

impl From<&AuthUser> for ViewerResource {
    fn from(user: &AuthUser) -> Self {
        Self {
            id: user.id,
            email: user.email.clone(),
            display_name: user.display_name.clone(),
            roles: user.roles.clone(),
            permissions: user.permissions.iter().map(str::to_owned).collect(),
        }
    }
}

/// What `POST /auth/mfa` accepts.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[schema(as = MfaAnswer)]
pub struct MfaBody {
    /// A code from the authenticator app, or a recovery code if the workspace
    /// allows them.
    #[schema(example = "123456")]
    pub code: String,
}

/// Sign in and receive a session token.
///
/// Unauthenticated, and the only endpoint on this surface that is. It is also
/// the only one that will accept a password, which is why it is counted in the
/// credential rate-limit tier rather than the API one.
#[utoipa::path(
    post,
    path = "/auth/token",
    tag = "auth",
    operation_id = "signIn",
    request_body = SignInBody,
    responses(
        (status = 200, description = "A session token, and how far the sign-in got", body = SessionResource),
        (status = 401, description = "Wrong credentials, no such account, or a suspended one", body = Problem),
        (status = 415, description = "The body was not sent as JSON", body = Problem),
        (status = 429, description = "Too many attempts; see Retry-After", body = Problem),
    ),
)]
pub async fn sign_in(
    workspace: ApiWorkspace,
    ApiJson(body): ApiJson<SignInBody>,
) -> Result<Json<SessionResource>, Problem> {
    let state = workspace.state;
    // Through `server::client`, which the browser's sign-in also reads through,
    // so the two describe the same client identically - including the bound on
    // a user-agent that is about to be stored.
    let facts = phonix_web::server::client::facts_of(&workspace.headers);

    let signed_in = authentication::sign_in(
        &workspace.pool,
        &state.security(),
        &Credentials {
            email: body.email,
            password: body.password,
            remember_me: false,
        },
        ClientFacts {
            ip: facts.ip.as_deref(),
            user_agent: facts.user_agent.as_deref(),
        },
        Delivery::Bearer,
    )
    .await?;

    match (&signed_in.result, signed_in.token) {
        // A refusal, and one answer for every reason behind it: wrong password,
        // no such account, suspended. The timing was equalised in the service.
        (LoginResult::Rejected, _) => Err(Problem::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "That email address and password do not match an account here.",
        )),
        // The wait is not a secret - the caller caused it - so unlike the
        // rejection reason it is stated.
        (LoginResult::Locked { retry_after_secs }, _) => Err(Problem::new(
            StatusCode::TOO_MANY_REQUESTS,
            "account_locked",
            format!("Too many attempts. Try again in {retry_after_secs} seconds."),
        )
        .retry_after(*retry_after_secs)),
        (result, Some(token)) => {
            let status = SessionStatus::of(result)
                .expect("a result with a token is not a refusal, and the two arms above took both");

            Ok(Json(SessionResource {
                token: token.expose_secret().to_owned(),
                token_type: "Bearer",
                expires_in: signed_in.max_age_secs,
                status,
            }))
        }
        // `Delivery::Bearer` always opens a session when the password was
        // accepted. Reaching here means that stopped being true, and answering
        // 200 with no token would leave a client retrying for ever.
        (_, None) => {
            tracing::error!("a bearer sign-in was accepted but produced no session");
            Err(Problem::from(phonix_core::Error::Unavailable(
                "session".to_owned(),
            )))
        }
    }
}

/// Answer the second factor a sign-in asked for.
///
/// Takes the token `POST /auth/token` returned with `mfa_required`. That token
/// reaches this endpoint and nothing else: until the factor is satisfied the
/// account reports nothing as permitted, so every other endpoint answers 403.
#[utoipa::path(
    post,
    path = "/auth/mfa",
    tag = "auth",
    operation_id = "answerMfa",
    request_body = MfaBody,
    responses(
        (status = 200, description = "The factor was accepted; the same token is now fully signed in", body = SessionResource),
        (status = 401, description = "No session, or it expired while waiting", body = Problem),
        (status = 422, description = "Wrong code; `detail` says how many attempts remain", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn answer_mfa(
    workspace: ApiWorkspace,
    ApiJson(body): ApiJson<MfaBody>,
) -> Result<Json<SessionResource>, Problem> {
    let state = workspace.state;

    // Not `ApiCaller`: this session has deliberately not finished
    // authenticating, and what is needed is the session *row* rather than a
    // caller that would hold nothing anyway.
    let Some(token) = bearer_of(&workspace.headers) else {
        return Err(Problem::unauthenticated(
            "Send the token from `POST /auth/token` as `Authorization: Bearer`.",
        ));
    };

    let Some(session) =
        session_service::resume(&workspace.pool, &token, &state.config.security.session).await?
    else {
        return Err(Problem::unauthenticated("That session is not valid."));
    };

    let answer = mfa::answer_challenge(
        &workspace.pool,
        &state.vault,
        &state.config.security.mfa,
        session.id,
        session.user_id,
        body.code.trim(),
    )
    .await?;

    match answer {
        // The token the caller already holds is now a full session. It is
        // returned again rather than minted afresh so a client can store one
        // response shape and be done.
        MfaChallengeResult::Accepted(_) => Ok(Json(SessionResource {
            token: token.expose_secret().to_owned(),
            token_type: "Bearer",
            expires_in: session.remaining_secs(),
            status: SessionStatus::SignedIn,
        })),
        MfaChallengeResult::Rejected { attempts_remaining } => Err(Problem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "mfa_rejected",
            format!("That code is not right. {attempts_remaining} attempts remain."),
        )),
        // The session is gone in both of these, and the difference matters to a
        // client: one means start again, the other means it already had.
        MfaChallengeResult::Exhausted => Err(Problem::new(
            StatusCode::UNAUTHORIZED,
            "mfa_exhausted",
            "Too many wrong codes. Sign in again.",
        )),
        MfaChallengeResult::NoChallenge => Err(Problem::unauthenticated(
            "There is no second factor outstanding on that session.",
        )),
    }
}

/// Who this credential acts as, and what it may do.
#[utoipa::path(
    get,
    path = "/auth/me",
    tag = "auth",
    operation_id = "getViewer",
    responses(
        (status = 200, description = "The signed-in person", body = ViewerResource),
        (status = 401, description = "No usable credential", body = Problem),
        (status = 403, description = "A second factor is still outstanding", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn viewer(caller: ApiCaller) -> Result<Json<ViewerResource>, Problem> {
    let Some(user) = caller.caller.auth_user() else {
        // `Caller::System` never reaches an HTTP handler; this is the arm that
        // says so rather than unwrapping and finding out otherwise.
        return Err(Problem::unauthenticated("That credential has no account."));
    };

    if !user.is_fully_authenticated() {
        return Err(Problem::new(
            StatusCode::FORBIDDEN,
            "mfa_required",
            "This session has not finished signing in. Answer `POST /auth/mfa` first.",
        ));
    }

    Ok(Json(ViewerResource::from(user)))
}

/// End this session.
///
/// Per session, not everywhere. "Sign out everywhere" is a security action
/// somebody takes from their own account screen after something has gone wrong,
/// next to the list of devices; it does not belong behind a logout button.
#[utoipa::path(
    post,
    path = "/auth/sign-out",
    tag = "auth",
    operation_id = "signOut",
    responses(
        (status = 204, description = "The session is over"),
        (status = 401, description = "No credential was presented", body = Problem),
    ),
    security(("api_key" = [])),
)]
pub async fn sign_out(workspace: ApiWorkspace) -> Result<StatusCode, Problem> {
    let Some(token) = bearer_of(&workspace.headers) else {
        return Err(Problem::unauthenticated("No session token was presented."));
    };

    // A token that was already dead is a sign-out that already happened, and
    // 204 is the honest answer to "make this session not exist".
    let user_id = session_service::resume(
        &workspace.pool,
        &token,
        &workspace.state.config.security.session,
    )
    .await?
    .map(|session| session.user_id);

    authentication::sign_out(&workspace.pool, &token, user_id).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_has_no_status_and_every_other_outcome_does() {
        // The split this surface is built on: a refusal is a problem document,
        // and everything else is a 200 carrying a token. A new `LoginResult`
        // variant has to be placed on one side or the other here, and the
        // exhaustive match in `SessionStatus::of` is what forces the choice.
        assert!(SessionStatus::of(&LoginResult::Rejected).is_none());
        assert!(SessionStatus::of(&LoginResult::Locked { retry_after_secs: 30 }).is_none());
    }

    #[test]
    fn every_outcome_that_holds_a_token_names_where_to_go_next() {
        // The three "yes, but" outcomes each return a real token that reaches
        // exactly one endpoint. What must not happen is a new one arriving and
        // being reported as `signed_in` by a wildcard arm - so this pins each
        // to a distinct status rather than only checking that it has one.
        let user_id = phonix_core::identity::UserId::from(uuid::Uuid::nil());

        assert_eq!(
            SessionStatus::of(&LoginResult::MfaRequired { user_id }),
            Some(SessionStatus::MfaRequired)
        );
        assert_eq!(
            SessionStatus::of(&LoginResult::MfaEnrolmentRequired { user_id }),
            Some(SessionStatus::MfaEnrolmentRequired)
        );
        assert_eq!(
            SessionStatus::of(&LoginResult::PasswordChangeRequired { user_id }),
            Some(SessionStatus::PasswordChangeRequired)
        );
    }
}
