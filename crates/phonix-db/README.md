# phonix-db — the data access layer

Repositories. A function here reads or writes rows and does nothing else.

![architecture](../../docs/architecture.svg)

## What lives here

| Module           | Owns                                                     |
| ---------------- | -------------------------------------------------------- |
| `connect`        | Pools and connections, catalog and tenant                |
| `tenancy/`       | The shared catalog, the pool registry, provisioning       |
| `identity/`      | `users`, `sessions`, `user_tokens`, `user_mfa_factors`, `password_history`, `identity_events` |
| `authorization/` | `roles`, `user_roles`, `role_permissions`, `user_permissions` |
| `settings`       | `workspace_settings`                                     |

## Database-per-tenant

Two kinds of database:

- **catalog** — one shared database holding the tenant registry. One long-lived
  pool, created at startup.
- **tenant** — one database per tenant. Pools are created lazily on first
  request and evicted when the tenant goes idle, so a thousand registered
  tenants do not mean a thousand open pools.

Everything outside `tenancy/` works against exactly one tenant's database and
carries **no tenant column**. Isolation is the database boundary, so a query
that reaches the wrong workspace is a routing bug, not a missing `WHERE` clause.

## The rule this layer keeps

> **No repository here ever receives a credential in a form it could use.**

| Arrives as        | Because                                              |
| ----------------- | ---------------------------------------------------- |
| PHC string        | `phonix-services` hashed the password with Argon2id   |
| 32-byte digest    | session and one-time tokens are SHA-256 before storage |
| sealed bytes      | TOTP secrets are XChaCha20-Poly1305 before storage    |
| 32-byte digest    | recovery codes are SHA-256 before storage             |

That is why this crate depends on no credential library at all — no `argon2`,
no `sha2`, no `chacha20poly1305`. If one appears in `Cargo.toml`, the boundary
has been crossed.

## What does *not* live here

Use cases. "Sign in" is not a query — it is a sequence of them with decisions in
between (is the account locked? does the workspace require a second factor?
should this hash be upgraded?). All of that is in `phonix-services`, along with
the hashing and the cipher.

## How it connects

```text
phonix-services ──> phonix-db ──> PostgreSQL
phonix-server ────> phonix-db      (health checks, tenant routing middleware)
phonix-web ───────> phonix-db      (AppState: Catalog + TenantRegistry)

phonix-db ──> phonix-core, phonix-config
```

## Queries, not macros

`sqlx::query*` at runtime rather than the compile-time `query!` macros. The
macros would need a reachable database — and one chosen tenant's schema — at
build time, which is the wrong trade for a multi-database application and for
CI. `FromRow` implementations are written by hand, which is also what keeps the
repeated column lists honest.

Migrations are embedded: `CATALOG_MIGRATIONS` and `TENANT_MIGRATIONS`.

## Tests

`cargo test -p phonix-db` runs the tests that need no database — deadline
arithmetic, identifier quoting, the column lists. Anything that needs Postgres
belongs in an integration test against a real one.
