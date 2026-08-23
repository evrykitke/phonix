# Phonix

Leptos (SSR + hydration) on Axum, with a **PostgreSQL database per tenant**,
Redis for caching, RabbitMQ for messaging, and tenants resolved from the request
subdomain.

---

## Architecture

```
                    acme.phonix.local
                            |
                    [ tenant middleware ]        resolves subdomain -> slug
                            |
                    [ tenant registry ]          slug -> catalog row -> pool
                       /         \
        phonix_catalog             phonix_tenant_acme
        (shared registry)          (this tenant's data)

    Redis  phonix:acme:*            RabbitMQ  tenant.acme.<event>
```

Postgres runs **baremetal** on this machine (Windows service
`postgresql-x64-18`). Redis and RabbitMQ run in **Docker**.

### Crates

| Crate              | Responsibility                                                     |
| ------------------ | ------------------------------------------------------------------ |
| `phonix-core`      | Shared types (`TenantSlug`, `Error`). Compiles to wasm — no I/O.    |
| `phonix-config`    | Layered config loading and fail-fast validation.                    |
| `phonix-telemetry` | `tracing` setup: console + rolling file.                            |
| `phonix-db`        | Catalog, per-tenant pool registry, provisioning, migrations.        |
| `phonix-cache`     | Redis, namespaced per tenant.                                       |
| `phonix-messaging` | RabbitMQ topology, publisher, consumers.                            |
| `phonix-web`       | The Leptos app. Built twice: `ssr` and `hydrate`.                   |
| `phonix-server`    | The Axum binary: router, middleware, health, shutdown.              |

`phonix-web` is the **lib** package and `phonix-server` the **bin** package of
the cargo-leptos project — the split declared in `[[workspace.metadata.leptos]]`.

---

## Prerequisites

Already verified on this machine:

- Rust 1.96 + `wasm32-unknown-unknown` target
- `cargo-leptos` 0.3.7
- PostgreSQL 18 (baremetal, port 5432)
- Docker Desktop
- Node 24 (not required to build — Tailwind is fetched as a standalone binary)

---

## First-time setup

### 1. Secrets

```powershell
copy .env.example .env
```

Fill in `PHONIX__DATABASE__PASSWORD` and friends. `.env` is gitignored.

> The `@` in a Postgres password must be percent-encoded as `%40` **inside
> `DATABASE_URL`** (that variable is only for `sqlx-cli`/`psql`). The application
> itself never builds a URL — it passes host, user and password as separate
> fields — so `PHONIX__DATABASE__PASSWORD` takes the raw password.

### 2. Infrastructure

```powershell
docker compose up -d
docker compose ps
```

- RabbitMQ management UI: <http://localhost:15672>
- Prometheus metrics: <http://localhost:15692/metrics>

### 3. Catalog database

```powershell
$env:PGPASSWORD = "<your password>"
& "C:\Program Files\PostgreSQL\18\bin\createdb.exe" -h localhost -U smartenduser phonix_catalog
```

Its schema is applied automatically at startup while
`database.migrate_on_start = true`.

### 4. Tenant hostnames

Subdomain routing needs local DNS. Edit
`C:\Windows\System32\drivers\etc\hosts` **as Administrator**:

```
127.0.0.1  phonix.local
127.0.0.1  acme.phonix.local
127.0.0.1  globex.phonix.local
```

You can skip this at first: `config/development.toml` sets
`tenancy.default_tenant = "acme"`, so plain `http://localhost:3000` resolves to
the `acme` tenant. It also sets `auto_provision = true`, so that tenant's
database is created on the first request.

### 5. Run

```powershell
cargo leptos watch
```

<http://localhost:3000> — rebuilds Rust, wasm and CSS on change, with live reload.

---

## Configuration

Four layers, each overriding the last:

| # | Source                      | Committed | Purpose                          |
| - | --------------------------- | --------- | -------------------------------- |
| 1 | `config/base.toml`          | yes       | Every key, with defaults         |
| 2 | `config/{PHONIX_ENV}.toml`  | yes       | Per-environment                  |
| 3 | `config/local.toml`         | **no**    | Per-machine                      |
| 4 | `PHONIX__*` env vars        | **no**    | Secrets, deploy-time overrides   |

Environment variables nest with a **double** underscore:

```
PHONIX__DATABASE__PASSWORD        -> database.password
PHONIX__TELEMETRY__FILE__ROTATION -> telemetry.file.rotation
```

`PHONIX_ENV` (`development` | `production`) selects layer 2.

Validation runs at startup and refuses to boot on a bad combination — empty
production secrets, `min_connections > max_connections`, both log sinks
disabled, `auto_provision` or a `default_tenant` in production, and so on. The
error names the exact key.

---

## Logging

Two sinks, both on by default, one filter.

- **Console** — `pretty` + colour in development, `json` in production.
- **File** — `var/{env}.log`, JSON.

`telemetry.file.directory` is fully configurable: `{env}` expands to the
environment name, a **relative** path resolves against the workspace root (so
`cargo leptos watch` and a packaged binary agree), and an **absolute** path is
used as-is.

```toml
[telemetry.file]
directory = "var"          # -> <workspace>/var
file_name_prefix = "{env}" # -> var/development.log
rotation = "never"         # "daily" -> var/development.2026-08-17.log
```

`RUST_LOG` overrides `telemetry.level` + `telemetry.directives` entirely:

```powershell
$env:RUST_LOG = "phonix=trace,sqlx::query=debug"; cargo leptos watch
```

---

## Multi-tenancy

`acme.phonix.local` → slug `acme` → catalog lookup → pool to
`phonix_tenant_acme`.

- `TenantSlug` validates on construction (`[a-z0-9-]`, no leading digit or
  hyphen, ≤ 40 chars). Since the slug becomes part of a database name, this is a
  security boundary, not a formatting nicety — `provision.rs` re-checks the
  derived identifier before any DDL.
- Host headers outside `base_domain` never resolve to a tenant.
- Tenant pools are cached with **idle** eviction, so total Postgres connections
  stay bounded by `max_cached_pools * tenant_pool.max_connections`.
- Cache keys are `phonix:<slug>:<key>`; events route as `tenant.<slug>.<event>`.

### Adding a tenant

In development, just visit `http://<slug>.phonix.local:3000` with
`auto_provision = true`.

Otherwise insert a catalog row and call
`phonix_db::provision::provision_tenant`, which creates the database, runs
`migrations/tenant/`, and marks the tenant active.

---

## Migrations

| Directory              | Applied to                     |
| ---------------------- | ------------------------------ |
| `migrations/catalog/`  | `phonix_catalog` only          |
| `migrations/tenant/`   | **every** tenant database      |

Both are embedded at compile time via `sqlx::migrate!`, so the binary is
self-contained. Every tenant migration must be safe to run against an existing
tenant.

Queries use the runtime `sqlx::query*` functions rather than the compile-time
`query!` macros — the macros need a reachable database at build time, which does
not fit a many-database application or CI.

---

## Styling

Tailwind v4, CSS-first — there is no `tailwind.config.js`. Everything lives in
`style/main.css`, in two layers:

1. **Palette** — raw ramps (`--phonix-brand-500`). Never used by components.
2. **Semantic roles** — `--surface`, `--content`, `--edge`, `--brand`, exposed
   as utilities through `@theme inline`.

Components only ever write `bg-surface-raised`, `text-content-muted`,
`border-edge`. That indirection is what makes a re-theme a change to one file,
and it is why light/dark works without a single `dark:` variant in any view.

Dark mode follows `prefers-color-scheme`, and `[data-theme="dark"|"light"]` on
`<html>` forces either one for a future in-app toggle.

> The current values are neutral placeholders — swap the palette block when the
> real theme is decided.

---

## Health

| Endpoint        | Checks                          | Use                     |
| --------------- | ------------------------------- | ----------------------- |
| `/health/live`  | nothing                         | liveness / restart probe |
| `/health/ready` | Postgres, Redis, RabbitMQ       | readiness / load balancer |

Deliberately separate: a Redis outage must not make an orchestrator kill an
otherwise healthy process.

---

## Commands

```powershell
cargo leptos watch                 # dev server, live reload
cargo leptos build --release       # production build
cargo leptos serve --release       # build + run

cargo test --workspace             # tests
cargo clippy --workspace --all-targets
cargo fmt --all

docker compose up -d
docker compose logs -f rabbitmq
docker compose down                # keep data
docker compose down -v             # wipe data
```

---

## Notes

- `docker-compose.yml` publishes to `127.0.0.1` only — nothing is reachable from
  the LAN.
- RabbitMQ topology is declared by the application at startup, not by a broker
  definitions file, so it stays in version control.
- Consumers get at-least-once delivery. Handlers must be idempotent; each tenant
  database has a `processed_events` table for that.
- `outbox_events` exists in the tenant schema for the transactional outbox
  pattern — write the event in the same transaction as the state change, then
  relay it. The relay itself is not implemented yet.
