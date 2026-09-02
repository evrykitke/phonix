# ADR 0005 — Phonix Desk: a second application over the catalog

Status: accepted, not yet built
Date: 2026-09-02

Phonix is a database-per-tenant application with a catalog that nothing can
read. Every administration screen built so far — including the thirty-nine
`/api/v1` operations finished on 2026-09-01 — lives *inside* one workspace,
reached at that workspace's own host and gated by that workspace's own
permissions. There is no surface anywhere that answers "how many workspaces are
there", "which of them is wedged", or "did that deploy actually migrate
everybody".

This record specifies the thing that answers those: a second application,
`phonix-desk`, served from its own process, over the catalog rather than over a
tenant. It is where the platform is run from — workspaces are created there,
licensed there, and stopped there.

It also draws a line it will not cross. Managing the *machine* — deploys,
systemd, nginx, certificates, backups — is a different problem with a different
security model, and section 12 says why it is not in here.

---

## 1. The mechanism exists; the surface does not

This is not a feature request. It is a set of callers missing from code that is
already written, tested and correct.

| Already built | Callers today |
| --- | --- |
| `Catalog::list()` | the startup migration sweep, and nothing else |
| `Catalog::set_status(slug, status)` | **none, anywhere in the workspace** |
| `TenantStatus::{Suspended, Archived}` | never written by any code path |
| `TenantStatus::serves_traffic()` | `find_active`, on every request |
| `middleware.rs:57` — an inactive tenant answers **403** | reached only in tests |
| `migrate_outdated_tenants` / `MigrationSweep` | boot, once, with the count going to a log line |
| `provision_tenant` | self-service signup |
| `drop_tenant_database` | tests |

Suspension in particular is **finished end to end**: a suspended workspace stops
serving, and it answers 403 where an unknown host answers 404, so a suspended
customer is distinguishable from a DNS mistake without anyone having to guess.
Every part of that works. The only thing missing is any way at all to set the
status, short of opening `psql` against the catalog and writing the `UPDATE` by
hand — which is also, today, the only way to discover that a workspace is stuck
in `provisioning` after a crash between onboarding steps 5 and 6.

So the question this record answers is not "what should Desk do". It is
"where does the caller live, and what is allowed to reach it".

## 2. It is a second binary, and not a second repository

`crates/phonix-desk`, a new workspace member — `members = ["crates/*"]`
already admits it — producing its own binary, its own systemd unit, its own
listener. It compiles none of the tenant application.

That gets every bit of isolation worth having. A process that cannot serve a
tenant request cannot be tricked into serving one. It reaches the world through
its own hostname and its own nginx block (section 5), is hardened separately,
and is restarted without touching the product. If the tenant application is
wedged, Desk is unaffected — which matters, because "the product is
misbehaving" is the moment somebody wants it.

**A separate repository was considered and rejected.** It buys three things:
independent release cadence, an independent dependency graph, and source you can
hand to someone who may not see the product. None applies — same team, same box,
same release — and the first is actively harmful:

1. **The migrations are the deciding argument.** Desk's central job is
   "which tenants are on which schema version, and move the ones that are
   behind". The migration set is embedded at build time by
   `phonix-db/src/lib.rs:111` and the four `sqlx::migrate!` statements beside
   it, and the expected version is `apps::schema_fingerprint()` — computed from
   the embedded migrators, not written down. A separate project either
   duplicates that set, or reads it over a wire. Duplicated, it drifts, and
   [drift here is already a known cost](0001-core-boundary.md) — a checksum
   failure from nothing worse than line endings has bitten this project once
   already.

2. **It needs the service layer, not the database.** Retrying a stuck workspace
   is `workspace::onboarding`, not an `UPDATE`: six ordered steps with a written
   comment explaining why they cannot be one transaction, because `CREATE
   DATABASE` is not transactional. Reimplementing that across a repository
   boundary means it is wrong the first time the original changes, and wrong
   silently.

3. **It must move in step with the schema.** Two repositories means two versions
   that can disagree about what "current" means — which is precisely the
   disagreement Desk exists to detect.

The isolation people reach for a separate repository to get is *process and
network* isolation. A second binary in the same workspace already has it.

## 3. It does not compile the tenant application

`phonix-desk` depends on `phonix-config`, `phonix-core`, `phonix-db`,
`phonix-services` and `phonix-telemetry`. It does **not** depend on
`phonix-web`.

That crate is the tenant application and it compiles to WebAssembly; taking it
as a dependency would drag the whole product — and its wasm build — into a tool
that should be able to run when the product cannot. The cost is that Desk
does not get the DataGrid: `phonix-web/src/ui` is 58 files and roughly 20,500
lines, all of it inside the app crate.

Two ways out, and this record takes the second.

**Rejected: extract `phonix-web/src/ui` into a shared `phonix-ui` crate.**
Honest, and the right move if a third Leptos application ever appears. It is
also a large refactor of the most load-bearing code in the product, undertaken
to serve a tool whose whole value is being simpler than the thing it watches.

**Chosen: server-rendered HTML, no wasm.** This is not a new idea here — it is
what `phonix-profiler` already does, and it depends on no internal crate at all.
[ADR 0004 §6](0004-development-profiler.md) wrote down the reason and it applies
with more force to this: a wasm panic freezes every handler on the page at once,
and that is exactly the moment the tool is wanted. Building Desk on the same
hydration stack as the product means one bug in the shared UI kit takes out the
instrument you would have used to find it.

**It shares the theme, and only the theme.** Not compiling the product does
not mean looking nothing like it. `style/theme.css` — the palette, the accent
ramps, the semantic roles in light and dark, the compact type scale — is
imported by both entry points, so a Desk page uses `bg-surface-shell` and
`text-content-muted` and means exactly what the product means by them. That
file is a split of `style/main.css` and changed nothing: the product's compiled
stylesheet is byte-identical across the split.

This is not the shared UI crate rejected above, and the difference is the whole
point. That was 58 files of Leptos components with hydration in them. This is
one CSS import with no code in it at all, and it cannot drag wasm into Desk
because there is none in it to drag. What Desk gets is the *look*; what it
still does not get is the DataGrid, the command palette, the collapsing
sidebar, or anything else that needs a script — its sidebar becomes a row of
links below `md` rather than a drawer, because that is what responsive without
JavaScript actually looks like.

Tailwind compiles it. `phonix-desk` is a plain `cargo build` with no
cargo-leptos in front of it, so the stylesheet is built by hand
(`node tools/build-desk-css.mjs`) and **the output is committed** — the same
arrangement the editor bundle already uses, and for the stronger version of the
same reason: a second binary whose deployment needs npm on the box is a second
way for a release to fail at the moment the tool is wanted. The compiled CSS is
`include_str!`d, so Desk stays one artefact.

**The markup is in `templates/`, not in `format!` strings.** Askama compiles
those HTML files into the binary at build time, so nothing is read from disk at
request time and there is still one thing to deploy. The reason to prefer it is
not tidiness: **escaping is the default**. The first version applied an `esc()`
helper at every interpolation and admitted in its own doc comment that "this one
is safe" is a judgement that has to be right every time or not made at all.
Pages compose by `{% extends %}` rather than by passing a rendered body string,
because a body string would need `{{ body|safe }}` in the frame and would have
put the same hole back one level up. No template of Desk's writes `|safe`.

**The rule is ADR 0004's replacement rule, unchanged: every page must be
complete without the script.** Tabs are sections otherwise stacked in order; a
detail link is an ordinary `<a href>` to a page that exists; collapsing is
`<details>`. A script compiled into this binary and served from it may enhance
what is already there. Anything that cannot degrade that way belongs on the
server.

**On the name.** It is **Phonix Desk**, and the two obvious alternatives were
already spent. "Host" is what this started as, and this codebase gives that word
to the HTTP `Host` header — `[tenancy]` resolves a tenant from it,
`reserved_subdomains` lists the hosts that are not tenants, `tenant_origin()`
builds one. "Console" is taken too, quietly: `[telemetry.console]` is the
terminal logger, so a top-level `[console]` beside it would mean something else
entirely in the same file.

A desk is where somebody sits to answer for the whole estate, which is what this
is, and the word appears nowhere in the codebase as an identifier — only in
prose, in the MFA warning about somebody away from their desk. So it is `desk`
throughout: `crates/phonix-desk`, the `[desk]` config section,
`catalog.desk_users`, `phonix-desk.service`. The person who signs in is a **desk
user**, mirroring `core.users` inside a tenant. The Cargo manifest already
states the principle behind that for the `app-*` prefix — two words for one idea
eventually disagree — so "operator" is not also used.

## 4. A desk user is not a `Caller`

`Caller` is tenant-scoped by construction. It carries a user, that user's grants
inside one workspace, and `Caller::require` is the gate every use case in
`phonix-services` is written against. A desk user has no workspace, and the
catalog has no permissions.

The tempting shortcut — mint a synthetic tenant, give it a "desk" role,
reuse everything — is the one thing this record forbids outright. It would make
every existing permission check answerable by a row a desk user can write, which
does not weaken the model so much as end it.

So Desk has its own identity, in the catalog, in tables that do not exist
yet. The catalog currently has exactly one table: `tenants`. Migration 0002 only
added an owner email column to it.

```
catalog.desk_users      id, email, display_name, password_hash,
                        totp_secret, status, created_at, disabled_at
catalog.desk_sessions   token_hash, desk_user_id, issued_at, last_seen_at,
                        expires_at, ip, user_agent
catalog.desk_audit      id, desk_user_id, action, tenant_slug, before, after,
                        occurred_at, ip
```

Four decisions inside that:

* **MFA is not optional.** `security.mfa` on a workspace is a policy an
  administrator chooses. Here there is nothing to choose: an account that can
  suspend every workspace on the box gets TOTP or it does not get in.
  `phonix_services::crypto::totp` already exists and is reused as-is.

* **Sessions are rows, hashed, exactly like `core.sessions`.**
  `crypto::token` already stores a SHA-256 of a 43-character URL-safe token and
  nothing else; Desk stores its sessions the same way, in the catalog. No
  JWT, for the reasons [ADR 0003](0003-mobile-authentication.md) already
  settled, and no Redis — Desk must work when Redis does not.

* **There are no desk roles in v1, and that is a decision with a
  trigger.** The operating team is small and internal, and every one of them is
  trusted with the same things; inventing a permission tree for three people
  produces a vocabulary nobody maintains. What forces the reversal is stated in
  advance so it is not argued later: **the first person who should be able to
  read Desk but not act with it.** Support staff are the likely first
  case. Until then, a desk account is one thing and the audit trail is what
  distinguishes people.

* **Accounts are created by a desk user, never self-service.** There is no
  signup, no password reset by email, and no invitation flow — Desk does
  not send mail at all (SMTP is per-workspace and, on this box, still disabled).
  A new one is created by an existing one and collects a first password the
  same way [an invited user does](0002-public-api.md): a single-use link handed
  over out of band. Bootstrapping the *first* desk user is a CLI subcommand of
  the binary, run on the box.

## 5. Where it is served, and how it is reached

**In a browser, at `console-desk.<base_domain>`** — `console-desk.evrykit.com`
on the box as it stands. Not an SSH tunnel: this has to be openable from
wherever the person is when a workspace is wedged, which is rarely at a
terminal.

**The process still binds `127.0.0.1:3100`, and nginx proxies to it.** A public
bind is not the same decision as a public hostname: a socket on `0.0.0.0`
answers whatever `Host` header it is sent and can be reached by address and
port directly, which skips `server_name` matching altogether. Loopback plus an
explicit nginx server block means there is exactly one way in.

That block has to exist. On this box Phonix is the nginx catch-all for every
unmatched `evrykit.com` subdomain, so a host with no block of its own does not
fail — it quietly lands on the tenant application instead.

**`console-desk` goes into `reserved_subdomains` before the DNS record does.**
The label matches both `tenancy.slug_pattern` and the `tenants_slug_format`
check constraint, so until it is reserved a workspace can be created that
claims it. This is the standing cost of borrowing the apex namespace, and it is
paid in the right order or not at all.

**The certificate is already there, and this is the one place the wildcard
problem does not bite.** `console-desk.evrykit.com` is *one* label deep, so the
existing `*.evrykit.com` origin certificate covers it — unlike tenant
subdomains, which sit two labels deep under `phonix.evrykit.com` and are the
reason that whole question is still open. Desk needs no new certificate and
should not wait on one.

**Its own rate limiter, in its own process, and one setting that decides whether
it works at all.** Credential-presenting endpoints get the `action` tier's shape
— twelve a minute, which is more than anybody types.

The setting is **not a new one**. `[security.rate_limit] client_ip_header`
already exists and `config/production.toml` already sets it to `x-real-ip`,
which nginx fills from `$remote_addr`; Desk reads the same value, because the
two processes sit behind the same proxy and a second copy of that answer is a
second thing to get wrong. What Desk adds is a refusal: it may not be empty
under production. The default keys on the peer address, and behind a proxy the
peer is always nginx, so every visitor in the world shares one bucket and the
limiter becomes a global counter that locks everybody out at once instead of an
attacker. The comment in `[security.rate_limit]` already says getting this wrong
makes the limiter decorative — this is where that stops being advice.

**What stands in for the source restriction**, now that the front door is
public: TOTP that cannot be turned off (section 4), lockout on repeated
failures, sessions deliberately shorter than a workspace's, a host-only session
cookie that is `Secure`, `HttpOnly` and `SameSite=Strict` — Desk has no
cross-site navigation to preserve, so the strict setting costs nothing — HSTS,
and **every sign-in written to `catalog.desk_audit`, the failures included**. An
nginx address allowlist stays available for anyone who wants it and is off by
default; it is a good control and a bad requirement, because the person who
needs Desk at 2am is not on the office address.

**It is never a route in `phonix-server`, and never on the tenant wildcard.**
The hostname is served by a different process on a different port; the only
thing the two share is the configuration file and the crates underneath them.

**One naming note, so nobody tidies it.** The identifier is `desk` and the DNS
label is `console-desk`. They are different on purpose: one is what the source
calls the thing, the other is what somebody types into a browser a year from now
when they cannot remember what it was called.

## 6. What it may do, and the two things it may not

In order of how much damage each can do:

| Action | Reaches | Reversible |
| --- | --- | --- |
| List workspaces, with status and schema version | catalog | — |
| **Create a workspace** | `workspace::onboarding` | no — a database is made |
| **Issue, extend or withdraw a licence** | catalog | yes |
| One workspace: its record, owner email, install set, size | catalog + tenant | — |
| Dependency health: catalog, Redis, RabbitMQ | the three checks `/health/ready` already runs | — |
| Retry a stuck `provisioning` | `workspace::onboarding` | idempotent by design |
| Migrate one workspace, or every outdated one | `migrate_tenant` / `migrate_outdated_tenants` | forward-only |
| Suspend, and resume | `Catalog::set_status` | yes |
| Archive | `Catalog::set_status` | yes — the row and database remain |
| Create and disable desk accounts | catalog | yes |

Creating a workspace from Desk is the same six-step sequence self-service signup
runs, with the same refusals: a slug that is reserved or already taken is
refused by `slug_is_available` and `reserved_subdomains`, not by a second copy
of those rules living here. What Desk does **not** do is set the owner's
password — it issues the same single-use invitation link an invited user gets,
because that rule does not bend for the person who created the workspace.

A workspace is never live without a licence: one is issued at creation, and
self-service signup issues a trial. Section 7 is what a licence is.

Everything in that table goes through `phonix-db` or `phonix-services`. Desk
writes no SQL of its own against a tenant database — if an action needs
a statement that does not exist yet, it is added to the repository layer where
the tenant application can be held to it too.

**It may not read a workspace's business data.** Not invoices, not parties, not
files, not a user's name. Desk answers questions about a workspace as an
*object* — is it running, is it current, is it wedged — and every question about
what is *inside* one is answered by signing in to it as somebody with the right
to see it. This is the line that keeps Desk auditable: a surface that can
read tenant data is a surface that has to justify every read, and no small team
sustains that.

**It may not become one of them.** No impersonation, no "sign in as this user",
no setting somebody's password — the same refusal `/api/v1` already makes, and
for the same reason: an account gets its password once, from the person who will
use it.

**`drop_tenant_database` is not exposed in v1.** It exists in `phonix-db` and it
is used by tests. Archiving already stops the workspace serving traffic, and it
is reversible; deletion is not, and the interval between "we should delete that"
and "we needed that" is measured in months. When it is built it wants a typed
confirmation of the slug, an archived-for-N-days precondition, and a line in the
audit that cannot be removed by the person who wrote it.

## 7. A licence is permission to use the platform

Not a feature switch, not a plan, not a bundle of entitlements: a licence
answers one question, **is this workspace authorized to use Phonix**, and it is
the commercial half of that answer rather than the operational one. This
section specifies the smallest honest version of it, because Desk cannot create
workspaces without deciding what a created workspace is allowed to do.

**Built**, as catalog migration 0005, `phonix_core::tenant::licence` and
`phonix_db::tenancy::licence`. Three things settled during the build that the
draft above did not say:

* **One row per workspace, and the primary key says so.** The history of what a
  workspace has been licensed under is `desk_audit`, in a database it cannot
  edit — not a table of superseded rows. Two places holding licence history is
  two answers to "is this authorized", and they eventually disagree.
* **The licence is joined onto every read of `catalog.tenants`,** not fetched
  separately. `serves_traffic` needs both halves to answer at all, and the
  registry resolves a catalog row on essentially every request; a second lookup
  would be a second thing that can be stale.
* **A lapse and a suspension are different errors, not one.**
  `DbError::TenantUnlicensed` carries the standing and the sentence to refuse
  with; `DbError::TenantInactive` stays what it was. Both are 403 and both are
  still distinguishable from the 404 an unknown host gets — the difference
  between them is for the log, the trail, and the sentence the customer reads.

**It lives in the catalog, and that is the whole point.** A licence a tenant's
own administrators can reach is not a licence. There is already a cautionary
example in the codebase: `workspace_settings.api_enabled` sits in the *tenant's*
database and `set_api_enabled` requires `Settings`, which a workspace
administrator holds — so ADR 0002 §4's claim that "an administrator can grant
themselves a permission; they cannot sell themselves a feature" is not true as
built. That is a separate defect about a feature switch, not about this, and it
is named here only because it is the exact mistake this section must not repeat.

```
catalog.tenant_licences   tenant_id, state, valid_from, valid_until,
                          note, updated_at, updated_by
```

`state` is one of **`trial`**, **`licensed`** or **`revoked`**, and a licence is
*current* when the state is not `revoked` and now falls within `valid_from` to
`valid_until` — a null `valid_until` meaning it has not ended. Three states
rather than dates alone, because "it ran out" and "we withdrew it" are the same
date arithmetic and completely different events: the first is answered by
extending, the second by a conversation. `trial` is separated from `licensed`
for the same reason — an expiring trial is the expected case and an expiring
paid licence is somebody's problem to chase.

Five decisions, and everything else deferred:

1. **A lapse is not a suspension, and must not be written as one.** Suspension
   is somebody's decision, recorded with their name against it; a lapse is a
   date passing. If a job flipped `status` to `suspended` on expiry, reinstating
   a workspace would mean guessing what its status had been before — and a
   workspace that was *deliberately* suspended and then lapsed would come back
   on when payment cleared. The two are stored separately and always were two
   different facts.

2. **Enforcement goes through the mechanism that already exists, in the one
   place that already decides.** `TenantStatus::serves_traffic()` is `status ==
   Active` today; it becomes "status is `Active` **and** the licence is
   current". Nothing else changes: `find_active` already refuses with
   `TenantInactive`, `middleware.rs:57` already answers **403**, and the catalog
   row is already resolved on every request, so there is no second lookup and
   nothing to keep in step. The refusal carries a different reason for a lapse
   than for a suspension, because "your licence ended" and "we stopped you" are
   different sentences to receive.

3. **No implicit grace period.** Extending a licence is changing one date, and
   one date is a thing a person can read off a screen and reason about. A grace
   window is a second number that quietly contradicts the first, and the moment
   it exists nobody can say what `valid_until` actually means. If a customer
   needs another week, they are given another week.

4. **A trial is a licence with an end date**, not a separate concept and not a
   status. Self-service signup issues one; its length is configuration. This
   costs nothing and it means the expiry path is exercised constantly rather
   than for the first time on a real customer.

5. **`valid_until` may be null**, meaning no end — which is what an internal
   workspace, a demonstration tenant and this box's own `med-app-staging` all
   are. A licence with no end is a deliberate act by a named desk user, and the
   audit row saying so is the point.

**What is deliberately not here, and wants its own record:** plans and editions,
seat limits, per-app entitlement, anything priced, and billing of any kind.
Desk records *that* a workspace is authorized and until when. Where that
authorization came from commercially — an invoice, a contract, a card that
cleared — is a system this one does not have and should not grow by accident.

The one thing worth saying in advance about that record: when entitlement does
arrive, it belongs beside this row and not in the tenant, and the effective
answer for any feature will be **the catalog's entitlement AND the tenant's own
switch** — the narrower of two things, neither able to widen the other. That is
the same shape as an API key's power being its owner's grants intersected with
its own scopes, and it is the shape `api_enabled` should have had.

## 8. The audit trail lives in the catalog

`core.entity_events` and `core.identity_events` are per tenant, and per tenant
is the wrong place for this. "Who suspended this workspace" must not be a row
that the workspace's own administrators can read, edit, or lose when the
database is archived. It goes in
`catalog.desk_audit`, and Desk's own read of that table is the only
screen in this product that shows it.

The shape follows the rule the entity trail already established: a change is
recorded **from → to**, never as narration, because that shape is what earns a
diff on the detail page. An action with no before-state — a migration, a retry —
records what it swept and what the result was.

Every action in section 6's table writes one row. There is no "read audit"; a
list of workspaces is not an event.

## 9. Configuration

An `[desk]` section in `config/base.toml`, following `[profiler]` as the
precedent for a tool that is part of the deployment but not part of the
application:

```toml
[desk]
# Loopback, with nginx in front on console-desk.<base_domain> - ADR 0005 s5.
# A public bind answers any Host header and can be reached by address, which
# skips server_name matching entirely.
listen = "127.0.0.1:3100"
# Say so deliberately to bind anywhere else under production. Refused otherwise.
allow_public = false
# Idle and absolute lifetimes for a desk session. Deliberately shorter
# than a workspace session: this one suspends workspaces.
session_idle_minutes = 30
session_absolute_hours = 8
# How long a single-use setup link is good for.
setup_link_hours = 48
# How long a self-service signup is licensed for. A trial is a licence with an
# end date - see section 7 - so this is the only number that says what one is.
trial_days = 30
```

Desk reads the same configuration file as the server, and therefore the same
`[database]`, `[telemetry]` and `[tenancy]` blocks. It must not grow its own
copy of the catalog connection string; two files that describe one database is
how a tool ends up operating on something other than production while saying it
is production.

The same argument governs what is **absent** from `[desk]`: hashing parameters,
TOTP parameters, the vault key that seals a secret, the lockout thresholds and
the proxy header all stay in `[security.*]` and are read from there. They are
facts about this deployment, not about this application.

`validate::check` gains two rules, both of them refusing at boot what would
otherwise be discovered by strangers — the same shape as the profiler's refusal
to run under production. Desk's `listen` may not be a non-loopback address under
production unless `desk.allow_public = true` is also set; and `client_ip_header`
may not be empty under production, because a rate limiter that counts every
visitor as nginx is worse than none — it reports a number that is always the
same and locks out everybody at once.

## 10. Where it sits in the stack

```
                              internet
                                 │
                          ┌──────┴──────┐
                          │    nginx    │
                          └──┬───────┬──┘
            tenant hosts ────┘       └──── console-desk.<base_domain>
                  │                               │
                  ▼                               ▼
      ┌───────────────────────┐        ┌──────────────────────┐
      │ phonix-server         │        │ phonix-desk          │
      │ 127.0.0.1:3000        │        │ 127.0.0.1:3100       │
      │ Leptos SSR + wasm     │        │ server-rendered HTML │
      └───────────┬───────────┘        └──────────┬───────────┘
                  │                               │
                  ▼                               ▼
      ┌───────────────────────────────────────────────────────┐
      │ phonix-services      use cases, Caller-gated          │
      │ phonix-db            catalog, registry, provisioning  │
      │ phonix-core          vocabulary, TenantStatus         │
      └───────────┬───────────────────────────────┬───────────┘
                  ▼                               ▼
         phonix_tenant_*                    phonix_catalog
```

Two shared layers, two applications, one direction of dependency. Neither binary
depends on the other, and neither can be built from the other's crates.

## 11. Build order

Each step ends somewhere the thing is usable and honest about what it does not
do yet.

1. **The crate, the config, and the identity.** `crates/phonix-desk`,
   `[desk]`, catalog migration 0004 for the three tables, the bootstrap
   subcommand, sign-in with mandatory TOTP, sessions, sign-out. Nothing else is
   reachable. This is first because a surface over the whole catalog cannot ship
   an unauthenticated read "temporarily".
2. **The audit table and its screen**, before the first action that would write
   to it. Building the trail after the actions means the first weeks are
   unrecorded, and those are the weeks with the most mistakes in them.
3. **Read.** The workspace list — slug, status, licence, schema version against
   `schema_fingerprint()`, created, owner email — the detail page, and the
   dependency health panel.
4. **Licences.** Catalog migration 0005 for `tenant_licences`, the screen that
   issues and extends one, and `serves_traffic()` learning about them. This
   comes before creating workspaces, because a workspace created with nowhere
   to record its authorization is a workspace somebody has to remember to go
   back to. **Built**, together with step 3's workspace page — the licence form
   needs a page to live on, and the page needs the licence to be worth opening.
5. **Creating a workspace**, licence and owner invitation together, in one act.
6. **The three safe writes.** Retry a stuck provisioning; migrate one workspace
   and migrate all outdated; suspend and resume. Each behind a confirm, each
   writing an audit row. **Built.** Two things the build settled:

   * **A confirm is a page, not a dialog**, because there is no script to open
     one — and it turns out to be the better of the two. There is room to say
     what will happen and what it will not touch, the back button means "no",
     and the address bar names the workspace about to be acted on. Every action
     is a link to that page and a `POST` from it; nothing acts on a `GET`.
   * **`mark_active` was wrong for a migration, and would have been a bug
     nobody connected to one.** It writes `status = 'active'` as well as the
     schema version, which is right at the end of provisioning — that write
     *is* the commit point of creating a workspace. The boot sweep used it too,
     so the first deploy after the first suspension would have silently resumed
     every suspended workspace. `Catalog::mark_migrated` writes only the
     version. It had never fired because until now nothing anywhere wrote
     `Suspended`, which is section 1's first row.
7. **Archive**, once suspend has been used in anger and the difference between
   the two is felt rather than argued.
8. **Jobs and the outbox.** The four loops in `jobs.rs` — verifier, sweeper,
   prune, relay — run in-process with no screen over any of them, and
   [ADR 0004 already concluded](0004-development-profiler.md) that operational
   visibility of background work is this tool's job and not the profiler's.

Deferred with intent, and each one wants its own record when it comes: the
licensing framework proper — plans, seats, entitlement, anything priced —
deleting a tenant database, desk roles, an nginx address allowlist, and
anything that touches the machine.

## 12. What was considered and not chosen

**A route area inside `phonix-server`.** Cheapest by far — the shell, the UI kit
and the session machinery are all there. Rejected because it puts the catalog's
write path in the same process as the internet-facing multi-tenant application,
one routing mistake away from a tenant host, and because it is unavailable
precisely when the server is the thing that is broken.

**A separate repository.** Section 2.

**Reaching it over an SSH tunnel, with no hostname at all.** The first draft of
this record chose exactly that, on the reasoning that everybody who needs Desk
already has the key. Rejected the same day it was written: the moment Desk is
wanted is a workspace wedged at an awkward hour, and a tool that requires a
terminal gets opened later than it should be. The tunnel stays available as the
fallback — the socket is loopback either way, so withdrawing the hostname means
deleting an nginx block and nothing else.

**A Desk built in Leptos, sharing an extracted `phonix-ui`.** Section 3.

**A generic tool — pgAdmin, a hosting panel, a database GUI.** They operate on
tables. Every action in section 6 is a *use case* with ordering constraints that
a table editor cannot express: setting `status = 'active'` on a row whose
database was never created produces a workspace that routes traffic into
nothing. The catalog is not a place to hand somebody a SQL prompt; it is exactly
the place where the six-step provisioning comment matters.

**A web UI over systemd, nginx and deploys.** Refused, and this is the line
section 12 exists to draw. A page that restarts a service or runs a deploy is a
remote shell with a login form in front of it, on a box that also carries three
sites that are not ours to break. The machine-management story is a **committed,
versioned `phonix-deploy`** — today it is gitignored and hand-edited, and the
copy on the VPS has already drifted from the laptop's over `--bin-features
ssr` — plus, at most, a read-only status page. Execution stays on SSH, where it
is already authenticated, already audited, and already limited to people who
have the key.

## Consequences

* One more workspace member, one more systemd unit, one more thing to deploy —
  and it must be deployed **in the same step** as the server, because it shares
  the embedded migration set and the two disagreeing about "current" is the
  failure it exists to catch.
* `phonix-deploy` grows a second binary and a second unit restart. It has to be
  committed to the repository before that is true, or the drift already
  observed once becomes drift in the thing that reports drift.
* The catalog gains its first tables since onboarding, and its first
  credentials. A compromise of Desk is a compromise of every workspace on the
  box, and it is now reachable from any browser. Mandatory TOTP, a limiter
  keyed on a real client address, sessions measured in hours, and a trail in a
  database no tenant can write are what stand in front of that. None may be
  softened for convenience without amending this record.
* Server-rendering Desk means it will look less finished than the product's
  screens. That is the trade taken deliberately: this tool has to work
  on the day the product does not.
* `reserved_subdomains` grows `console-desk`, and it has to grow it before the
  DNS record exists rather than after.
* **`TenantStatus::serves_traffic()` stops being a pure function of status.**
  It is called on every request through `find_active`, so this is the highest
  traffic change in the record — and it is one `&&` in one place, against a row
  that is already loaded. Every existing test that asserts a workspace serves
  traffic needs a current licence to keep passing, which is the right kind of
  breakage: it is the compiler asking who authorized this workspace.
* **Every existing workspace needs a licence row** in that migration, or the
  first deploy after it stops serving all of them at once. The backfill issues
  one with no end date and a note saying it was created by the migration, which
  is honest — nobody licensed those workspaces, they predate the idea.
* `workspace_settings.api_enabled` is left where it is and keeps working. It is
  a feature switch a workspace administrator can flip, ADR 0002 §4 describes it
  as something they cannot, and that contradiction is now written down as its
  own problem rather than quietly fixed inside this one.
* `TenantStatus::Suspended` and `Archived` become reachable for the first time.
  The 403 they produce is already implemented and already distinguishable from a
  404 — the first suspension will be the first time anyone sees it outside a
  test.
