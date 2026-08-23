# phonix-server — the composition root

The binary. It knows every layer and no layer knows it.

![architecture](../../docs/architecture.svg)

## What lives here

| Module       | Responsibility                                                |
| ------------ | ------------------------------------------------------------- |
| `main`       | Load configuration, install telemetry, run until a signal      |
| `startup`    | Build `AppState` — pools, catalog, registry, cache, broker, hasher, vault |
| `middleware` | Resolve the tenant from the `Host` header before anything else |
| `health`     | Liveness and readiness                                         |

## Tenant routing

Every request is resolved to a tenant by its host — `acme.localhost:3000` →
`acme` → `phonix_tenant_acme` — before it reaches a handler. The resolved
`TenantHandle` goes into request extensions, so nothing downstream has to parse
a host or pick a database.

`tenancy.auto_provision` creates a workspace on first sighting. It is for
development only, and production validation refuses to start with it enabled:
otherwise any unrecognised `Host` header can create a database.

## Wiring, and why it is all here

The layers below take their dependencies as parameters — a `&PgPool`, a
`&Security`, a `&Caller`. Nothing constructs a pool or reads a config file on
its own. This crate is where those are built, once, at startup:

```text
AppConfig ──> pools ──> Catalog + TenantRegistry ──┐
          └─> Hasher (Argon2 parameters)           ├──> AppState
          └─> SecretVault (MFA key)                ┘
```

That is what makes the lower layers testable without a process: they never reach
out for anything.

## How it connects

```text
phonix-server ──> phonix-web       (routes, SSR, the cookie helpers)
              ──> phonix-services  (use cases)
              ──> phonix-db        (pools, catalog, provisioning)
              ──> phonix-config, phonix-telemetry, phonix-cache, phonix-messaging
```

## Running it

```bash
cargo leptos watch          # development, with hot reload
cargo leptos build --release
```
