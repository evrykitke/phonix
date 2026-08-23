# phonix-cache — Redis, namespaced per tenant

![architecture](../../docs/architecture.svg)

Every key is written as `<key_prefix>:<tenant-slug>:<key>`. Callers reach the
cache through `Cache::for_tenant`, which owns the namespace, so a handler cannot
read another tenant's entry by passing a bare key — the same isolation rule as
the database-per-tenant split, enforced by construction rather than by care.

## Fail-open, and what that means

With `redis.fail_open = true` (the default) a cache failure degrades to a miss
and the request continues against Postgres. That is right for a cache and wrong
for a lock, a rate limiter or a session store: those need a failure to *stop*
the operation, not wave it through. Sessions accordingly live in Postgres, not
here.

## How it connects

```text
phonix-server ──> phonix-cache ──> Redis
phonix-web ─────> phonix-cache        (through AppState)

phonix-cache ──> phonix-core, phonix-config
```

Cross-cutting: available to every layer, dependent on none of them.
