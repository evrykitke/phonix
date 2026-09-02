# ADR 0002 — The public API: a versioned surface, and the credential that opens it

Status: accepted, built 2026-08-28, verified 2026-08-31
Date: 2026-08-28

Phonix is a Leptos application, and its server functions are already an HTTP
surface. This record is about the *other* one: a documented, versioned REST API
with an OpenAPI specification, for callers that are not this browser.

Three of them are already foreseeable, and they are the reason the surface has
to be deliberate rather than incidental:

* **A mobile app.** It cannot use a session cookie set on a workspace
  subdomain, and it cannot be redeployed the afternoon a payload changes.
* **A customer extending their own workspace.** Their script is written once
  and runs for years against whatever we ship next week.
* **A licensable capability.** "API access" is a thing a plan grants, which
  means there has to be one place that can refuse it, and administrators have to
  be able to issue and revoke the credential themselves.

None of those tolerates a surface that changes when we refactor. That single
sentence is what every decision below follows from.

---

## 1. It is mounted on the services layer, not on the server functions

`phonix-services` is the application layer: one function per use case, taking a
[`Caller`](../../crates/phonix-services/src/caller.rs) and naming its permission
on the first line. It has no `leptos` dependency, it is already reachable from a
background job, and `Caller::require` is already the gate an API wants.

The server functions are thin adapters over exactly that - resolve the pool and
the caller, call the service, map the error. Wrapping *them* would buy nothing
and would cost two things we would not get back:

* the published surface would be coupled to `ServerFnError` and to cookie
  authentication, neither of which a mobile client wants;
* the 104 server-function endpoints would become a contract. They are named and
  stable (`#[server(prefix = "/api", endpoint = "currencies/save")]`) but they
  are *internal*: free to be renamed, split, or deleted the day the screen above
  them changes. Publishing them would end that freedom silently.

So: **`/api/v1/*` is a plain axum router in `phonix-server`, calling
`phonix-services` directly**, mounted the way `files::routes` already is - see
section 9 for where in the stack, which is not a detail.

### The two surfaces, stated once

| | Server functions | Public API |
| - | - | - |
| Path | `/api/<name>` | `/api/v1/<resource>` |
| Caller | this browser | anybody holding a key |
| Auth | session cookie | `Authorization: Bearer` |
| Contract | none; changes with the screen | frozen within a major version |
| Errors | `Error` as JSON, `Message` keys for text | RFC 9457 `problem+json`, machine codes |
| Documented | no | OpenAPI |

They are not layered on one another, and neither deprecates the other. They are
two adapters over one application layer, which is the whole point of having an
application layer.

## 2. Versioning: `v1` is in the path, and it only grows

The version is a path segment, not a header. A header-negotiated version is
invisible in a log line, in a `curl` a customer pastes into a support ticket, and
in the browser bar of somebody exploring; a path segment is none of those things,
and every consumer of a small API gets it right by accident.

Within `v1`, changes may only be **additive**:

* a new endpoint;
* a new *optional* request field;
* a new field in a response object.

Everything else - removing a field, renaming one, tightening a validation,
changing a status code, making an optional field required - is `v2`. A client
built against `v1` has to keep working, and "nobody was using that field" is not
knowable from here.

Which imposes one rule on the code, and it is the rule this ADR exists to make
enforceable:

> **The wire types are declared in the API module and nowhere else.**

`CurrencyResource` in `phonix-server/src/api/` is a separate type from
`phonix_db::currency::CurrencyRow`, with a hand-written conversion between them.
Deriving `ToSchema` on the internal types would have been less typing and would
have made every internal rename a silent breaking change to a published spec -
the exact failure this record exists to prevent. The conversion function is where
the two shapes are allowed to disagree, and it stops compiling when the internal
one moves.

It has a second benefit worth naming: `utoipa` stays out of `phonix-core`, and
therefore out of the WebAssembly bundle that ships to every browser.

## 3. Authentication: API keys, and what a key is allowed to be

Sessions are cookies with host-to-tenant resolution in middleware. An API cannot
use them: there is no browser to hold the cookie and no login form to fill in. So
`v1` authenticates with an opaque bearer token, presented as
`Authorization: Bearer <token>`.

### The token

`crypto/token.rs` already mints opaque bearer tokens for sessions and one-time
links, and its rule is the one that matters here: **the database stores a
SHA-256 digest, never the token**. A dump of `api_keys` cannot be replayed. API
keys reuse that primitive unchanged, with one addition:

```
phx_<43 url-safe base64 characters>
```

The `phx_` prefix is not decoration. It is what lets a secret scanner recognise
one of our credentials in a public repository, and what lets a person reading a
configuration file know what they are looking at. It is stripped before the
digest is computed, so the stored bytes are the same shape as a session's.

The full token is shown **once**, at creation, and cannot be recovered
afterwards. What the administration screen shows from then on is the prefix and
the last four characters, which is enough to answer "which key is this" and
useless to anybody reading it over a shoulder.

### A key belongs to a user, and can never exceed them

A key is issued *by* a user and acts *as* that user. It is not a service account
with a permission set of its own.

```
effective permissions = the owner's current permissions  ∩  the key's scopes
```

Both halves are load-bearing:

* **The intersection with the owner** means a key cannot be a privilege
  escalation. Revoke the user's `Settings` grant, suspend the account, delete it,
  and every key it issued loses exactly what it lost, at the next request, with
  no key-side bookkeeping at all. A suspended or half-authenticated account
  already holds nothing (see `Caller`), and so, therefore, do its keys.
* **The scopes** mean a key handed to a phone app, or to a customer's script, can
  be narrower than the person who issued it. That is the half a customer
  extending their workspace will actually use.

**Scopes are permission names.** Not a second vocabulary of `currencies:read` -
the permission tree already exists, is compiled into both sides, is already what
`Caller::require` checks, and already has an editor UI. An API-specific scope
language would mean two lists to keep in agreement, and they would disagree
within a release. A scope is therefore a node from
`phonix_core::authorization::DEFINITIONS`, and holding a parent implies its
children exactly as a grant does.

This also settles how an installed app's endpoints will work when they arrive: an
app owns a permission subtree, so a key scoped to that subtree is scoped to that
app, and nothing extra has to be invented.

### What a key is not, in v1

* **Not a way to sign in.** There is no "log in with an API key" screen, and a
  key never produces a session cookie.
* **Not a refresh/access token pair.** OAuth client credentials, per-key IP
  allowlists and short-lived exchanged tokens are all reasonable and all later;
  none is needed by a first mobile app, and each would have to be supported for
  as long as `v1` lives.
* **Not a way around a second factor.** A key is issued from a fully
  authenticated session and inherits its owner's standing; it does not let
  anybody *become* that user in a browser.

### Expiry and revocation

Every key has an optional `expires_at` and a `revoked_at`, and both are checked
in the same statement that looks the key up - for the reason sessions do it that
way: an expired credential must not be resurrectable by a code path that forgot
to check. `last_used_at` is written best-effort and coarsely, because "is this
key still in use" is the question every administrator asks before revoking one,
and a write on every request to answer it precisely is not worth the contention.

## 4. Licensing: one switch, checked in one place

API access is a licensable capability, so a workspace can have the software
without having the surface.

`workspace_settings.api_enabled`, **default false**. It is checked once, in the
API router's authentication layer, before the key is even looked up:

* off, and every `/api/v1/*` call answers `403` with code `api_disabled`,
  whatever key was presented;
* on, and the request proceeds to the key.

Deliberately *not* a permission. Permissions say what a person may do inside a
workspace; this says whether the workspace has the feature at all, and putting it
in the permission tree would make it something an administrator can grant
themselves.

The switch is per workspace and lives in the workspace's own database, so
enabling it is an ordinary administrative act rather than a deployment. Where the
switch is *set from* - a plan, a licence file, a support console - is not decided
here. What is decided here is that there is exactly one flag, and one place that
reads it.

## 5. Errors: RFC 9457, and a code that is not a translation key

Every user-facing string in Phonix is a `Message` key resolved by the view. That
is right for a browser and wrong for an API consumer: a key is a *label for a
sentence*, the sentence is translated, and neither is a stable thing to branch
on. A script needs a machine code.

`phonix_core::Error` already has both halves - `status_code()` and `code()`, with
a test asserting the statuses match the semantics - so the body is:

```json
{
  "type":   "urn:phonix:problem:validation",
  "title":  "The request was not valid.",
  "status": 422,
  "code":   "validation",
  "detail": "currency: EUR is not on this workspace's list.",
  "errors": [
    { "field": "currency", "code": "error.currency.not_listed",
      "message": "EUR is not on this workspace's list." }
  ]
}
```

RFC 9457 `application/problem+json` rather than a shape of our own, because it is
the one error format an HTTP client library might already understand, and because
arguing about the field names of a bespoke envelope is time spent on nothing.

Three decisions inside it:

* **`code` is the contract.** It comes from `Error::code()`, it is stable within
  a major version, and it is what a client branches on. `title` and `detail` are
  for a person reading a log.
* **Field rejections survive as `errors[]`**, because a form on somebody else's
  phone has to know which field was refused. Each carries the `Message` key as
  its `code` - stable, and a client that wants its own wording can key off it -
  plus the English rendering, so a caller who does not want to build a catalog
  still has something to print.
* **Nothing else crosses.** `ServiceError -> Error` already strips connection
  strings, SQL fragments and key descriptions and logs them server-side. The API
  inherits that conversion rather than reimplementing it, which is why no handler
  matches on `ServiceError` directly.

`401` when no key was presented or it is not live; `403` when the key is live but
its effective permissions do not cover the operation. The distinction is what
tells a client whether to re-issue a credential or to ask for a broader scope, so
it is worth being careful about.

## 6. Listing: the paging contract, spelled once

`PageRequest` already exists and is already the vocabulary the browser, the
server functions and the DBAL agree on. `v1` exposes it as query parameters, and
the mapping is the whole contract:

```
GET /api/v1/currencies?page=1&per_page=25&sort=code&order=asc&q=eur&filter[enabled]=true
```

| Parameter | Maps to | Notes |
| - | - | - |
| `page` | `PageRequest.page` | 1-based, as the pager and the URL both are |
| `per_page` | `PageRequest.per_page` | default 25, ceiling 500, clamped not refused |
| `sort` / `order` | `PageRequest.sort` | `order` is `asc`/`desc`; a field the reader does not know is ignored |
| `q` | `PageRequest.search` | free text |
| `filter[<name>]` | `PageRequest.filters` | named predicates, answered where the rows are |

`sanitised()` runs before anything acts on a request, so page zero becomes page
one and a request for a million rows becomes 500. **A bad paging parameter is
clamped, never refused** - the alternative is an API that returns 422 for
`?page=0`, which is a worse answer than the first page.

The response envelope separates the rows from the pager, because a client
appending to a list needs to know whether there is more without counting:

```json
{
  "data": [ ... ],
  "page": { "page": 1, "per_page": 25, "total": 163, "page_count": 7 }
}
```

A single resource answers the object itself, unwrapped. Wrapping one record in a
`data` key buys consistency with the list and costs every caller an extra
dereference for the rest of the API's life.

## 7. Rate limiting: the public API needs its own tier

`rate_limit::classify` currently lets everything under `/api/` past uncounted,
with an explicit reason: those are signed-in calls, and `Caller::require` is a
better control than a counter. That reasoning is correct for a browser holding a
session cookie and wrong for a key a script can hold in a loop.

So `/api/v1/*` is counted in its own tier, keyed by **the credential** rather
than by IP: a mobile fleet shares an IP, a datacentre client changes one, and
the credential is the thing whose behaviour we mean to bound. A request that
arrives without a bearer token falls back to the IP key, so an unauthenticated
flood is still counted.

The limiter runs *above* tenant resolution - deliberately, so a flood of unknown
subdomains cannot be used to hammer the catalog - which means it has no database
and therefore cannot know the key's id. It keys on a one-way hash of the
presented token instead, never the token itself: this string lives in a map for
the life of the process, and a heap dump holding live API keys would be a worse
leak than anything the limiter prevents.

The residual, stated so nobody has to rediscover it: a caller who rotates
syntactically valid tokens gets a fresh allowance per token. Each such request
still costs exactly one indexed lookup and is refused, and closing it properly
means counting *after* authentication, against the key id. That is worth doing
when there is a reason to; it is not worth an extra database round trip on every
request today.

The allowance is configuration (`[rate_limit] api_requests`, `api_window_secs`),
generous by default. `429` carries `Retry-After`, and `Error::RateLimited`
already maps to it.

## 8. The specification and the documentation are served by the application

`GET /api/v1/openapi.json` is generated by `utoipa` from the handlers and the
wire types, so it cannot drift from what the code does - a hand-written spec is
wrong within a month and nobody finds out until a customer does.

`GET /api/v1/docs` serves Scalar against that spec: a page a customer's developer
can read, try a call in, and paste a `curl` out of.

Both are unauthenticated. The specification describes the *software*, not a
workspace: it is byte-identical for every tenant, it contains no data, and
everybody who needs it needs it before they have a credential. Requiring a key to
read the documentation that explains how to get a key is a circle worth not
drawing.

The Scalar bundle is **vendored, not fetched** (added 2026-08-29). The
`utoipa-scalar` crate's own template loads `@scalar/api-reference` from a CDN
with no version in the URL, which makes this page blank without internet and
lets whatever that CDN answers with today execute inside our origin - on the
one page a customer's developer reads *before* deciding to trust us.
`node tools/vendor-scalar.mjs` pins a version, writes
`public/app-assets/scalar.<hash>.js` and generates the constant the template
points at, exactly as the editor bundle is handled. Serving the specification
from the application and the renderer from somebody else's would have been
half a decision.

`utoipa` over the alternatives because it generates from the handler signatures
rather than from a parallel description, because `utoipa-axum` registers a route
and its documentation in one call (so an endpoint cannot be added without
appearing in the spec), and because it needs no build step of its own.

## 9. Where it sits in the stack

Mounted with `Router::nest`, not `merge`, and that is not a stylistic preference:

* the application's fallback renders a **Leptos error page**. A nested router
  keeps its own fallback, which is what makes an unknown `/api/v1/...` path
  answer `404 problem+json` instead of a page of HTML no client can parse.
  `merge` cannot do this: two fallbacks in one router is a panic at startup.
* the tenant middleware and the tracing layer sit *below* everything, so the API
  resolves its workspace from the host exactly as a page does, and every log line
  for an API request already carries its tenant.

The 2 MiB body limit and the 30-second timeout do apply, and both are right here:
no `v1` endpoint takes a file, and one that eventually does will follow
`files::routes` and carry its own.

## 10. Scope of the first cut

**Currencies**, because it is the smallest thing that exercises every decision
above: its reads are ungated (so a key with no scopes still gets a 200 and proves
the auth path end to end), its writes require
`Pages.Administration.Settings` (so a scoped key proves the gate), its types are
small, and it is a singleton for audit purposes, so nothing about the audit trail
has to be invented alongside.

```
GET    /api/v1/currencies            list, paged
GET    /api/v1/currencies/{code}     one
PUT    /api/v1/currencies/{code}     save: enabled, symbol   (Settings)
```

Not in the first cut, and each for a stated reason:

* **Every other resource.** One resource proves the model; the second is an
  afternoon once it is proved, and doing them together means changing eleven
  handlers when the error body turns out to be wrong.
* **Exchange rates.** A rate is a series with a source and a date, and its query
  shape deserves its own thought rather than being dragged along behind
  currencies.
* **Webhooks.** The outbox exists; delivering from it to a customer's endpoint is
  its own project, with its own retry semantics and its own signing.
* **Cursor pagination.** Offsets are wrong for a large export and right for
  everything a screen does. When an export endpoint exists it can carry a cursor;
  adding one to `v1` later is additive.

### Amended 2026-08-31 — users, and what the second resource cost

The afternoon happened, and the estimate held. **Users** is the second
resource, read-only:

```
GET    /api/v1/users                 list, paged            (Users)
GET    /api/v1/users/{id}            one                    (Users)
```

It was chosen over the larger candidates because it proves the two things
currencies structurally could not:

* **A gated read.** Currencies reads ungated, so every 200 the first resource
  ever returned was also a 200 for a key carrying no scopes at all. `Users`
  requires `Pages.Administration.Users`, so the intersection of the owner's
  grants with the key's scopes is now load-bearing on a *read* and not only on
  the one `PUT`.
* **An opaque address.** A currency is reached by a code the caller already
  knows; an account is reached by an id that only the list can supply. That is
  the ordinary shape of every resource after this one.

Three things the build settled, worth recording because they are decisions the
third resource should not have to make again:

1. **`ServiceError::rejected` is a 422, and a missing row wants a 404.**
   `directory::find` answers a missing account with a rejection, which
   §"Errors" renders as a validation problem with a field in it. That is right
   for a form and wrong for an address. The handler therefore does its own
   lookup over the same list `find` scans, and spells the 404 itself - exactly
   as `currencies::get` already did for an unknown ISO code. **A `find` on a
   service is not automatically a `GET` on a resource**, and the difference is
   the status.

2. **An unknown *sort field* is ignored; an unknown *filter value* narrows to
   nothing.** §"Paging" says a bad parameter is clamped rather than refused,
   and that still holds - but the two are not the same request. Sorting by a
   column this build does not have is a client asking for an ordering, and any
   ordering answers it. Filtering on `status=retired` is a client naming a set,
   and handing back everybody would assert that everybody is in it. Neither is
   a 422; the difference is what "clamped" means for each.

3. **Paging in memory is a property of the service, not of the resource.**
   §"Paging" was written as though currencies were the exception because it is
   bounded. It is not the rule: a handler pages in memory while the use case it
   calls hands back the whole list for the screen's own reasons, and passes the
   `PageRequest` down the day that use case takes one. The wire contract is
   identical either way, which is the point - `api::paging::cut` now owns the
   clamp-and-cut tail so no resource can get `total` or the last page wrong on
   its own.

Still not here, and still for the reasons above: writes to users (roles and
status move together, and a key must not be able to hand itself a role),
exchange rates, webhooks, and cursor pagination.

### Amended 2026-08-31 — roles, and the resource that closes the loop

**Roles** is the third, read-only:

```
GET    /api/v1/roles                 list, paged            (Roles)
GET    /api/v1/roles/{id}            one, with its grants   (Roles)
```

It is not here because it was easy. §2 says a scope **is a permission name**,
and until this resource existed nothing on the published surface said which
names exist or which of them a role confers — so building a correctly-scoped
key meant reading them off the administration screen by eye, and `User.roles`
was a list of names with nowhere to resolve them. `GET /roles/{id}` answers
both.

* **Two shapes, not one.** The list carries `permission_count`; the detail
  carries the set. That is the split `RoleSummary`/`RoleDetail` already draws,
  and collapsing it would make every row of every list drag a whole permission
  set along.
* **The detail nests `Role` rather than merging it.** A client that already
  holds one from the list reads it into the same type, and a field added to
  either cannot collide with the other. Merging is additive-safe only until two
  names meet.
* **A two-valued filter has no third answer.** `filter[static]` and
  `filter[default]` narrow the other way on anything that is not `true`, rather
  than refusing. This is not in tension with the users rule above: there, a
  *status* names one of four sets and a fifth name matches none of them; here
  there are exactly two, so "not true" and "false" are the same set.

**One thing the first two resources hid**, and worth stating as a rule: a
tie-break must sort the same way the default does. Roles' first draft compared
raw names while the default lowercased, so `Bookkeeper` sorted before `auditor`
on a tie and after it by default — the same two rows swapping places depending
on which column was sorted. `roles.name` is matched case-insensitively, so the
lowercased name is both consistent and still unique. **Every in-memory
paginator here must end in a tie-break that is total and agrees with its own
default**, or paging shows one row twice and another never.


### Amended 2026-09-01 — the administration area, finished

Everything the administration screens can do, `/api/v1` can now do. The surface
is thirty-nine operations across nine tags; twenty-eight of them are new, and
they include the writes §10 deferred:

```
GET    /permissions                    the tree scopes are named from   ungated
GET    /users                          list                             (Users)
POST   /users                          invite                           (Users.Create)
GET    /users/{id}                     one                              (Users)
PUT    /users/{id}                     name, status, roles              (Users.Edit)
POST   /users/{id}/invitation          reissue                          (Users.Create)
GET    /users/{id}/permissions         by source                        (Users)
PUT    /users/{id}/permissions         replace                          (Users.ChangePermissions)
GET    /roles, /roles/{id}             list, one with grants            (Roles)
POST   /roles                          define                           (Roles.Create)
PUT    /roles/{id}                     rename                           (Roles.Edit)
DELETE /roles/{id}                     remove                           (Roles.Delete)
PUT    /roles/{id}/permissions         replace what it grants           (Roles.ChangePermissions)
GET    /api-keys                       list, live and revoked           (ApiKeys)
POST   /api-keys                       issue, token once                (ApiKeys.Create)
POST   /api-keys/{id}/revoke           stop                             (ApiKeys.Revoke)
GET    /apps                           the catalog and this workspace   (Apps)
POST   /apps/{id}/install              switch on, with dependencies     (Apps.Install)
POST   /apps/{id}/uninstall            switch off                       (Apps.Install)
GET    /settings/security              password, MFA, audit policy      ungated
PUT    /settings/security              replace                          (Settings)
GET    /settings/organization          who the workspace is             (Settings)
PUT    /settings/organization          replace                          (Settings)
GET    /settings/mail                  the relay, never its password    (Settings)
PUT    /settings/mail                  replace                          (Settings)
GET    /settings/api                   the licence                      ungated
PUT    /settings/api                   sell it, or stop                 (Settings)
GET    /audit/changes, /{id}           the change trail                 (AuditLogs)
GET    /audit/events, /{id}            the security trail               (AuditLogs)
```

Nine decisions the build settled. Each is here because the next resource
should not have to make it again.

1. **The user writes are safe, and §10's worry has an answer.** That section
   held them back because "roles and status move together, and a key must not
   be able to hand itself a role". Two existing mechanisms answer it. Changing
   roles requires `Users.ChangePermissions`, asked for by the service *only
   when the roles actually differ* — so renaming somebody needs `Users.Edit`
   alone. And a key cannot widen itself: its power is its owner's current
   grants ∩ its own scopes, re-read per request, so granting its owner more
   moves the first half and not the second.

   What is left is that somebody holding `Users.ChangePermissions` can escalate
   *themselves*. That has always been true of that permission and is what it
   means. It is not a property of this surface, and refusing here would only
   make the browser and the API disagree about who may administer a workspace.

2. **An adapter must not add a gate the service does not have — the converse
   of the rule under "Consequences".** Three reads on this surface are
   ungated: the security policy, the API licence, and the permission tree.
   Gating them "because they are administration" would have made the API
   refuse what the browser allows, with nothing to report the difference. The
   password policy is readable by everybody because somebody choosing a
   password has to be told the rules; the API flag is readable because the
   screen that manages keys has to be able to say the API is off; the
   permission tree is a compiled `const` describing the software, not a tenant.

3. **`/settings` is four sub-resources, not one document.** Merging them would
   force the strictest gate onto the loosest part — the password policy is
   ungated, the organization's registered address is not — so a single
   `GET /settings` would mean only administrators could find out how long their
   password has to be.

4. **A `PUT` is the whole document, and no field defaults.** An omitted
   `enabled` defaulting to `false` would let a caller who sent half a policy
   switch the audit trail off without naming it. "I did not mention it" must
   not mean "turn it off". A client changing one field reads, edits and writes
   back, which is also what makes two concurrent saves produce one of the two
   documents rather than a mixture.

5. **A refusal the service models as a value is still a refusal on the wire.**
   `uninstall` answers `AlwaysOn` and `NeededBy` as ordinary outcomes, which is
   right where a screen renders both beside the button. A script that read
   `200` and moved on would have been told an app is off while it is still on.
   They became `409 app_always_on` and `409 app_required_by` — the second
   naming the dependant, because "no" without a reason is a dead end.
   `SwitchedOff`, including "it was already off", stays a `200`: that is a true
   statement about the end state.

6. **`errors[].code` has two namespaces, and the prefix is the discriminator.**
   `error.*` is a catalog key from a service rejection, which a client may
   render its own wording for. `request.*` is this surface's own, for the
   values a wire type cannot express — an ISO code, an IANA timezone — where
   there is no sentence in the catalog to point at. Both are stable within a
   major version. Inventing catalog keys for API-only refusals would have put
   sentences in the translation files that no screen ever shows.

7. **The §"users" 404 trap has a second answer, for when there is no list to
   scan.** `directory::audit_event` reports a missing row as a rejection, like
   `directory::find`. `users::get` works around it by scanning the list; the
   audit trail has no list — it is paged in SQL and grows forever — so the
   handler recognises the rejection by the field the service names and spells
   the 404. A test asserts against the real message that call site produces, so
   a rename there fails here rather than quietly reverting to a 422.

8. **A key that can mint a key is not an escalation.** `issue` refuses a scope
   the issuer does not hold, and the widest key a key can mint is therefore a
   copy of itself, issued by and acting as the same person. What it buys is
   rotation without a browser, and a deployment that needs a browser to replace
   a credential is a deployment whose credentials do not get replaced.

9. **Revoking is not deleting, so it is not `DELETE`.** The row stays, with its
   name, its scopes, its `last_used_at` and the reason somebody gave — because
   "which key was that, and who stopped it" is asked long after the key is
   dead. `POST /api-keys/{id}/revoke` is named after what it does. It is also
   deliberately *not* idempotent: revoking an already-revoked key answers 422
   rather than reporting success for an act that did nothing.

Two things a write must do, restated as rules because every new endpoint here
had to be reminded of them:

* **Read back; never echo.** The value returned is loaded from storage after
  the write, not the draft that came in. The two differ whenever the database
  declined something — a static role keeps its name however it was submitted —
  and echoing the draft shows a change that did not happen.
* **A `Submission::Rejected` goes out through `ServiceError::Rejected`.** It is
  not an error inside the application, but on a wire it is a 422, and it has to
  be *the same* 422 a service error produces or a client parses two shapes for
  one situation.

Still not here, and still for the reasons §10 gives: exchange rates, webhooks,
cursor pagination. And one new one — **there is no endpoint that sets somebody
else's password.** An account gets one exactly once, from the person who will
use it, by opening an invitation link. Anything else is an account takeover
with a friendly name, and no client needs it.


## Consequences

* A published spec cannot be walked back. Every endpoint added to `v1` is
  supported until `v2`, so the bar for adding one is "a client needs it", not "it
  was easy".
* Two adapters now sit over `phonix-services`, which makes the service layer's
  rule stricter rather than looser: **a use case that authorizes in its adapter
  is a bug**, because the other adapter will not do it.
* Wire types are duplicated by hand. That is the cost, it is deliberate, and the
  conversions are where a breaking change becomes visible instead of silent.
* `workspace_settings` grows a flag that is not a permission, and the difference
  between a licence and a grant becomes something the codebase distinguishes.

---

## What this ADR shipped

* `core.api_keys` and `workspace_settings.api_enabled` (migration 0020).
* `Pages.Administration.ApiKeys`, `.Create` and `.Revoke` in the permission
  tree, and an `api_key` audit kind - issuing and revoking a key are recorded;
  using one is not.
* `phonix_db::identity::api_key` (digests only), and
  `phonix_services::identity::api_key` (issue, list, revoke, authenticate).
* `phonix_server::api`: the nested `/api/v1` router, the bearer extractor, the
  problem body, the paging contract, the specification at
  `/api/v1/openapi.json`, Scalar at `/api/v1/docs`, and currencies.
* A rate-limit tier of its own.

**Since built** (2026-09-01): the whole administration area, listed in the
amendment above — permissions, the user and role writes, API keys, apps,
settings and both audit trails.

**Since built** (2026-08-28, checked 2026-08-29): `/admin/api-keys`, which
lists and revokes, carries the `api_enabled` control, and hands the token over
once at `/admin/api-keys/new`. Until it existed a key could only be created
from code, which was enough to test the surface and not enough to sell it.

**One amendment from [ADR 0003](0003-mobile-authentication.md)**, in force
since that record was accepted and built on 2026-08-29: §4 above
says `api_enabled` is checked "in the API router's authentication layer, before
the key is even looked up", and that stays true *of a key*. A request
authenticated by a **session** bearer - a person signed in on their own phone -
does not pass through that check at all. The flag is the licence for the
API-key surface, not for everything under `/api/v1`; ADR 0003 §3 argues why,
and the administration screen's wording has to say so.
