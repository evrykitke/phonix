# phonix-web — the presentation layer

The Leptos application: pages, components, and the server functions that connect
them to the application layer.

![architecture](../../docs/architecture.svg)

Compiled twice:

- with `--features ssr` for the server binary (`phonix-server`)
- with `--features hydrate` for the WebAssembly bundle

Code that must not reach the browser lives behind `#[cfg(feature = "ssr")]` or
inside a `#[server]` function body, which the macro strips from the client build.

## What lives here

| Module        | Responsibility                                             |
| ------------- | ---------------------------------------------------------- |
| `app`         | The router and the document shell                          |
| `pages/`      | One file per route                                         |
| `components/` | Shared UI                                                  |
| `server_fns/` | One file per scope — `tenant_fns`, `onboarding_fns`, `auth_fns` |
| `server/`     | ssr-only plumbing: the session cookie                      |
| `state`       | `AppState`: config, catalog, tenant registry, cache, broker |

## Server functions are thin

A server function parses its input, calls one use case, and maps the result to
something serialisable. It does not open transactions, hash anything or decide
who may do what — that is `phonix-services`' job, and duplicating it here would
mean two implementations of the same rule with one of them eventually wrong.

```rust
#[server]
pub async fn change_password(current: String, new_password: String) -> Result<.., ServerFnError> {
    let (pool, caller) = ..;                       // resolve from the session
    let outcome = phonix_services::identity::password::change_own_password(..).await?;
    Ok(outcome)                                     // per-field rejections survive
}
```

## The session cookie lives here

`server/cookie.rs` builds and parses it, because two callers need it and they
cannot share an axum extractor: `phonix-server` sets cookies from a handler,
while the server functions set them through `ResponseOptions`. Both end up
writing a `Set-Cookie` string, so that string is built once — in the lower of
the two crates, since `phonix-server` depends on this one.

The cookie is **host-only**: no `Domain` attribute, ever. A cookie scoped to the
parent domain would be sent to every workspace subdomain, so one workspace's
server would receive another's token on every request. That is also why signup
cannot simply set a cookie — the wizard runs on the bare domain and the new
workspace lives on a subdomain, which the one-time handoff token exists to cross.

## How it connects

```text
phonix-server ──> phonix-web ──> phonix-services ──> phonix-db
                       └───────> phonix-core        (shared with the browser)
```
