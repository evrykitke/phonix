//! Signing in with Google.
//!
//! The OAuth 2.0 authorization-code flow, with PKCE, against Google's OpenID
//! Connect endpoints. Four steps: send the browser to Google, get a `code`
//! back, trade it server-to-server for an ID token, read an email out of it.
//!
//! # What this deliberately cannot do
//!
//! **It never creates an account.** The email it recovers has to already
//! belong to a member of the workspace, or the sign-in fails. That is the same
//! rule the signup screens follow - a workspace is joined by invitation, and
//! the only place an account comes into existence is a form somebody filled
//! in - and it is what stops "Continue with Google" from being an open door
//! into a private workspace for anybody with a Google account.
//!
//! # Why the ID token's signature is not checked here
//!
//! Google signs the ID token, and there is a JWKS endpoint to verify it
//! against. Verification matters when a token arrives from somewhere untrusted
//! - the implicit flow, or a token forwarded by a client.
//!
//! It does not arrive that way here. [`exchange_code`] is a direct HTTPS POST
//! to Google's token endpoint, authenticated with the client secret, and the
//! response comes back over that connection. The chain of custody is TLS to a
//! pinned host, not a signature, and OpenID Connect Core says as much: §3.1.3.7
//! excuses signature validation when the token is received directly from the
//! token endpoint over a protected channel.
//!
//! What follows from that is a rule with teeth: **the claims below are only
//! trustworthy because of where they came from.** An ID token reaching this
//! module by any other route - a query parameter, a header, a client-side
//! `credential` from Google's One Tap button - has no such guarantee and must
//! not be fed to [`Claims`] without full JWKS verification first.
//!
//! # `email_verified` is not decoration
//!
//! Sign-in is matched on the email address, so an unverified one would let
//! anybody who can create a Google account with an arbitrary address walk into
//! whichever workspace has a member by that name. The claim is checked, and a
//! `false` is a refusal.
//!
//! There is a residual risk this cannot close, and it is worth naming: on a
//! Google Workspace domain, an administrator can create a mailbox at any
//! address in that domain and it will be `email_verified`. Federating on email
//! therefore trusts whoever controls the DNS of every domain your members use.
//! That is inherent to email-based federation rather than specific to this
//! implementation, and `hosted_domain` is the lever for a deployment that wants
//! to narrow it.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use phonix_config::GoogleConfig;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{ServiceError, ServiceResult};

/// Where a browser is sent to sign in.
const AUTHORIZE_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";

/// Where a code is traded for tokens. Server-to-server, never in a browser.
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// Ceiling on the token exchange.
///
/// A sign-in is a person waiting, and a hung request to Google must not hold a
/// connection open behind them. Short enough to fail visibly rather than
/// silently stall.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

/// The scopes asked for, and no more.
///
/// `openid email` and nothing else - not `profile`, not a name, not a picture.
/// This flow needs an address to match against an account that already exists,
/// so anything further would be data collected because it was on offer. The
/// consent screen shows what is asked for, and a short list is a screen people
/// say yes to.
const SCOPES: &str = "openid email";

/// The one-time material a sign-in attempt carries between the two requests.
///
/// Held in a cookie on the host that started the flow, never in the redirect,
/// and checked when Google comes back. The two fields answer two different
/// attacks and neither substitutes for the other.
pub struct Pending {
    /// Echoed by Google in the callback and compared byte for byte.
    ///
    /// This is the CSRF defence: without it, an attacker can send somebody a
    /// link to a callback URL carrying *the attacker's* code, and the victim
    /// signs in to the attacker's account without noticing.
    pub state: String,
    /// The PKCE verifier. Its SHA-256 goes to Google in the first request; the
    /// verifier itself goes in the second.
    ///
    /// Defends the code between Google and this server: a code intercepted in
    /// a redirect, a log or a `Referer` cannot be exchanged without it. The
    /// client secret already makes that hard, and PKCE costs one hash.
    pub verifier: SecretString,
}

impl Pending {
    /// Draw fresh state and a fresh verifier from the OS CSPRNG.
    pub fn generate() -> Self {
        Self {
            state: random_url_safe(),
            verifier: SecretString::from(random_url_safe()),
        }
    }

    /// The `code_challenge` that goes to Google: base64url(SHA-256(verifier)).
    fn challenge(&self) -> String {
        let digest = Sha256::digest(self.verifier.expose_secret().as_bytes());
        URL_SAFE_NO_PAD.encode(digest)
    }
}

/// 32 bytes from the OS CSPRNG, base64url, no padding.
fn random_url_safe() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};

    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Build the URL to send the browser to.
///
/// `workspace` is carried through as `login_hint`'s neighbour rather than in
/// `state`: state is compared for equality against a cookie, and stuffing a
/// payload into it would mean the comparison had to parse rather than match.
/// The workspace instead rides in the callback's own query, having been put
/// there by the caller - see the `redirect_uri` note in [`GoogleConfig`].
pub fn authorize_url(config: &GoogleConfig, pending: &Pending) -> String {
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", &config.redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPES)
        .append_pair("state", &pending.state)
        .append_pair("code_challenge", &pending.challenge())
        .append_pair("code_challenge_method", "S256")
        // Ask for an account picker every time rather than silently reusing
        // whichever Google account the browser last used. On a shared machine
        // the silent path signs somebody in as the previous person, and the
        // only visible symptom is the wrong name in the corner.
        .append_pair("prompt", "select_account")
        .finish();

    // `hd` narrows the accounts Google will even offer to one hosted domain.
    // Advisory on its own - it is a hint, and the claim below is what is
    // actually checked - so it is set for the account picker's benefit and
    // enforced separately in `Claims::email_for`.
    let hosted = match &config.hosted_domain {
        Some(domain) if !domain.trim().is_empty() => {
            format!("&hd={}", urlencode(domain.trim()))
        }
        _ => String::new(),
    };

    format!("{AUTHORIZE_ENDPOINT}?{query}{hosted}")
}

/// What Google's token endpoint returns. Only the field this needs is read.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    id_token: String,
}

/// The claims this flow acts on.
///
/// A deliberately small subset. `sub`, `name` and `picture` are all in the
/// token and none of them is read: matching is by verified email, and a field
/// that is parsed is a field somebody will later be tempted to store.
#[derive(Debug, Deserialize)]
pub struct Claims {
    #[serde(default)]
    pub email: String,
    /// Whether Google says this address has been proved.
    ///
    /// Absent is treated as `false` by the `Default`, which is the safe
    /// reading: a token that does not say the address was verified has not
    /// said it was.
    #[serde(default)]
    pub email_verified: bool,
    /// The Google Workspace domain, when the account belongs to one. Absent
    /// for a personal `@gmail.com` account.
    #[serde(default)]
    pub hd: Option<String>,
}

impl Claims {
    /// The address to match an account against, or why there is not one.
    ///
    /// Every refusal here is a refusal to sign anybody in. They are separate
    /// variants so the log says which, but the screen says one thing - see the
    /// caller.
    pub fn email_for(&self, config: &GoogleConfig) -> Result<&str, ClaimProblem> {
        if !self.email_verified {
            return Err(ClaimProblem::EmailNotVerified);
        }

        let email = self.email.trim();
        if email.is_empty() {
            return Err(ClaimProblem::NoEmail);
        }

        if let Some(required) = config.hosted_domain.as_deref()
            && !required.trim().is_empty()
        {
            // Checked against the claim, not against the address: `hd` is what
            // Google asserts about the account, and an address ending in the
            // right characters is not the same thing as an account in that
            // domain.
            let matches = self
                .hd
                .as_deref()
                .is_some_and(|actual| actual.eq_ignore_ascii_case(required.trim()));

            if !matches {
                return Err(ClaimProblem::WrongDomain);
            }
        }

        Ok(email)
    }
}

/// Why a token that arrived intact still cannot sign anybody in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimProblem {
    /// Google will not vouch for the address. See the module note on why this
    /// is a refusal and not a warning.
    EmailNotVerified,
    /// No address in the token at all, which means the `email` scope was not
    /// granted.
    NoEmail,
    /// The deployment restricts sign-in to one hosted domain and this account
    /// is not in it.
    WrongDomain,
}

/// Trade the authorization code for an ID token, and read its claims.
///
/// Server-to-server over TLS, authenticated with the client secret. The secret
/// never reaches a browser, which is the entire reason this step exists rather
/// than the browser being handed a token directly.
pub async fn exchange_code(
    config: &GoogleConfig,
    code: &str,
    pending_verifier: &SecretString,
) -> ServiceResult<Claims> {
    let client = reqwest::Client::builder()
        .timeout(EXCHANGE_TIMEOUT)
        // No redirects. The token endpoint answers directly, and a redirect
        // from it would mean something has gone wrong in a way worth failing
        // on rather than following.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|err| ServiceError::Upstream(format!("http client: {err}")))?;

    let response = client
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.expose_secret()),
            ("code", code),
            ("code_verifier", pending_verifier.expose_secret()),
            ("grant_type", "authorization_code"),
            ("redirect_uri", config.redirect_uri.as_str()),
        ])
        .send()
        .await
        .map_err(|err| {
            // The error can name the host but never the secret - `form` values
            // are not in `reqwest`'s error display.
            ServiceError::Upstream(format!("could not reach Google: {err}"))
        })?;

    let status = response.status();
    if !status.is_success() {
        // Google's body explains which of a dozen things was wrong - an expired
        // code, a redirect_uri that does not match what was registered - and
        // that is exactly what somebody setting this up needs. It goes to the
        // log; the screen gets a fixed string.
        let body = response.text().await.unwrap_or_default();
        tracing::warn!(%status, body = %truncate(&body), "Google refused the token exchange");

        return Err(ServiceError::Upstream(
            "Google refused the sign-in".to_owned(),
        ));
    }

    let tokens: TokenResponse = response
        .json()
        .await
        .map_err(|err| ServiceError::Upstream(format!("unreadable token response: {err}")))?;

    decode_claims(&tokens.id_token)
}

/// Read the claims out of a JWT's payload.
///
/// **Decode, not verify.** Safe only for a token that came straight back from
/// the token endpoint over TLS - see the module note. The signature is not
/// looked at, so this must never be pointed at a token from anywhere else.
fn decode_claims(id_token: &str) -> ServiceResult<Claims> {
    let mut parts = id_token.split('.');
    let (Some(_header), Some(payload), Some(_signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ServiceError::Upstream(
            "Google returned a malformed ID token".to_owned(),
        ));
    };

    let decoded = URL_SAFE_NO_PAD.decode(payload).map_err(|_| {
        ServiceError::Upstream("Google's ID token is not valid base64url".to_owned())
    })?;

    serde_json::from_slice(&decoded)
        .map_err(|err| ServiceError::Upstream(format!("unreadable ID token claims: {err}")))
}

/// Percent-encode one query value.
fn urlencode(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// Keep a logged error body to something a log line can hold.
fn truncate(body: &str) -> String {
    const LIMIT: usize = 500;

    if body.len() <= LIMIT {
        return body.to_owned();
    }

    let cut = body
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= LIMIT)
        .last()
        .unwrap_or(0);

    format!("{}...", &body[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> GoogleConfig {
        GoogleConfig {
            enabled: true,
            client_id: "123.apps.googleusercontent.com".to_owned(),
            client_secret: SecretString::from("not-a-real-secret"),
            redirect_uri: "https://phonix.example.com/auth/google/callback".to_owned(),
            hosted_domain: None,
        }
    }

    fn claims(email: &str, verified: bool, hd: Option<&str>) -> Claims {
        Claims {
            email: email.to_owned(),
            email_verified: verified,
            hd: hd.map(str::to_owned),
        }
    }

    #[test]
    fn the_authorize_url_carries_everything_google_requires() {
        let pending = Pending::generate();
        let url = authorize_url(&config(), &pending);

        assert!(url.starts_with(AUTHORIZE_ENDPOINT));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("state={}", pending.state)));
        assert!(url.contains("scope=openid+email"));
        // The redirect must arrive percent-encoded, or Google compares a
        // mangled string against what was registered and refuses.
        assert!(
            url.contains(
                "redirect_uri=https%3A%2F%2Fphonix.example.com%2Fauth%2Fgoogle%2Fcallback"
            )
        );
    }

    #[test]
    fn the_verifier_never_travels_in_the_first_request() {
        // PKCE's whole point: the first request carries only the hash. A
        // verifier in the authorize URL would be a verifier in browser history.
        let pending = Pending::generate();
        let url = authorize_url(&config(), &pending);

        assert!(!url.contains(pending.verifier.expose_secret()));
        assert!(url.contains(&urlencode(&pending.challenge())));
    }

    #[test]
    fn the_challenge_is_the_sha256_of_the_verifier() {
        let pending = Pending::generate();

        let expected =
            URL_SAFE_NO_PAD.encode(Sha256::digest(pending.verifier.expose_secret().as_bytes()));

        assert_eq!(pending.challenge(), expected);
    }

    #[test]
    fn two_attempts_share_nothing() {
        let one = Pending::generate();
        let two = Pending::generate();

        assert_ne!(one.state, two.state);
        assert_ne!(
            one.verifier.expose_secret(),
            two.verifier.expose_secret(),
            "a reused PKCE verifier defeats the point of having one",
        );
    }

    #[test]
    fn an_unverified_address_is_refused() {
        // The one check standing between this flow and anybody who can put an
        // arbitrary address on a Google account.
        assert_eq!(
            claims("someone@example.com", false, None).email_for(&config()),
            Err(ClaimProblem::EmailNotVerified),
        );
    }

    #[test]
    fn a_token_with_no_address_is_refused() {
        assert_eq!(
            claims("", true, None).email_for(&config()),
            Err(ClaimProblem::NoEmail),
        );
        assert_eq!(
            claims("   ", true, None).email_for(&config()),
            Err(ClaimProblem::NoEmail),
        );
    }

    #[test]
    fn a_verified_address_comes_back_trimmed() {
        assert_eq!(
            claims("  someone@example.com  ", true, None).email_for(&config()),
            Ok("someone@example.com"),
        );
    }

    #[test]
    fn a_hosted_domain_restriction_is_checked_against_the_claim() {
        let mut config = config();
        config.hosted_domain = Some("acme.com".to_owned());

        assert_eq!(
            claims("someone@acme.com", true, Some("acme.com")).email_for(&config),
            Ok("someone@acme.com"),
        );
        assert_eq!(
            claims("someone@acme.com", true, Some("ACME.COM")).email_for(&config),
            Ok("someone@acme.com"),
            "the domain comparison is case-insensitive",
        );

        // The address ends in the right characters and the account is not in
        // the domain. This is the case a string suffix check would wave
        // through, which is why the claim is what gets compared.
        assert_eq!(
            claims("someone@acme.com", true, None).email_for(&config),
            Err(ClaimProblem::WrongDomain),
        );
        assert_eq!(
            claims(
                "someone@acme.com.attacker.test",
                true,
                Some("attacker.test")
            )
            .email_for(&config),
            Err(ClaimProblem::WrongDomain),
        );
    }

    #[test]
    fn an_unset_hosted_domain_restricts_nothing() {
        for hd in [None, Some("acme.com"), Some("gmail.com")] {
            assert_eq!(
                claims("someone@example.com", true, hd).email_for(&config()),
                Ok("someone@example.com"),
            );
        }

        // An empty string in the TOML means "not set", not "match the domain
        // whose name is the empty string".
        let mut blank = config();
        blank.hosted_domain = Some("  ".to_owned());
        assert_eq!(
            claims("someone@example.com", true, None).email_for(&blank),
            Ok("someone@example.com"),
        );
    }

    #[test]
    fn a_jwt_payload_is_read_without_its_signature() {
        let payload = URL_SAFE_NO_PAD
            .encode(br#"{"email":"someone@example.com","email_verified":true,"hd":"acme.com"}"#);
        let token = format!("header.{payload}.signature");

        let claims = decode_claims(&token).expect("a readable payload");

        assert_eq!(claims.email, "someone@example.com");
        assert!(claims.email_verified);
        assert_eq!(claims.hd.as_deref(), Some("acme.com"));
    }

    #[test]
    fn a_claim_google_omitted_reads_as_absent_rather_than_true() {
        let payload = URL_SAFE_NO_PAD.encode(br#"{"email":"someone@example.com"}"#);
        let claims = decode_claims(&format!("header.{payload}.signature")).expect("readable");

        assert!(
            !claims.email_verified,
            "a token that does not say the address was verified has not said it was",
        );
        assert_eq!(
            claims.email_for(&config()),
            Err(ClaimProblem::EmailNotVerified)
        );
    }

    #[test]
    fn anything_that_is_not_a_three_part_jwt_is_refused() {
        for bad in [
            "",
            "one-part",
            "two.parts",
            "four.parts.are.too.many",
            "header.!!!not-base64!!!.signature",
        ] {
            assert!(decode_claims(bad).is_err(), "{bad:?} should not decode");
        }
    }

    #[test]
    fn a_long_error_body_is_cut_to_something_a_log_line_can_hold() {
        assert_eq!(truncate("short"), "short");

        let long = "x".repeat(2_000);
        let cut = truncate(&long);

        assert!(cut.len() < 600);
        assert!(cut.ends_with("..."));
    }

    #[test]
    fn truncation_does_not_split_a_character() {
        // A body of multi-byte characters is the case a naive `&body[..500]`
        // panics on, and a panic while logging an error is the worst place for
        // one.
        let long = "é".repeat(2_000);
        let cut = truncate(&long);

        assert!(cut.ends_with("..."));
        assert!(cut.chars().count() > 1);
    }
}
