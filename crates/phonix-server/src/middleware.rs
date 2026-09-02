//! Tenant resolution and request tracing.

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use phonix_config::{TenancyConfig, TenantStrategy};
use phonix_core::TenantSlug;
use phonix_web::state::AppState;
use tracing::field::Empty;

/// Resolve the tenant for a request and attach it as an extension.
///
/// Runs before every Leptos handler, so `phonix_web::state::tenant_from_request`
/// can read it back out inside a server function.
pub async fn resolve_tenant(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let tenancy = &state.config.tenancy;

    let Some(slug) = extract_slug(tenancy, request.headers(), request.uri()) else {
        // No tenant in the request. Static assets and the Leptos client bundle
        // legitimately have none, so the request continues without one and any
        // server function that needs a tenant fails on its own terms.
        tracing::debug!("request carries no tenant");
        return next.run(request).await;
    };

    match state.tenants.resolve(&slug).await {
        Ok(handle) => {
            let summary = handle.record.summary();

            // Record on the surrounding span so every log line for this
            // request is attributable to a tenant.
            tracing::Span::current().record("tenant", tracing::field::display(&summary.slug));

            request.extensions_mut().insert(summary);
            request.extensions_mut().insert(handle);
            next.run(request).await
        }

        Err(phonix_db::DbError::UnknownTenant(slug)) => {
            // Auto-provisioning is a development convenience; config validation
            // refuses to enable it in production.
            if tenancy.auto_provision {
                return provision_and_continue(state, slug, request, next).await;
            }

            tracing::info!(tenant = %slug, "unknown tenant");
            tenant_error(tenancy.unknown_tenant_status, "unknown tenant")
        }

        Err(phonix_db::DbError::TenantInactive { slug, status }) => {
            tracing::info!(tenant = %slug, status = %status, "tenant is not active");
            tenant_error(StatusCode::FORBIDDEN.as_u16(), "tenant is not active")
        }

        // The same 403, and deliberately not the same sentence. A workspace we
        // stopped and a workspace whose licence ran out need to start different
        // conversations - see ADR 0005 section 7. Both are still distinguishable
        // from the 404 an unknown host gets, which is what makes a suspended
        // customer tellable apart from a DNS mistake.
        Err(phonix_db::DbError::TenantUnlicensed {
            slug,
            standing,
            reason,
        }) => {
            tracing::info!(tenant = %slug, standing = %standing, "tenant is not licensed");
            tenant_error(StatusCode::FORBIDDEN.as_u16(), &reason)
        }

        Err(err) => {
            tracing::error!(error = %err, "tenant resolution failed");
            tenant_error(
                StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                "tenant lookup failed",
            )
        }
    }
}

/// Create a tenant on first sighting, then serve the request.
async fn provision_and_continue(
    state: AppState,
    slug: String,
    request: Request,
    next: Next,
) -> Response {
    let Ok(parsed) = TenantSlug::parse(&slug) else {
        return tenant_error(StatusCode::BAD_REQUEST.as_u16(), "invalid tenant");
    };

    tracing::warn!(tenant = %parsed, "auto-provisioning tenant (development only)");

    match phonix_db::tenancy::provision::provision_tenant(
        &state.catalog,
        &state.config.database,
        &parsed,
        parsed.as_str(),
        // Development only, and recorded as such: a workspace that appeared
        // because somebody typed a host name has no owner and no signup behind
        // it, and the catalog should say so.
        phonix_db::tenancy::catalog::TenantOrigin::AutoProvision,
        None,
        // An open licence, because this path only exists in development -
        // `validate::check` refuses `auto_provision` under production. A trial
        // here would mean a laptop's workspaces silently stopping serving a
        // month after they were conjured, which is a mystery, not a feature.
        phonix_db::tenancy::LicenceInput {
            state: phonix_core::LicenceState::Licensed,
            valid_from: chrono::Utc::now(),
            valid_until: None,
            note: Some("Auto-provisioned in development by an unrecognised Host header."),
            updated_by: "auto-provision",
        },
    )
    .await
    {
        Ok(_) => {
            // Drop any cached negative routing so the retry below sees the new
            // tenant rather than the state from a moment ago.
            state.tenants.invalidate(&parsed).await;

            let mut request = request;
            match state.tenants.resolve(&parsed).await {
                Ok(handle) => {
                    request.extensions_mut().insert(handle.record.summary());
                    request.extensions_mut().insert(handle);
                    next.run(request).await
                }
                Err(err) => {
                    tracing::error!(error = %err, "tenant unusable right after provisioning");
                    tenant_error(
                        StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                        "tenant unavailable",
                    )
                }
            }
        }
        Err(err) => {
            tracing::error!(error = %err, "auto-provisioning failed");
            tenant_error(
                StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                "could not provision tenant",
            )
        }
    }
}

fn tenant_error(status: u16, message: &str) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::NOT_FOUND);
    // Owned rather than `&'static str`: a licence refusal names which standing
    // it was, and that sentence is built from the row rather than written here.
    (status, message.to_owned()).into_response()
}

/// Pull the tenant slug out of the request, per the configured strategy.
fn extract_slug(
    tenancy: &TenancyConfig,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> Option<TenantSlug> {
    let candidate = match tenancy.strategy {
        TenantStrategy::Subdomain => subdomain_of(headers, &tenancy.base_domain),
        TenantStrategy::Header => headers
            .get(&tenancy.header_name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        TenantStrategy::Path => uri
            .path()
            .strip_prefix("/t/")
            .and_then(|rest| rest.split('/').next())
            .map(str::to_owned),
    };

    let candidate = candidate.filter(|value| !tenancy.is_reserved(value));

    // Fall back only when the request genuinely had no tenant, never when it
    // had one that happened to be reserved or malformed.
    match candidate {
        Some(value) => TenantSlug::parse(value).ok(),
        None => tenancy
            .default_tenant()
            .and_then(|value| TenantSlug::parse(value).ok()),
    }
}

/// The leading label of the Host header, when the host sits under `base_domain`.
///
/// Returns `None` for the bare base domain and for any host outside it, so a
/// request to `evil.com` cannot pick up a tenant.
fn subdomain_of(headers: &HeaderMap, base_domain: &str) -> Option<String> {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())?;

    // Strip the port: Host is "acme.phonix.local:3000" in development.
    let host = host.split(':').next()?.trim().to_ascii_lowercase();
    let base = base_domain.trim().to_ascii_lowercase();

    // `localhost` has no dots, so a bare-host development request falls through
    // to the configured default tenant rather than matching nothing forever.
    let rest = host.strip_suffix(&base)?.strip_suffix('.')?;

    if rest.is_empty() {
        return None;
    }

    // Only a single label counts: "a.b.phonix.local" is not tenant "a.b".
    if rest.contains('.') {
        return None;
    }

    Some(rest.to_owned())
}

/// Build the span that wraps each request.
pub fn make_request_span(request: &axum::http::Request<axum::body::Body>) -> tracing::Span {
    // `tenant` starts empty and is filled in by `resolve_tenant`.
    tracing::info_span!(
        "http",
        method = %request.method(),
        path = %request.uri().path(),
        tenant = Empty,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with_host(host: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::HOST,
            HeaderValue::from_str(host).unwrap(),
        );
        headers
    }

    #[test]
    fn extracts_the_leading_label() {
        let headers = headers_with_host("acme.phonix.local");
        assert_eq!(
            subdomain_of(&headers, "phonix.local").as_deref(),
            Some("acme")
        );
    }

    #[test]
    fn ignores_the_port() {
        let headers = headers_with_host("acme.phonix.local:3000");
        assert_eq!(
            subdomain_of(&headers, "phonix.local").as_deref(),
            Some("acme")
        );
    }

    #[test]
    fn bare_base_domain_has_no_tenant() {
        let headers = headers_with_host("phonix.local:3000");
        assert_eq!(subdomain_of(&headers, "phonix.local"), None);
    }

    #[test]
    fn rejects_hosts_outside_the_base_domain() {
        // The critical case: an attacker-controlled Host must not yield a tenant.
        for host in [
            "acme.evil.com",
            "evil.com",
            "acmephonix.local",
            "acme.phonix.local.evil.com",
        ] {
            let headers = headers_with_host(host);
            assert_eq!(
                subdomain_of(&headers, "phonix.local"),
                None,
                "{host} must not resolve to a tenant"
            );
        }
    }

    #[test]
    fn rejects_multi_label_subdomains() {
        let headers = headers_with_host("a.b.phonix.local");
        assert_eq!(subdomain_of(&headers, "phonix.local"), None);
    }

    #[test]
    fn is_case_insensitive() {
        let headers = headers_with_host("AcMe.Phonix.Local");
        assert_eq!(
            subdomain_of(&headers, "phonix.local").as_deref(),
            Some("acme")
        );
    }
}
