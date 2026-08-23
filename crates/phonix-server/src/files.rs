//! `POST /files/upload` and `GET /files/{id}/content`.
//!
//! Plain axum handlers rather than server functions, and for two different
//! reasons.
//!
//! **Uploading** is a multipart body that has to be *streamed*. A server
//! function takes its argument deserialised, which means the whole file in
//! memory before any code of ours runs - and the byte ceiling would then be
//! something we check after paying for it. Here the field is read chunk by
//! chunk straight into storage, and the ceiling is enforced by the writer as
//! the bytes arrive.
//!
//! **Downloading** is a response whose body is the file: a content type, a
//! disposition header and a stream. There is nothing for Leptos to render.
//!
//! # These routes carry their own limits
//!
//! The application's global body limit is 2 MiB, which is right for a form post
//! and would refuse every upload. So this router sets its own - the configured
//! `storage.max_upload_bytes` - and its own timeout, because 25 MB does not
//! arrive inside the clock a page render is judged by. `startup` merges it
//! *after* the global layers precisely so that those do not apply here.
//!
//! # What the download route will not do
//!
//! * It never serves the `Content-Type` the uploader declared - only the one
//!   detection decided on.
//! * It never renders anything inline except a picture that cannot carry a
//!   script, and it says so with `X-Content-Type-Options: nosniff` and a CSP
//!   that permits nothing at all.
//! * It never reads from quarantine. That is enforced in the service layer, so
//!   it is true of every caller and not only of this one.

use axum::Router;
use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use phonix_core::files::FileSummary;
use phonix_core::{Error as CoreError, TenantSummary};
use phonix_db::PgPool;
use phonix_services::files::{access, upload};
use phonix_services::{Caller, ServiceError};
use phonix_web::state::AppState;
use serde::Deserialize;
use tokio_util::io::ReaderStream;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use uuid::Uuid;

/// The file routes, with the limits uploads need rather than the ones pages do.
pub fn routes(state: &AppState) -> Router<AppState> {
    let storage = &state.config.storage;

    Router::new()
        .route("/files/upload", post(upload_file))
        .route("/files/{id}/content", get(download_file))
        // Axum's own extractor limit would otherwise refuse the body before
        // `RequestBodyLimitLayer` had a chance to allow it. Disabled here and
        // replaced by the configured ceiling on the next line - not removed.
        .layer(axum::extract::DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(
            // A coarse ceiling above every bucket's own. It exists because the
            // bucket is known from the query string but the *body* has to be
            // refusable before any of it is read, and startup refuses to boot
            // if this is smaller than the largest bucket allows.
            usize::try_from(storage.max_upload_bytes).unwrap_or(usize::MAX),
        ))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(storage.upload_timeout_secs),
        ))
}

#[derive(Debug, Deserialize)]
pub struct UploadQuery {
    /// Which bucket, and therefore which policy. In the query string rather
    /// than in the body because the ceiling and the permission have to be
    /// settled *before* the first byte is accepted - a bucket arriving as a
    /// multipart field would be read after the file in half the browsers.
    bucket: String,
}

/// Accept a file into quarantine and queue the work on it.
///
/// Answers `202 Accepted` with the row as it stands - normally `received`. It
/// deliberately does not wait for verification: that is a job, the caller is
/// not waiting on it, and a request that blocked on it would make the timeout a
/// function of how busy the worker is.
async fn upload_file(
    State(state): State<AppState>,
    tenant: Option<axum::Extension<TenantSummary>>,
    headers: HeaderMap,
    Query(query): Query<UploadQuery>,
    multipart: Multipart,
) -> Response {
    let (tenant, pool, caller) = match authenticate(&state, tenant, &headers).await {
        Ok(who) => who,
        Err(response) => return response,
    };

    // Before a single byte: may this caller write into this bucket, and how
    // many bytes may they write? Nothing here touches the disk, so a refusal
    // costs nothing and leaves nothing to undo.
    let ticket = match upload::authorise_upload(&caller, &tenant.slug, &query.bucket) {
        Ok(ticket) => ticket,
        Err(err) => return service_error(err),
    };

    match accept_bytes(&state, &pool, &caller, ticket, multipart).await {
        Ok(summary) => (StatusCode::ACCEPTED, axum::Json(summary)).into_response(),
        Err(response) => response,
    }
}

/// Stream the file field into quarantine and record it.
///
/// Split out so the `?` on each step is readable; every early return here has
/// already removed whatever bytes it had written.
async fn accept_bytes(
    state: &AppState,
    pool: &PgPool,
    caller: &Caller,
    ticket: upload::UploadTicket,
    mut multipart: Multipart,
) -> Result<FileSummary, Response> {
    let mut field = loop {
        match multipart.next_field().await {
            // Only the part actually called `file`. A body carrying three other
            // fields first is ordinary; a body carrying none is a bad request.
            Ok(Some(field)) if field.name() == Some("file") => break field,
            Ok(Some(_)) => continue,
            Ok(None) => {
                return Err(bad_request("The upload carried no file."));
            }
            Err(err) => {
                return Err(bad_request(&format!("Malformed upload: {err}")));
            }
        }
    };

    // Read before the body is consumed - both borrow the field.
    let original_name = field
        .file_name()
        .map(str::to_owned)
        .unwrap_or_else(|| "file".to_owned());
    let declared_content_type = field.content_type().map(str::to_owned);

    let mut writer = state
        .storage
        .begin(&ticket.quarantine_key, ticket.limit)
        .await
        .map_err(|err| service_error(ServiceError::Storage(err)))?;

    loop {
        match field.chunk().await {
            Ok(Some(chunk)) => {
                if let Err(err) = writer.write(&chunk).await {
                    writer.abort().await;
                    // A body that runs past the bucket's limit stops here,
                    // mid-stream, rather than after the whole thing has been
                    // written and measured.
                    return Err(match err {
                        phonix_storage::StorageError::LimitExceeded { limit } => too_large(limit),
                        other => service_error(ServiceError::Storage(other)),
                    });
                }
            }
            Ok(None) => break,
            Err(err) => {
                writer.abort().await;
                return Err(bad_request(&format!("The upload did not finish: {err}")));
            }
        }
    }

    let stat = match writer.finish().await {
        Ok(stat) => stat,
        Err(err) => return Err(service_error(ServiceError::Storage(err))),
    };

    let row = match upload::record_upload(
        pool,
        &ticket,
        &original_name,
        declared_content_type.as_deref(),
        &stat,
        caller,
    )
    .await
    {
        Ok(row) => row,
        Err(err) => {
            // The bytes are down and nothing points at them. The sweeper would
            // find them eventually; removing them now means it never has to.
            upload::discard(state.storage.as_ref(), &ticket.quarantine_key).await;
            return Err(service_error(err));
        }
    };

    dispatch(state, pool, &tenant_slug_of(&ticket), row.id);

    Ok(row.to_summary(None))
}

/// Start verifying this upload now, in the background.
///
/// The fast path. The periodic sweep would pick the row up anyway - that is
/// what makes losing this race harmless - but waiting a poll interval to find
/// out whether a profile picture was accepted is a long time to look at a
/// spinner.
///
/// Spawned rather than awaited: the caller is not waiting on the answer, and
/// blocking the response on the work would make the request's timeout depend on
/// how busy the worker is.
fn dispatch(state: &AppState, pool: &PgPool, tenant: &phonix_core::TenantSlug, file_id: Uuid) {
    let state = state.clone();
    let pool = pool.clone();
    let tenant = tenant.clone();

    tokio::spawn(async move {
        crate::jobs::verify_one(&state, &pool, &tenant, file_id).await;
    });
}

fn tenant_slug_of(ticket: &upload::UploadTicket) -> phonix_core::TenantSlug {
    // The key's first segment is a slug that was validated to build it, so this
    // cannot fail; `unwrap_or_else` rather than an unwrap because a panic in a
    // request handler is a 500 for somebody who did nothing wrong.
    phonix_core::TenantSlug::parse(ticket.quarantine_key.tenant())
        .unwrap_or_else(|_| unreachable_slug())
}

/// A slug that parses, for a branch that cannot be reached.
fn unreachable_slug() -> phonix_core::TenantSlug {
    #[allow(clippy::expect_used)]
    phonix_core::TenantSlug::parse("unknown").expect("a literal that is a valid slug")
}

/// Send a stored file's bytes.
async fn download_file(
    State(state): State<AppState>,
    tenant: Option<axum::Extension<TenantSummary>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let (tenant, pool, caller) = match authenticate(&state, tenant, &headers).await {
        Ok(who) => who,
        Err(response) => return response,
    };

    let opened = access::open_for_download(&pool, state.files(), &tenant.slug, &caller, id).await;

    let (row, reader) = match opened {
        Ok(opened) => opened,
        Err(err) => return service_error(err),
    };

    // The detected type, never the declared one. Serving what the uploader
    // claimed would hand a browser an `image/png` header on a file that is not
    // one - which is the whole attack the detection exists to stop.
    let content_type = row
        .content_type
        .as_deref()
        .and_then(|mime| phonix_core::files::by_mime(mime));

    let inline = content_type.is_some_and(|file_type| file_type.is_inline_safe());
    let mime = content_type.map_or("application/octet-stream", |file_type| file_type.mime);

    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, row.byte_size)
        .header(
            header::CONTENT_DISPOSITION,
            disposition(inline, &row.original_name),
        )
        // Without this a browser is free to decide for itself what the bytes
        // are, and to run whatever it concludes. It is the header that makes
        // the detection above binding rather than advisory.
        .header("x-content-type-options", "nosniff")
        // Nothing this file references may load, and nothing in it may run,
        // even if it is opened directly. Belt and braces over `is_inline_safe`.
        .header(
            "content-security-policy",
            "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; sandbox",
        )
        // A stored file is per-workspace and often per-person, so it must never
        // land in a shared cache. Immutable because the bytes at an id never
        // change: a replacement is a new upload with a new id.
        .header(header::CACHE_CONTROL, "private, max-age=604800, immutable")
        .body(Body::from_stream(ReaderStream::new(reader)))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());

    // The digest, so a client that cares can check what it received.
    if let Some(checksum) = row.checksum_sha256.as_deref()
        && let Ok(value) = HeaderValue::from_str(checksum)
    {
        response.headers_mut().insert("x-content-sha256", value);
    }

    response
}

/// A `Content-Disposition` header that survives a name in any language.
///
/// Two forms, and both are needed. The bare `filename=` is ASCII only, which
/// covers old clients; `filename*=` carries the real name percent-encoded per
/// RFC 5987, which every current browser prefers. A client that understands
/// only the first still gets something it can save.
///
/// The name reaching here has already been through
/// `phonix_core::files::sanitize_file_name`, so it holds no quotes, no
/// separators and no control characters - which is what makes quoting it safe
/// rather than merely conventional.
fn disposition(inline: bool, name: &str) -> String {
    let kind = if inline { "inline" } else { "attachment" };

    // Two characters are ASCII-graphic and still cannot appear: a quote ends
    // the quoted-string, and a backslash escapes whatever follows it. Names
    // reaching here have already been through `sanitize_file_name`, which
    // removes both - this is the guarantee restated where the quoting happens,
    // so the header is safe to build from any string rather than only from one
    // that has been through the right function first.
    let ascii: String = name
        .chars()
        .map(|ch| match ch {
            '"' | '\\' => '_',
            ch if ch.is_ascii_graphic() || ch == ' ' => ch,
            _ => '_',
        })
        .collect();

    format!(
        "{kind}; filename=\"{ascii}\"; filename*=UTF-8''{}",
        percent_encode(name)
    )
}

/// Percent-encode everything that is not an RFC 5987 `attr-char`.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());

    for byte in text.bytes() {
        let safe = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
            );

        if safe {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

/// Who is asking, in a handler with no Leptos context to ask through.
///
/// `phonix_web::state::current_caller` reads the request through
/// `leptos_axum::extract`, which only works inside a Leptos handler. These are
/// plain axum routes, so the same three steps are done directly: the tenant
/// from the middleware's extension, the pool from the registry, and the session
/// from the cookie.
async fn authenticate(
    state: &AppState,
    tenant: Option<axum::Extension<TenantSummary>>,
    headers: &HeaderMap,
) -> Result<(TenantSummary, PgPool, Caller), Response> {
    let Some(axum::Extension(tenant)) = tenant else {
        return Err(error_response(
            StatusCode::NOT_FOUND,
            "This address does not belong to a workspace.",
        ));
    };

    let handle =
        state.tenants.resolve(&tenant.slug).await.map_err(|_| {
            error_response(StatusCode::SERVICE_UNAVAILABLE, "Workspace unavailable.")
        })?;

    let token = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| {
            let name = state
                .config
                .security
                .session
                .cookie_name_for(tenant.slug.as_str());
            phonix_web::server::cookie::read(raw, &name)
        })
        .map(secrecy::SecretString::from);

    let Some(token) = token else {
        return Err(error_response(StatusCode::UNAUTHORIZED, "Not signed in."));
    };

    let authenticated =
        phonix_services::authenticate_session(&handle.pool, &token, &state.config.security)
            .await
            .map_err(service_error)?;

    let Some(auth_user) = authenticated else {
        return Err(error_response(StatusCode::UNAUTHORIZED, "Not signed in."));
    };

    Ok((tenant, handle.pool.clone(), Caller::user(auth_user)))
}

/// Turn a service failure into a response, without leaking what went wrong.
///
/// The conversion to `CoreError` is where a SQL fragment, a path or a mount
/// point is dropped and replaced by a label - see `phonix_services::error`. All
/// this adds is the status code and the JSON shape the browser reads.
fn service_error(err: ServiceError) -> Response {
    let core = CoreError::from(err);
    let status =
        StatusCode::from_u16(core.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    error_response(status, &core.to_string())
}

fn too_large(limit: u64) -> Response {
    error_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        &format!(
            "That file is larger than the {} allowed here.",
            phonix_core::files::human_size(limit)
        ),
    )
}

fn bad_request(message: &str) -> Response {
    error_response(StatusCode::BAD_REQUEST, message)
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(serde_json::json!({ "error": message }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disposition_carries_the_name_twice_and_quotes_it_safely() {
        let header = disposition(false, "Q3 report.pdf");

        assert!(header.starts_with("attachment; "));
        assert!(header.contains("filename=\"Q3 report.pdf\""));
        assert!(header.contains("filename*=UTF-8''Q3%20report.pdf"));
    }

    #[test]
    fn a_name_in_another_script_survives_in_the_encoded_form() {
        let header = disposition(false, "報告書.pdf");

        // The ASCII form cannot carry it, so it degrades to underscores rather
        // than to something that would split the header.
        assert!(header.contains("filename=\"___.pdf\""));
        // The real name is still there, and every non-ASCII byte is encoded.
        assert!(header.contains("filename*=UTF-8''%E5%A0%B1%E5%91%8A%E6%9B%B8.pdf"));
    }

    #[test]
    fn only_a_picture_that_cannot_run_is_offered_inline() {
        assert!(disposition(true, "photo.png").starts_with("inline;"));
        assert!(disposition(false, "invoice.pdf").starts_with("attachment;"));
    }

    #[test]
    fn nothing_in_a_name_can_break_out_of_the_header() {
        // Belt and braces: names reaching here are already sanitised, so this
        // is checking that the encoding would hold even if one were not.
        let hostile = "a\"b\r\nX-Evil: 1";
        let header = disposition(false, hostile);

        assert!(!header.contains('\r'));
        assert!(!header.contains('\n'));
        assert_eq!(header.matches('"').count(), 2);
    }

    #[test]
    fn percent_encoding_leaves_the_unreserved_characters_alone() {
        assert_eq!(
            percent_encode("report-2026_final.pdf"),
            "report-2026_final.pdf"
        );
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("100%"), "100%25");
    }
}
