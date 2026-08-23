# phonix-core — the domain layer

The vocabulary and the rules. Entities, value objects, policies and invariants,
with no I/O of any kind: no `tokio`, no `sqlx`, no `redis`, no `lapin`.

This crate compiles **twice** — once for the server, once to WebAssembly for the
browser — which is the constraint that shapes everything in it.

![architecture](../../docs/architecture.svg)

## What lives here

| Folder           | Answers                                            |
| ---------------- | -------------------------------------------------- |
| `tenant/`        | Which workspace is this, and what has it configured? |
| `identity/`      | Who is signed in, and what may their password be?  |
| `authorization/` | What may they do?                                  |
| `error.rs`       | What crosses back to the browser on failure?       |

A new domain (inventory, billing) becomes a folder beside these. It does not
become a crate.

## The rule that decides what belongs here

> **`phonix-core` holds what the client must also enforce, and nothing that
> would hand the client a capability.**

`PasswordPolicy` is here because the sign-up form has to draw the same checklist
the server applies — otherwise the browser shows a green field the server
rejects. `PermissionSet` is here because a view needs `user.can(..)` to decide
what to render.

The Argon2 parameters, the TOTP arithmetic and the encryption key are **not**
here, and never will be. A client able to hash with the server's parameters or
produce a code from a secret holds something it has no business holding. Those
live in `phonix-services/crypto`.

## How it connects

```text
phonix-web ─────┐
phonix-services ┼──> phonix-core        (everything depends on it)
phonix-db ──────┤
phonix-config ──┘

phonix-core ──> nothing in this workspace
```

Because it is a leaf, it is also the crate to change when two layers disagree
about a rule: there is exactly one definition, and both sides compile against
it.

## Notable pieces

- **`PasswordPolicy`** — what an organization requires of a password.
  `system_default()` is the seed for a new workspace, `ABSOLUTE_MIN_LENGTH` is
  the floor nobody may configure below, and `check()` is what both the form and
  the server call.
- **`MfaPolicy`** / **`MfaEnforcement`** — `disabled`, `optional` or `required`,
  with a grace period so switching to `required` does not lock out everybody
  away from their phone.
- **`PermissionSet`** and the ABP-style tree in `authorization/definitions.rs` —
  `Pages.Administration.Users.Create` and friends, compiled in so the tree has
  one source of truth. Grants are rows; the *shape* is code.
- **`AuthUser::can`** — returns false until `is_fully_authenticated()`, so a
  session that has not cleared its second factor renders nothing it could not
  also do.
- **`Error`** — deliberately coarse. Anything crossing a server-function
  boundary is serialised to a browser, so it carries no SQL, no host name and no
  backtrace.

## Tests

79 unit tests, all of them pure. `cargo test -p phonix-core` needs no database,
no network and no configuration.
