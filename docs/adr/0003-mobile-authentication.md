# ADR 0003 — Signing a person in from a phone

Status: accepted, built 2026-08-29
Date: 2026-08-29

[ADR 0002](0002-public-api.md) built one credential and said, in §3, what it
deliberately was not:

> **Not a way to sign in.** There is no "log in with an API key" screen, and a
> key never produces a session cookie.
> **Not a refresh/access token pair.** OAuth client credentials, per-key IP
> allowlists and short-lived exchanged tokens are all reasonable and all later.

"Later" is now. An API key answers *a machine acting on behalf of one person who
issued it*. It cannot answer *a person signing in on their own phone*, because
there is no key to ship inside an application a thousand people install, and
because a credential minted in an administration panel is not something you can
ask an employee to paste into a login form.

So this record adds a **second credential to the same door**, and its whole
argument is about how small that addition is allowed to be.

---

## 1. A mobile sign-in produces a session, not a new kind of thing

The tempting shape here is a new subsystem: an `access_tokens` table, a refresh
token beside it, a rotation scheme, an expiry policy, a revocation path, and a
screen listing them. Every one of those already exists, once, in `sessions`.

Look at what that table was built to do, in its own words
(`migrations/apps/core/0002_identity.sql`):

> Server-side sessions rather than a self-contained signed token. The cost is
> one indexed lookup per request; what it buys is instant revocation - sign out
> everywhere, suspend an account, respond to a stolen laptop - which a JWT
> cannot do without exactly this table anyway.

It already carries an opaque token's SHA-256 digest and never the token; a
sliding `expires_at` and an immovable `absolute_expires_at`; `revoked_at` with a
reason; `mfa_satisfied`, so a half-finished login has somewhere to live; and
`ip`/`user_agent` for a person auditing their own account. `session::resume`
already slides the deadline, `sign_out` already ends it, suspending an account
already empties it because `AuthUser::can` returns false for a suspended user.

None of that is browser-specific. **The only thing that is browser-specific is
the envelope**: the token travels in a `Set-Cookie` header instead of a JSON
body. So:

> **A mobile session is a session. It is delivered as a bearer token instead of
> a cookie, and that is the entire difference.**

The extension point for this already exists and was written for a different
reason. `authentication::Delivery` has two variants today — `Cookie` for a
sign-in on the workspace's own host, `Handoff` for one that has to carry a new
session across hosts. This adds a third:

```rust
pub enum Delivery {
    Cookie,   // set it here
    Handoff,  // hand a one-time token to redeem on the workspace host
    Bearer,   // hand the session token back in the response body
}
```

`sign_in` does not otherwise change. The lockout check, the timing-equalised
dummy verify, the audit entry, the password-age policy and every `LoginResult`
variant are reached identically whichever envelope was asked for — which is the
point. A second sign-in path is a second place for a security control to be
forgotten, and there is not going to be one.

### What this rules out, and why that is the right trade

A **JWT** would remove the per-request lookup. It would also remove revocation,
and the paragraph quoted above already refused that trade for browsers. A phone
is *more* likely to be lost than a laptop, not less. One indexed lookup on a
32-byte digest is the same cost the API key path already pays.

An **access/refresh pair** exists to bound the damage of a long-lived credential
on a server that cannot revoke. We can revoke, in one statement, and the
administration side of it is already built. Rotation would buy nothing here and
would cost a second token, a second table or column, a replay-detection rule for
a rotated refresh token used twice, and support for all of it for the life of
`v1`. If a third-party OAuth surface is ever wanted, it is a separate record and
it does not want to be retrofitted onto this one.

## 2. Two credentials, one door

`ApiCaller::from_request_parts` is the single place a request becomes a
[`Caller`], and everything downstream — `Caller::require` inside each use case —
is indifferent to how that caller was proved. So the branch is one line deep:

```
Authorization: Bearer phx_…   →  API key   →  owner ∩ scopes   →  Caller
Authorization: Bearer <other> →  session   →  the user         →  Caller
```

The `phx_` prefix earns a second job here. ADR 0002 introduced it so a secret
scanner could recognise our credential in a public repository; it is now also
what tells the extractor which lookup to do, with no second header, no `?type=`
and no trying both.

Both branches then answer identically:

* **`401`** — no credential, or one that is not live. Deliberately one answer
  for unknown, expired, revoked and owned-by-a-suspended-account, exactly as the
  key path already does.
* **`403`** — live, but its effective permissions do not cover the operation,
  from `Caller::require` inside the use case.

A session-backed caller is **not** narrowed by scopes: it is the person, with
their current permissions, the same set the browser resolves. That is what
distinguishes the two credentials and it is worth stating plainly — a key is
*at most* its owner and usually less; a mobile session simply *is* its owner.

### Roles do not come into it

Worth writing down because it is the question everybody asks. Phonix has no
active-role concept: `AuthUser.roles` is `Vec<String>` and its doc comment says
"For display; authority comes from `permissions`", which is the already-flattened
union of role grants and individual overrides. There is nothing to choose between
at sign-in, on a phone or in a browser. A caller who wants *less* than their
whole self wants an API key with scopes, which is the thing that already exists.

## 3. This is not gated by `api_enabled`

ADR 0002 §4 made `workspace_settings.api_enabled` a licence: default false,
checked before the credential is even looked up, so a workspace that has not
been sold the API cannot be probed for valid keys.

**Mobile sign-in is outside it.** A customer's staff opening the phone app are
*using the product*; a customer's script calling `/api/v1/currencies` with a key
is *integrating with* it. Folding the first into the second's licence would mean
anybody who wants the phone app has to buy "API access", which is a pricing
accident rather than a decision.

Concretely: `/api/v1/auth/*` and any request authenticated by a **session**
bearer skip the `api_enabled` check. A request authenticated by an **API key**
still fails it. One flag, unchanged in meaning, applied to the credential it was
written about.

The residual is honest and small: a determined customer could drive the public
endpoints with a session token obtained by signing a real user in, and pay
nothing. They would be doing it with a human's password, against endpoints that
already enforce that human's permissions, at mobile session lifetimes. If that
ever needs closing it is closed with a flag, not with a redesign.

## 4. Its own lifetimes

`[security.session]` is tuned for a browser: 12 hours idle, 7 days absolute, 30
days with "remember me". A phone application signed out weekly is an application
people stop opening.

So `[security.session.mobile]` overrides the two deadlines and nothing else:

```toml
[security.session.mobile]
# Sliding, as the browser's is. 30 days: a phone that is used at all keeps
# itself signed in, and one left in a drawer does not.
idle_timeout_mins = 43200
# Hard ceiling activity cannot extend. 90 days, after which the person signs
# in again with their password and their second factor.
absolute_timeout_days = 90
```

Only the deadlines. Cookie name, `Secure`, `SameSite` and the handoff TTL are
all properties of a cookie and there is no cookie here, so the mobile block does
not have them and cannot be misconfigured with them.

`sessions` grows one column to make this possible and to make it visible:

```sql
ALTER TABLE sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'browser';
-- CHECK (kind IN ('browser', 'mobile'))
```

Which lifetimes to apply is the immediate reason. The lasting reason is the
screen where a person reviews the devices holding their account: "Chrome on
Windows, this browser" and "the phone app, last seen an hour ago" are different
facts, and a list that shows both as a user-agent string is one nobody reads.

## 5. Second factors travel unchanged

`LoginResult` already models every "yes, but" a sign-in can reach — `MfaRequired`,
`MfaEnrolmentRequired`, `PasswordChangeRequired`, `Rejected`, `Locked` — and
`Caller` already guarantees the safety underneath them: a session that has not
satisfied MFA fails every permission check, because `AuthUser::can` returns false
until `is_fully_authenticated`.

So the token endpoint returns those outcomes rather than hiding them:

| Outcome | Status | Body | What the app does |
| - | - | - | - |
| `Success` | 200 | `status: "signed_in"` | Store the token; it is signed in |
| `MfaRequired` | 200 | `status: "mfa_required"` | Token *is* returned, and reaches only `POST /auth/mfa` |
| `MfaEnrolmentRequired` | 200 | `status: "mfa_enrolment_required"` | Same; enrolment is not on this surface yet, so finish in a browser |
| `PasswordChangeRequired` | 200 | `status: "password_change_required"` | Same, for a password change |
| `Rejected` | 401 | `code: "invalid_credentials"` | One answer for wrong password, no such account, suspended |
| `Locked` | 429 | `code: "account_locked"` | `Retry-After`; the wait is not a secret, the caller caused it |

**Amended during the build:** the four 200s carry a `status` field on the
session object, not a `code` on a problem document. `code` is the machine half
of an *error* (ADR 0002 §5), and these are not errors - each returns a real
token, and the client's next step is to use it. Putting them in a problem body
would have meant a 200 whose body was shaped like a failure, which is the kind
of thing a client library unwraps into an exception. `SessionStatus::of` is an
exhaustive match, so a new `LoginResult` variant has to be placed on one side of
the split or the other before it will compile.

The three middle rows return a real token on purpose, and it is the same
arrangement the browser uses: the half-authenticated session exists so the
challenge has something to attach to, and it can reach nothing else. An
application that ignores the `code` and tries to use that token gets a `403`
from the first endpoint it touches, which is the correct outcome for a client
that skipped a step.

## 6. Rate limiting: this endpoint is a password oracle

ADR 0002 §7 keys the `/api/v1` tier on **the credential**, because a mobile
fleet shares an IP and the credential is the thing whose behaviour we mean to
bound. That reasoning inverts for a sign-in: there is no credential yet, and the
thing worth bounding is *guessing*.

`POST /api/v1/auth/token` and `POST /api/v1/auth/mfa` are therefore counted in
**`Tier::Action`** - the credential tier the browser's own `/api/sign-in`,
`/api/mfa-challenge` and password-reset endpoints already use - rather than in
`Tier::Api`. They are matched *above* the `/api/v1` line in `classify`, which
would otherwise swallow them.

This sits *on top of* account lockout rather than replacing it. Lockout is the
per-account defence and is already generous and self-expiring, deliberately,
because it is itself a denial-of-service vector; the limiter is the per-source
defence and does not care which account is being tried.

**Amended during the build:** this record originally said the key should be the
address *and the submitted email address together*. It is the address alone, as
every other credential endpoint's is. The limiter runs as middleware above
tenant resolution, before any handler has read the body - so keying on the email
would mean buffering every request body through that layer to extract one field,
paid on all traffic, to narrow a defence that account lockout already provides
per account. The gain did not justify the shape. If per-account limiting is ever
wanted at this layer, the honest way is to count *after* authentication, which is
the same fix ADR 0002 §7 already names as the proper close for its own residual.

## 7. The endpoints

```
POST   /api/v1/auth/token       email + password        → session token, or an outcome
POST   /api/v1/auth/mfa         token + code            → satisfies the second factor
GET    /api/v1/auth/me          the signed-in person
POST   /api/v1/auth/sign-out    ends this session
```

Four, and the reasons the obvious fifth is missing:

* **No refresh endpoint.** The session slides on use, and `session::resume`
  already does the sliding. An application that is opened inside the idle window
  never needs one; one that is not needs a real sign-in, which is the point.
* **`/auth/me` rather than making the client decode a token.** It answers the
  resolved `AuthUser` — display name, roles for display, permissions — so the
  phone can hide what the person cannot do, exactly as the browser shell does,
  and re-fetch it when a grant changes rather than trusting a claim minted at
  sign-in.
* **Sign-out is per session, not everywhere.** "Sign out everywhere" is a
  security action a person takes from their own account screen after something
  has gone wrong, and it belongs there, next to the device list, rather than in
  a mobile client's logout button.

`/auth/token` and the sign-in itself are unauthenticated; the other three take a
session bearer. All four are in the OpenAPI document, because an endpoint that is
not in the document is an endpoint nobody outside the building can find.

## 8. What is deliberately not here

* **OAuth 2.0 / third-party clients.** Everything above assumes *our* client,
  which is why a password can be sent to it at all. The day somebody else's
  application wants to act for a Phonix user, they need an authorization-code
  flow, a client registry, consent, and a record of its own.
* **Push notification tokens, biometric unlock, device attestation.** Properties
  of a mobile application, not of authentication, and all reachable later
  against a `sessions.kind` that already distinguishes the two.
* **Per-device revocation UI.** The column and the data land here; the screen
  that lists a person's devices is worth building when there is an application
  producing them.
* **Offline or optimistic writes.** A synchronisation problem wearing an
  authentication problem's clothes.

## Consequences

* `sessions` now has two envelopes and one meaning. Anything added to the
  session — a new deadline, a new revocation reason, a new challenge state —
  is automatically true of both, which is the reason for doing it this way and
  also the thing to be careful about: a change made "for the browser" now
  reaches a phone.
* `Delivery` gains a variant, so every place that matches on it is a compile
  error until it says what it does about `Bearer`. That is the intended cost.
* The `/api/v1` extractor now has two credential paths. They converge on
  `Caller` within a few lines and must be kept converging; a check that exists
  on one path and not the other is the failure this design is arranged to make
  obvious.
* `api_enabled` stops meaning "everything under `/api/v1`" and starts meaning
  "the API-key surface". The flag's own documentation and the administration
  screen's wording both have to say so, or an administrator will read the switch
  as one that turns the phone app off.

---

## What this ADR shipped

* Migration `0021_mobile_sessions.sql`: `sessions.kind`, defaulting to
  `browser`, with the `sessions_kind_known` check constraint.
* `phonix_core::identity::SessionKind` - a closed enum whose `FromStr` refuses
  an unknown stored value rather than guessing.
* `[security.session.mobile]` (`idle_timeout_mins`, `absolute_timeout_days`),
  validated by the same two rules the browser block gets.
* `phonix_db::identity::session`: `windows()` as the single place either kind's
  deadlines are chosen, `create` taking a kind, and a `CASE` in `touch` so the
  idle window is picked from the row rather than from the caller - which only
  holds a token and cannot know.
* `Delivery::Bearer`, sharing one arm of `finish_sign_in` with `Cookie`.
* `phonix_server::api::session`: `POST /auth/token`, `POST /auth/mfa`,
  `GET /auth/me`, `POST /auth/sign-out`, all in the OpenAPI document.
* `ApiCaller` accepting either credential on the `phx_` prefix, with
  `ApiWorkspace` for the endpoints that run before anybody is authenticated.
* `Problem::retry_after`, so a 429 cannot be built without the header a client
  actually backs off on.
* `Tier::Action` for the two credential endpoints, matched above the
  `/api/v1` line.

**Not built, and named so nobody assumes otherwise:** MFA *enrolment* and the
forced password change are not on this surface, so those two statuses mean
"finish in a browser" today. The device list that `sessions.kind` exists to feed
is not built either - the data starts being recorded now because a column added
later is null for every session that mattered.

**Verify with** `cargo test -p phonix-db --test session_kind -- --ignored
--test-threads=1`, which needs Postgres and also proves migration 0021 applies
to a workspace built from nothing.
