//! The half of the logo panel that only exists in a browser.
//!
//! Compiled into the wasm bundle and nowhere else. Everything here touches the
//! DOM or the network, so the server build gets the no-op stubs in the parent
//! module instead.
//!
//! # Nothing in this file may panic
//!
//! `wasm32-unknown-unknown` aborts rather than unwinds, so one panic stops
//! every handler, effect and pending request in the tab at once and the page
//! simply freezes - see `phonix_web::recovery`, which can only report the
//! freeze after the fact. That is why every JS call below is destructured with
//! `let ... else` or matched, and why a failure becomes a message on the screen
//! rather than a `?` that discards the reason.
//!
//! # Why there is no cropper here
//!
//! An avatar is cropped to a square because it is drawn in a circle beside a
//! name. A logo is not: wordmarks are wide, and forcing one into a square is
//! how a logo becomes unreadable. The file is sent as chosen, and the bucket's
//! pixel ceiling is what bounds it.

use leptos::prelude::*;
use uuid::Uuid;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};

use crate::components::page::Tone;
use crate::server_fns::file_fns::{set_organization_logo, upload_status, upload_url};

use super::BUCKET;

/// How long to wait between asking whether the job has finished.
const POLL_INTERVAL_MS: i32 = 400;

/// How many times to ask before giving up.
///
/// Twenty-five polls at 400ms is ten seconds, far longer than verifying a
/// half-megabyte PNG takes. A bound on a loop rather than a timeout anybody
/// should reach: if it is hit, the job is genuinely stuck and saying so beats
/// spinning forever.
const MAX_POLLS: u32 = 25;

/// A file has been chosen: upload it, wait for the verdict, attach it.
pub(super) fn pick_and_upload(
    ev: leptos::ev::Event,
    busy: RwSignal<bool>,
    message: RwSignal<Option<(String, Tone)>>,
    current: RwSignal<Option<Uuid>>,
) {
    let Some(input) = ev
        .target()
        .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
    else {
        return;
    };

    let Some(file) = input.files().and_then(|files| files.get(0)) else {
        return;
    };

    // Cleared so that choosing the same file twice fires `change` twice. Without
    // this, a failed upload cannot be retried by picking the same file again.
    input.set_value("");

    // Refused here as a courtesy, so somebody who picks a twelve-megabyte
    // photograph is told immediately rather than after uploading it. The bucket
    // enforces the same limit on the server, against the bytes that arrive.
    if let Some(bucket) = phonix_core::files::bucket(BUCKET)
        && file.size() > bucket.max_bytes as f64
    {
        message.set(Some((
            format!(
                "That image is {} and the limit is {}.",
                phonix_core::files::human_size(file.size() as u64),
                phonix_core::files::human_size(bucket.max_bytes),
            ),
            Tone::Danger,
        )));
        return;
    }

    let file_name = file.name();

    busy.set(true);
    message.set(None);

    spawn_local(async move {
        let outcome = attempt(&file, &file_name).await;
        busy.set(false);

        match outcome {
            Ok(file_id) => {
                current.set(Some(file_id));
                message.set(Some((
                    "The logo has been updated.".to_owned(),
                    Tone::Success,
                )));
            }
            Err(reason) => message.set(Some((reason, Tone::Danger))),
        }
    });
}

/// Upload, wait for the verifier, then point the profile at it.
///
/// Three steps rather than one because each can fail differently, and the
/// sentence shown has to say which: refused on the way in, refused by the
/// checker, or refused when attached.
async fn attempt(file: &web_sys::File, file_name: &str) -> Result<Uuid, String> {
    let id = post(file, file_name).await?;

    let summary = poll(id).await?;

    // The upload answered "received", not "accepted": deciding what the bytes
    // actually are is the verifier's job, and this is where its verdict lands.
    if summary.status != phonix_core::files::UploadStatus::Stored {
        // The rejection's own sentence: it says what was wrong with this file,
        // and a house phrase here would hide the one line worth reading.
        return Err(summary
            .rejection
            .map(|rejection| rejection.message())
            .unwrap_or_else(|| "That image was not accepted.".to_owned()));
    }

    set_organization_logo(id)
        .await
        .map(|stored| stored.id)
        .map_err(|err| err.to_string())
}

async fn post(file: &web_sys::File, file_name: &str) -> Result<Uuid, String> {
    let window = web_sys::window().ok_or_else(|| "No browser window.".to_owned())?;

    let form = web_sys::FormData::new().map_err(describe)?;
    // The field name the upload route looks for. The file name is a courtesy -
    // the server renames everything it stores - but it is what the files list
    // and the audit entry will show.
    form.append_with_blob_and_filename("file", file, file_name)
        .map_err(describe)?;

    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_body(&form);

    let request =
        web_sys::Request::new_with_str_and_init(&upload_url(BUCKET), &init).map_err(describe)?;

    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(describe)?
        .dyn_into::<web_sys::Response>()
        .map_err(|_| "The upload gave an answer this page could not read.".to_owned())?;

    let body = JsFuture::from(response.text().map_err(describe)?)
        .await
        .map_err(describe)?
        .as_string()
        .unwrap_or_default();

    if !response.ok() {
        // The route answers `{"error": "..."}` on every refusal, and that
        // sentence is written to be shown - it says what was wrong with the
        // file and nothing about how the server is arranged.
        return Err(error_from(&body)
            .unwrap_or_else(|| format!("The upload was refused ({}).", response.status())));
    }

    serde_json::from_str::<phonix_core::files::FileSummary>(&body)
        .map(|summary| summary.id)
        .map_err(|_| "The upload gave an answer this page could not read.".to_owned())
}

/// Pull the message out of the route's error body.
fn error_from(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_owned)
}

/// Ask about the upload until the job has decided.
async fn poll(id: Uuid) -> Result<phonix_core::files::FileSummary, String> {
    for _ in 0..MAX_POLLS {
        match upload_status(id).await {
            Ok(Some(summary)) if summary.status.is_terminal() => return Ok(summary),
            // Still queued or being checked. Normally taken once, if at all:
            // the verifier is dispatched the moment the bytes land.
            Ok(_) => sleep(POLL_INTERVAL_MS).await,
            Err(err) => return Err(err.to_string()),
        }
    }

    Err("That image is taking longer than expected to check. It may appear shortly.".to_owned())
}

/// Wait, without blocking anything.
async fn sleep(milliseconds: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        if let Some(window) = web_sys::window() {
            let _ = window
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, milliseconds);
        }
    });

    let _ = JsFuture::from(promise).await;
}

/// A JS exception as a sentence.
///
/// The browser's own message where there is one, because "NetworkError when
/// attempting to fetch resource" tells somebody more than "upload failed" does.
fn describe(err: JsValue) -> String {
    err.as_string()
        .or_else(|| {
            err.dyn_ref::<js_sys::Error>()
                .map(|error| error.message().into())
        })
        .unwrap_or_else(|| "The upload could not be sent.".to_owned())
}
