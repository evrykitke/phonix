# ADR 0004 — The development profiler: what it measures, and why it is not a Leptos component

Status: accepted; phases 1 and 2 built 2026-08-31
Date: 2026-08-31

Symfony ships a profiler: a toolbar pinned to the bottom of every page in
development, and a full report behind it holding the request, the route, the
queries, the timings and the memory the page cost. It is the single biggest
reason Symfony feels fast to work in, and it is worth having here.

This record is about porting the *idea*, because the mechanism does not port.
Symfony's profiler is built on two facts that are false for this application:
one request is one page, and one request is one process. Sections 2 and 3 are
what happens when those two assumptions are removed.

---

## 1. This is not "read the log file"

Everything the profiler shows is already being emitted. `TraceLayer` spans every
request, `make_request_span` records the method, path and tenant, and sqlx emits
an event per query with the statement and its elapsed time. All of it goes to
`telemetry.file.directory` as JSON.

That is a different tool. A log is a stream to search *after* you have a
question; a profiler is an answer standing next to the thing that raised it.
The gap it closes is the moment a screen is slow or wrong and the developer has
not yet formed the question — which is most moments, and all of the ones where
somebody is new to the codebase.

So: no new instrumentation. The profiler is a *reader* of the tracing registry
that already exists (section 4). If it ever needs a `tracing::info!` added
somewhere to work, that is a smell — the log line should have been there anyway.

## 2. The unit is a page load, not a request

Symfony's `RequestStack` is a master request plus its sub-requests, and the
whole model rests on the browser making one HTTP request per page. Here, one
page load is:

| | |
|---|---|
| First visit | 1 document request (SSR) + the wasm bundle + N server-fn `POST`s as resources resolve |
| In-app navigation | **0** document requests, and N server-fn `POST`s |
| A form submit | 1 server-fn `POST`, then usually a refetch of everything on screen |

A profile per request therefore produces a list nobody can read: forty entries,
thirty-nine of them a server function, and no way to see which screen asked for
them. Worse, the case a developer most wants to inspect — clicking through the
app after it has hydrated — never produces a document request at all, so the
Symfony model would show nothing for it.

**So the profiler groups by page load.** A `page_id` is minted in the browser on
every navigation and sent as `X-Phonix-Page` on every server call. The report
for a page is the group, and *that* is this codebase's answer to `RequestStack`:
not master-and-sub, but "here are the seven server calls this screen made, in
order, with what each one cost".

This is strictly more useful than the thing it replaces. The commonest real
performance bug in an application shaped like this one is a screen that makes
eleven server calls where it could make two, and that bug is invisible in any
per-request view.

### The document request is a member of the group, not the parent

An SSR document and a later server-fn call are the same kind of thing: an axum
request through the same stack. The document is distinguished by *being* a
navigation, not by owning the others. On an in-app navigation the group simply
has no document member, and the report says so rather than looking broken.

## 3. What is measured, and the one thing that cannot be

### Route — and a defect it exposes

`axum::extract::MatchedPath` carries the *pattern* a request matched
(`/admin/users/{id}`), which is what "current route" means to a developer. The
profile records it, and falls back to the concrete path when nothing matched.

One trap, recorded because it will otherwise be rediscovered: `MatchedPath` is
inserted into the request *during* routing, so a middleware attached with
`Router::layer` — which wraps routing — cannot see it. `route_layer` runs after
routing and can.

That looked like the end of it, and it is not. The profiler also needs the
tenant, and `resolve_tenant` is a `Router::layer` that has *finished* before
routing begins — so a collector opened at `route_layer` misses both the
`Span::record("tenant", ..)` and the `SELECT … FROM tenants` that produced it.
Phase 1 shipped attached only at `route_layer`, and every profile reported no
tenant on requests that had resolved one perfectly well. It was invisible on
the bare host, where the honest answer is also "none".

The requirement does not fit in one middleware, so it is two: an outer one that
opens the collector, times the request and files the profile, and an inner
`route_layer` one that does nothing but write `MatchedPath` into it. They share
a task, which is what lets the inner one reach a task-local the outer one
opened. A consequence worth having: the outer layer wraps routing, so a request
matching no route *is* profiled now, with no route pattern rather than no row.

This also settles a question that looked like a free improvement and is not.
`make_request_span` records `path = uri().path()` — the concrete URL — so the
log cannot be aggregated by route, and the obvious fix is to record the pattern
there instead. It cannot: that span is opened by `TraceLayer`, which is attached
with `Router::layer` and therefore runs before routing has decided anything.
Aggregating the log by route needs the span to be opened somewhere else, which
is a change to the log and not to the profiler, and is not made here.

After hydration there is no server request to match, so the client's route comes
from `leptos_router` and is *pushed* to the toolbar one-way, by an effect
calling into a global the toolbar owns. Nothing in the app reads anything back.
See section 7 for why that direction is not negotiable.

### Time

Wall time per request is exact and free: an `Instant` at the top of the
middleware. The number worth looking at is the breakdown, not the total, and the
breakdown comes from the spans that are already open.

### Queries

sqlx emits a `tracing` event per statement with the SQL and its elapsed time.
Collecting those events against the request that caused them gives the panel
this whole exercise is really for — the equivalent of Symfony's Doctrine tab —
with no instrumentation in any query.

It also gives the N+1 detector for free: a group where the same normalised
statement appears eleven times is a screen with a loop in it, and that is worth
saying loudly in the UI rather than leaving to be noticed.

### Which of your files ran it — the one thing the registry cannot answer

"There is a slow query" is half an answer. The other half is *where it came
from*, and that is the one part of this profiler that the registry route of
section 4 cannot supply:

* The event is emitted by sqlx, so `metadata().file()` is
  `sqlx-core/src/logger.rs` on every single row.
* The span stack around it is **one frame** — `http`. `#[instrument]` appears
  nowhere in this workspace, so there is no `resolve_tenant` span, no service
  span, nothing.

The obvious fix is to instrument the hundreds of functions that touch a
database. That is precisely the rot section 4 rejects: every new query has to
remember to declare itself, and the profiler is silently wrong about the ones
that forget.

So this one thing walks the stack instead. It is legitimate here for a reason
that does not generalise: an `.await`ed future is polled from its caller's
poll, so at the moment sqlx logs the statement the chain from the axum handler
down to the query really is on the stack.

**The cost is split, and that split is the whole design.** On the request, a
walk that records instruction pointers and nothing else — measured at **756 ns**
a capture, so a screen running forty statements pays about thirty microseconds.
Turning those addresses into function names and file positions is two orders of
magnitude more expensive, and it happens only when a human opens the report and
is already waiting for a page. Resolving later is sound because a profile never
outlives the process that recorded it: the ring is in memory and dies with the
binary the addresses point into.

Only this workspace's frames are kept — a stack that is ninety per cent tokio
is not a stack anybody reads — and the profiler's own frames are dropped,
because they are on every capture, at the top, always.

A run of identical frames collapses to one. An `async fn` is on the stack
twice, as itself and as the poll the compiler generated for it, at a single
source position; once the machinery segment is trimmed off the name the two
rows are the same function, file and line. Only *consecutive* repeats go — the
same position reached again with something between is a real cycle, and that is
worth seeing.

`profiler.backtraces` switches it off. It is the only part of the profiler that
does work per *statement* rather than per request, which is the only reason it
gets a switch of its own.

A log line needs none of this: the application is what emitted it, so its own
metadata already names the right file. One trap, paid for: a resolved stack
frame carries an absolute path, while `metadata().file()` carries one already
relative to the workspace root — code that matches only the absolute form
leaves every log line claiming it has no position.

### Payload size

The SSR HTML's size, and the size of the serialised hydration payload embedded
in it. When a Leptos page feels heavy this is overwhelmingly the reason, and
unlike a memory figure it points at something a developer can act on.

Only half of it is free, which is why phase 1 has the cheap half. A response
that declares `Content-Length` is measured by reading it. Leptos streams its
HTML and declares nothing, so measuring a page means wrapping the body in a
counting wrapper — which puts the profiler inside the path the response is
delivered on, and makes it the first thing anybody blames for a streaming bug.
An unmeasured body is reported as unmeasured, never as zero.

Phase 2 crossed that line for a different reason and stopped short of the
measurement: the toolbar's `<script>` tag is appended to the streamed body
(section 7), so the profiler is already in the delivery path. The counter is
now a small addition to a wrapper that exists — and it is still not built,
because the number is only known once the client has drained the body, and the
profile is filed before that. Making it work means a filed profile whose size
fills in afterwards, which is a mutable profile in an otherwise immutable ring.
Worth doing when somebody wants the number; not worth doing for tidiness.

### Memory — the honest section

**There is no per-request memory figure in Rust, and the profiler will not
pretend otherwise.**

`memory_get_peak_usage()` works in PHP because the process serves one request
and its arena is torn down afterwards. This process serves every request
concurrently through one allocator. Three things are available, and only the
first two are being built:

1. **Process RSS**, sampled at the start and end of a request. Free, no
   allocator hook. Under any concurrency it describes the process, not the
   request. Kept as a gauge of whether the process is growing, labelled as
   exactly that.
2. **Payload sizes** (above), which answer the question that usually sits behind
   "how much memory did this page use".
3. **Bytes allocated on the request's task**, via a wrapping `GlobalAlloc`
   incrementing a `tokio::task_local!` counter. This *is* per-request and it is
   *not* accurate: it counts bytes allocated rather than peak resident, and
   anything that leaves the request's task — spawned work, the pool's background
   tasks, connection buffers — is not counted. It also costs a branch and an
   atomic on every allocation in the process, so it can only exist behind a
   compile-time feature.

(3) is deferred, deliberately. It is the item most likely to be built because it
sounds like the Symfony number and then trusted because it is a number. If it is
ever added it ships with the caveat rendered next to the figure, not in a
document nobody opens.

## 4. The data comes from the tracing registry

`phonix-telemetry` already builds a `Vec<Box<dyn Layer<Registry>>>` and hands it
to one `Registry`. The profiler adds one more element to that Vec, and
`phonix_telemetry::ExtraLayer` names the type so a contributing crate needs no
tracing-subscriber dependency of its own.

The collector is reached through a **task-local**, not through the span
registry. The middleware wraps the whole downstream future in it, and the layer
appends whatever it sees to whatever is in there. The registry route — hang a
collector off the `http` span's extensions, find it again by walking up from the
event — is the more thorough mechanism, and reaching it from the middleware
costs a subscriber downcast through `WithContext`. A task-local needs none of
that and gets the same answer for the case that matters, because a request is
one task.

What that gives up, stated so it is not later rediscovered as a bug: **work that
leaves the request's task is not collected.** A `tokio::spawn` inside a handler,
and the pool's own background work, emit outside the scope. For a request
profiler that is the right answer — those events did not belong to the request —
but it means the query list is what this request ran, not everything the process
ran while it waited.

The one thing the layer does read from the span is the tenant. `resolve_tenant`
already records it on the surrounding span so that every log line is
attributable to one; the profiler reads that same field, which is why it needs
no cooperation from the tenant middleware and no dependency on the type it
resolves. The seam is a span *name*, so there is a test in `phonix-server` that
fails if `make_request_span` is ever renamed.

This is the whole architectural bet, and it is worth stating why it is the right
one. The alternative is for the profiler to instrument the code it profiles —
timers in the middleware, a query counter threaded through the pool, a hook in
the SSR renderer. That version rots: every new subsystem has to remember to
report itself, and the profiler is silently wrong about the ones that forget.
Reading the registry inverts it. Anything that opens a span or logs an event is
profiled the day it is written, by nobody's decision.

### It carries its own filter, and that is not a compromise

The obvious arrangement is one `EnvFilter` for the process: one filter, one
truth. It does not survive contact with the first requirement. sqlx logs a
statement at DEBUG and `[telemetry]` sets `sqlx::query = "warn"`, so under a
shared filter the query panel would be present, correct, and permanently empty
— which is worse than absent, because an empty panel reads as "this request
ran no queries".

Raising the shared filter instead would put every statement in the terminal,
and a profiler that makes the console unusable is a profiler that gets turned
off. So `[profiler] filter` is separate, defaulting to `info,sqlx::query=debug`,
and it is layer-local: what the profiler records has no effect on what the
console and the file record, in either direction.

The cost is real and worth naming — there are now two filters, and somebody
who silences a target in one will be surprised to still see it in the other.
The startup line prints the profiler's filter for that reason, and an empty
query panel says which filter to go and check.

## 5. Storage is a bounded ring in memory

`Arc<Mutex<VecDeque<Profile>>>`, capped at a few hundred, with a token index.
Symfony writes files to `var/cache`; this does not, initially, because the
in-memory version is a smaller thing to get right and the retention a developer
actually needs is "the last few minutes".

The known cost: `cargo leptos watch` restarts the process on every save, and the
history dies with it. If that turns out to bite — the case being "I changed a
file and lost the profile I was reading" — profiles spill as JSON to
`var/profiler/`, which is already outside git. Not built until it bites.

The cap is a hard cap, not a target. This is a development tool holding request
and response detail in memory; an unbounded one is a memory leak that eats a
developer's machine overnight, and it would do it for the most engaged user.

## 6. The UI is not Leptos

The report and the toolbar are plain HTML and vanilla JavaScript, served from
their own routes, sharing nothing with the application's reactive graph. Three
reasons, in ascending order of how much they settle the question:

1. **Styling.** The app's Tailwind must not leak into the toolbar, and the
   toolbar's styles must not leak into a page being debugged. A shadow root
   ends this completely; a Leptos component fights it forever.

2. **Hydration.** Markup in the SSR document that the client render does not
   expect is a hydration mismatch, and mismatches here take the page — this is
   settled ground in this codebase, at some cost. A profiler that can break the
   application it is profiling is worse than no profiler, because the first
   thing anybody will suspect is their own code.

3. **A wasm panic freezes the entire application.** It is uncatchable by design.
   The moment a developer most needs the profiler is the moment the app has
   died, and a Leptos component dies with it. A vanilla toolbar in a shadow root
   survives, keeps its data, and can show what the last request did before
   everything stopped.

Reason 3 is on its own sufficient, and it also sets the standard the
implementation is held to: **the profiler must work on a page whose application
has panicked.** No import from the app, no dependency on hydration having
finished, no reactive value read from anywhere.

### It is served the way the API reference already is

`/api/v1/docs` is precedent, not analogy: a standalone HTML page with a vendored
hashed bundle served from this origin, built by a script under `tools/` and
committed. The profiler follows it exactly.

| | |
|---|---|
| `/_profiler` | the report — standalone HTML |
| `/_profiler/{token}` | one page load, or one request |
| `/_profiler/api/*` | JSON; read by the report and by the toolbar |
| `/_profiler/toolbar.js` | the injected toolbar |

Phase one's report is **server-rendered HTML with no JavaScript at all**. It
ships the value on the first day and defers the question of a bundle until the
tool has earned one. A profiler nobody uses should not have cost a build step.

## 7. The toolbar emits nothing into the SSR document

The toolbar is appended by a `defer` script *after* hydration completes. It is
never present in the server's HTML, because the server's HTML must be exactly
what the client render expects (section 6, reason 2).

It lives in a shadow root on a host element appended to `document.body`, and it
is `position: fixed` with a hard `max-width`. Anything wider than the viewport
inflates the viewport on a phone and throws fixed overlays off-screen — an
already-recorded property of this layout that a debugging tool has no excuse for
reintroducing.

Two data paths, both one-way out of the application:

* **Route changes.** An effect in the app calls `window.__phonix_profiler?.route(path)`.
  Optional chaining, so with no profiler the call is a no-op, and the app never
  reads a value back.
* **Server calls.** The toolbar patches `window.fetch` to add `X-Phonix-Page`
  and to read `X-Debug-Token` off the response. This is how Symfony's toolbar
  lists AJAX sub-requests, and it requires no cooperation from the caller —
  which matters, because the caller is Leptos' server-fn client and not ours to
  change.

## 8. It is gated twice

A profiler holds request headers, response bodies and SQL. On a production
deployment it is a data breach with a URL.

1. **A cargo feature.** `phonix-profiler` is an optional dependency of
   `phonix-server` behind `--features profiler`. Without it the crate is not
   in the binary, so the routes cannot exist and the collector cannot run. It
   is deliberately *not* in `bin-features`, so the ordinary build — the one
   `phonix-deploy` runs — does not have it, and a developer opts in per run:

   ```
   cargo leptos watch --bin-features "ssr,profiler"
   ```

   `ssr` is repeated because `--bin-features` **replaces** the manifest's
   `bin-features` rather than adding to it (cargo-leptos 0.3.7,
   `config/bin_package.rs`). Dropping it builds a server with no `ssr`
   feature, which is not this application. The neighbouring `--features`
   flag is not the answer either: it is appended to the lib target as
   well, and `phonix-web` has no `profiler` feature to enable.

   A binary built without the feature but configured with `enabled = true`
   says so on stderr rather than starting quietly with no profiler. The
   alternative is an afternoon spent debugging a tool that was never there.
2. **A config key.** `profiler.enabled`, which `validate::check` refuses to
   accept as `true` under `production` — the same refusal already applied to
   `tenancy.auto_provision` and `telemetry.tracing.log_bodies`, for the same
   reason and with the same shape of message.

Either alone would do. Both, because the failure is silent and permanent: a
profiler left on in production announces nothing, and the first sign of it is
somebody else reading it.

The routes are registered only when both agree. There is no runtime toggle, no
header that turns it on, and no allow-list of IPs — every one of those is a
mechanism that can be wrong, and the answer here is for the code not to be
present.

## 9. Where it sits in the stack

Two pieces, in two places, for reasons the router's existing comments already
establish.

The **collector layer** goes into the `Vec` in `phonix-telemetry`, so it sees
spans and events from everything — a background job as much as a request.

The **middleware** is two middlewares, for the reason in section 3.

The outer one sits directly inside `TraceLayer`, so it runs within the `http`
span it needs to hang data on, and *outside* `resolve_tenant`, so the tenant is
recorded and the statement that resolved it runs while the collector is
listening. It mints the token, times the request, files the profile, writes
`X-Debug-Token` on the response and appends the toolbar's tag to the body.
Layers apply bottom-up, so in `startup` that means: after `.layer(resolve_tenant)`
and before `.layer(TraceLayer)`.

The inner one is a `route_layer`, and all it does is write `MatchedPath` into
the collector the outer one opened.

`/_profiler/*` is merged **outside** the application's outer layers - after
`with_state`, after `resolve_tenant`, `TraceLayer` and the rate limiter, and
only then under `CatchPanicLayer`. Registration order alone would not do it:
`Router::layer` wraps every route already on the router regardless of the
order they were added in, so a route that must escape a layer has to be added
after it. Three things follow, and each is a reason: the report answers when
resolving a tenant is the thing that is broken; it is not counted against a
rate limit meant for the application; and it does not emit an `http` span of
its own into the log it is displaying.

## 10. Build order

Each phase is useful on its own and none of them assumes the next is coming.

| Phase | | |
|---|---|---|
| 1 | Crate, config gate, collector, ring, request timings, matched route, query panel, server-rendered report | built |
| 2 | Toolbar, `page_id` grouping, patched `fetch`, route push | built |
| 3 | A real bundle for the report, if phase 1 earns it | conditional |
| 4 | Allocation counting behind its feature, if phase 2 has not already answered the question | deferred, section 3 |

## 11. What was considered and not chosen

**OpenTelemetry into a local Jaeger.** One more layer in the same `Vec`, and it
produces a span waterfall better than anything this will draw. It was not chosen
as a *replacement* because it answers only "why was that slow" — it has no
on-page presence, no request and response detail, and nothing that orients
somebody who does not yet know what they are looking for. It remains the right
answer for production tracing, and nothing here forecloses it: both are layers
on the same registry, and they can coexist.

**Instrumenting the code directly.** Section 4.

**Serving the profiler from a separate port or process.** It would sidestep the
tenant middleware and any chance of the app's styles leaking. It also puts the
profiler on a different origin from the page, which breaks the toolbar's link to
its own report, and it doubles the thing that has to be gated in section 8. The
routes are cheaper.
