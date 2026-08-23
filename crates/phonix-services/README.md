# phonix-services — the application layer

Use cases. A repository answers *what is in this row*; a component answers *what
does the user see*; this crate answers **what happens when somebody does X**.

X is usually several rows, a decision or two, and something that must not be
half-done if the process dies in the middle.

![architecture](../../docs/architecture.svg)

## What lives here

| Module        | Use cases                                                    |
| ------------- | ------------------------------------------------------------ |
| `identity/`   | `sign_in`, `answer_challenge`, `change_own_password`, `begin_totp_enrolment`, `generate_recovery_codes`, `sign_out` |
| `workspace/`  | `onboard_workspace`, `settings::save`                        |
| `caller`      | Who is asking, and whether they may                          |
| `crypto/`     | Not a use case — the primitives the others need               |

A new domain becomes a folder here: `inventory/` beside `identity/`, with its
rows in `phonix_db::inventory` and its vocabulary in `phonix_core::inventory`.

## Authorization happens here

Every use case that changes something takes a `Caller` and names its permission
on the first line:

```rust
pub async fn create_requisition(pool: &PgPool, caller: &Caller, ..) -> ServiceResult<..> {
    caller.require(permissions::INVENTORY_REQUISITION_CREATE)?;
    ..
}
```

A route guard would protect a URL. A use case is reachable from a server
function, another use case, a background job and any future API — four places to
forget. Checking where the work happens means the check cannot be routed around,
and it puts the required permission next to the code that needs it.

`Caller` has four gates:

| Call                              | Means                                            |
| --------------------------------- | ------------------------------------------------ |
| `require(p)`                      | must hold `p`                                    |
| `require_all(&[..])`              | must hold every one                              |
| `require_any(&[..])`              | must hold at least one                           |
| `require_self_or(user_id, p)`     | acting on your own account, or holds `p` for others |

Two properties worth knowing:

- **A half-authenticated caller holds nothing.** A session that has proven a
  password but not its second factor fails every check, including
  `require_self_or` on its own account — that is exactly the session an attacker
  with a stolen password holds.
- **`Caller::system("reason")` passes everything.** It exists for onboarding
  (before an owner exists), scheduled sweeps and redeemed reset links. It is a
  named variant rather than an absent check, so it is visible in review.

The UI still hides what a user cannot do. That is a courtesy on top, not the
control.

## Why the crypto is here

Hashing a password, sealing a TOTP secret and minting a session token are things
a *use case does*, not things a table stores. Keeping them here is what lets
`phonix-db` hold its own rule — it never sees a usable credential.

They are equally not in `phonix-core`, which ships to the browser.

| `crypto/`   | Does                                                      |
| ----------- | --------------------------------------------------------- |
| `password`  | Argon2id at the configured cost, plus `verify_dummy` so a missing account costs what a real one does |
| `token`     | 32 CSPRNG bytes → URL-safe base64; SHA-256 for storage     |
| `totp`      | RFC 6238/4226, constant-time comparison, bounded drift     |
| `vault`     | XChaCha20-Poly1305, AAD-bound to the owning user           |

## Outcomes are not errors

A wrong password, a wrong TOTP code and a taken workspace name come back as
`Ok(LoginResult::Rejected)`, `Ok(MfaChallengeResult::Rejected { .. })` and
`Ok(SignupResult::Rejected(..))`. They are the expected path through a form, and
modelling them as failures makes every caller unwrap something that happens all
day long. `ServiceError` is for the storage failing, a key being unusable, or
the caller not being permitted.

## How it connects

```text
phonix-web ────> phonix-services ──> phonix-db ──> PostgreSQL
phonix-server ─> phonix-services         │
                        └──────────────> phonix-core   (the rules it applies)
```

## Tests

56 unit tests, none needing a database: the RFC 6238 and RFC 4226 test vectors,
the vault's tamper and cross-user checks, the recovery-code alphabet, the
permission gates, and the sign-in outcome table.
