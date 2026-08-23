//! The half of the cropper that only exists in a browser.
//!
//! Compiled into the wasm bundle and nowhere else. Everything here touches the
//! DOM, a canvas or the network, so the server build gets the no-op stubs in
//! the parent module instead.
//!
//! # Nothing in this file may panic
//!
//! `wasm32-unknown-unknown` aborts rather than unwinds, so one panic stops
//! every handler, effect and pending request in the tab at once and the page
//! simply freezes - see `phonix_web::recovery`, which can only report the
//! freeze after the fact. That is why every JS call below is destructured with
//! `let ... else` or matched, and why a failure becomes a message on the screen
//! rather than a `?` that discards the reason.

use leptos::prelude::*;
use uuid::Uuid;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};

use crate::components::page::Tone;
use crate::server_fns::file_fns::{set_profile_picture, upload_status, upload_url};

use super::{OUTPUT_PIXELS, PickedImage, VIEWPORT, clamp_offset, scale_of};

/// How long to wait between asking whether the job has finished.
const POLL_INTERVAL_MS: i32 = 400;

/// How many times to ask before giving up.
///
/// Twenty-five polls at 400ms is ten seconds, which is far longer than
/// verifying a half-megabyte PNG takes. It is a bound on a loop rather than a
/// timeout anybody should reach: if it is hit, the job is genuinely stuck and
/// saying so beats spinning forever.
const MAX_POLLS: u32 = 25;

/// A file has been chosen: show it, and measure it.
pub(super) fn pick_file(
    ev: leptos::ev::Event,
    picked: RwSignal<Option<PickedImage>>,
    message: RwSignal<Option<(String, Tone)>>,
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

    // Refused here as a courtesy, so somebody who picks a twelve-megabyte
    // photograph is told immediately rather than after uploading it. The bucket
    // enforces the same limit on the server, against the bytes that actually
    // arrive.
    if let Some(bucket) = phonix_core::files::bucket("avatars")
        && file.size() > bucket.max_bytes as f64
    {
        message.set(Some((
            format!(
                "That picture is {} and the limit is {}.",
                phonix_core::files::human_size(file.size() as u64),
                phonix_core::files::human_size(bucket.max_bytes)
            ),
            Tone::Danger,
        )));
        return;
    }

    let Ok(object_url) = web_sys::Url::create_object_url_with_blob(&file) else {
        message.set(Some((
            "This browser would not open that file.".to_owned(),
            Tone::Danger,
        )));
        return;
    };

    // Clearing the input's value is what makes choosing the *same* file twice
    // fire a change event the second time. Without it, cancelling a crop and
    // picking the same photograph again does nothing at all.
    input.set_value("");

    spawn_local(async move {
        match measure(&object_url).await {
            Some((natural_width, natural_height)) => {
                picked.set(Some(PickedImage {
                    object_url,
                    natural_width,
                    natural_height,
                }));
            }
            None => {
                revoke(&object_url);
                message.set(Some((
                    "That file could not be read as a picture.".to_owned(),
                    Tone::Danger,
                )));
            }
        }
    });
}

/// Load an image and report its natural size.
///
/// `decode()` rather than an `onload` handler: it answers with a promise, which
/// is a thing this async function can await, and it resolves only once the
/// picture is actually decodable - so a file that is named `.png` and is not
/// one fails here rather than by rendering a broken image.
async fn measure(object_url: &str) -> Option<(f64, f64)> {
    let image = web_sys::HtmlImageElement::new().ok()?;
    image.set_src(object_url);

    JsFuture::from(image.decode()).await.ok()?;

    let width = f64::from(image.natural_width());
    let height = f64::from(image.natural_height());

    (width > 0.0 && height > 0.0).then_some((width, height))
}

/// A drag has started: remember where the pointer was, relative to the picture.
pub(super) fn begin_drag(
    ev: &leptos::ev::PointerEvent,
    offset: RwSignal<(f64, f64)>,
    dragging: RwSignal<Option<(f64, f64)>>,
) {
    let (x, y) = offset.get();
    // The grab point, not the pointer position: storing the difference means
    // the picture does not jump to centre itself under the cursor on the first
    // pixel of movement.
    dragging.set(Some((
        f64::from(ev.client_x()) - x,
        f64::from(ev.client_y()) - y,
    )));

    // Keeps the drag alive when the pointer leaves the circle, which it will:
    // the whole point is to pull the picture past the edge.
    if let Some(target) = ev
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
    {
        let _ = target.set_pointer_capture(ev.pointer_id());
    }
}

/// The pointer has moved: put the picture where it now belongs.
pub(super) fn continue_drag(
    ev: &leptos::ev::PointerEvent,
    picked: RwSignal<Option<PickedImage>>,
    offset: RwSignal<(f64, f64)>,
    zoom: RwSignal<f64>,
    dragging: RwSignal<Option<(f64, f64)>>,
) {
    let Some((grab_x, grab_y)) = dragging.get() else {
        return;
    };

    offset.set((
        f64::from(ev.client_x()) - grab_x,
        f64::from(ev.client_y()) - grab_y,
    ));

    // Every move, so the picture cannot be dragged off the edge even for one
    // frame. Clamping only on release would let go of a photograph with a
    // transparent wedge showing.
    clamp_offset(picked, offset, zoom);
}

/// Crop, upload, wait for the verdict, and attach it to the account.
pub(super) fn save_picture(
    picked: RwSignal<Option<PickedImage>>,
    offset: RwSignal<(f64, f64)>,
    zoom: RwSignal<f64>,
    busy: RwSignal<bool>,
    message: RwSignal<Option<(String, Tone)>>,
    current: RwSignal<Option<Uuid>>,
) {
    let Some(image) = picked.get() else {
        return;
    };

    busy.set(true);
    message.set(None);

    let scale = scale_of(Some(&image), zoom.get());
    let (offset_x, offset_y) = offset.get();

    spawn_local(async move {
        let outcome = attempt(&image, scale, offset_x, offset_y).await;
        busy.set(false);

        match outcome {
            Ok(file_id) => {
                revoke(&image.object_url);
                current.set(Some(file_id));
                picked.set(None);
                message.set(Some((
                    "Your picture has been updated.".to_owned(),
                    Tone::Success,
                )));
            }
            Err(reason) => message.set(Some((reason, Tone::Danger))),
        }
    });
}

/// The whole save, with every step's failure named.
///
/// Returns the id of the stored file. An `Err` carries a sentence that is safe
/// and useful to show - the browser's own words where they help, and ours where
/// they do not.
async fn attempt(
    image: &PickedImage,
    scale: f64,
    offset_x: f64,
    offset_y: f64,
) -> Result<Uuid, String> {
    let blob = crop_to_blob(image, scale, offset_x, offset_y)
        .await
        .ok_or_else(|| "This browser could not prepare the picture.".to_owned())?;

    let received = post(&blob).await?;

    // The upload answered "received", not "accepted": deciding what the bytes
    // are is a job, and it runs after the request returned. So the id is
    // followed until the job has an answer.
    let settled = poll(received).await?;

    if let Some(rejection) = settled.rejection {
        // The server's own sentence about the file, which says what is wrong
        // with *this* picture rather than something generic.
        return Err(rejection.message());
    }

    if !settled.status.is_available() {
        return Err("That picture could not be stored. Please try again.".to_owned());
    }

    set_profile_picture(settled.id)
        .await
        .map(|summary| summary.id)
        .map_err(|err| err.to_string())
}

/// Draw the chosen crop into a canvas and get its bytes.
///
/// The arithmetic is the inverse of the preview's transform, which is why
/// `scale_of` is shared rather than reimplemented: the picture is drawn at
/// `offset` scaled by `scale`, so the part of it inside the window starts at
/// `-offset / scale` and is `VIEWPORT / scale` across.
async fn crop_to_blob(
    image: &PickedImage,
    scale: f64,
    offset_x: f64,
    offset_y: f64,
) -> Option<web_sys::Blob> {
    if scale <= 0.0 || !scale.is_finite() {
        return None;
    }

    let document = web_sys::window()?.document()?;
    let canvas = document
        .create_element("canvas")
        .ok()?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .ok()?;

    canvas.set_width(OUTPUT_PIXELS);
    canvas.set_height(OUTPUT_PIXELS);

    let context = canvas
        .get_context("2d")
        .ok()??
        .dyn_into::<web_sys::CanvasRenderingContext2d>()
        .ok()?;

    // Decoded a second time rather than reaching into the preview's DOM node:
    // this function has no reason to know how the preview is built, and an
    // element it did not create is one that may have been replaced mid-render.
    let element = web_sys::HtmlImageElement::new().ok()?;
    element.set_src(&image.object_url);
    JsFuture::from(element.decode()).await.ok()?;

    let source_x = -offset_x / scale;
    let source_y = -offset_y / scale;
    let source_side = VIEWPORT / scale;

    context
        .draw_image_with_html_image_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
            &element,
            source_x,
            source_y,
            source_side,
            source_side,
            0.0,
            0.0,
            f64::from(OUTPUT_PIXELS),
            f64::from(OUTPUT_PIXELS),
        )
        .ok()?;

    // PNG rather than JPEG: lossless, no quality argument to get wrong, and at
    // 512 pixels square it is a few hundred kilobytes - well inside the two
    // megabytes the bucket allows.
    let data_url = canvas.to_data_url_with_type("image/png").ok()?;
    let (_, encoded) = data_url.split_once(',')?;

    to_blob(encoded)
}

/// Turn the base64 tail of a data URL into a blob.
///
/// `atob` is used rather than a base64 crate because it is already in every
/// browser and this is the only place that needs it. It answers with a string
/// whose characters are the bytes, which is what the map below is unpacking.
fn to_blob(encoded: &str) -> Option<web_sys::Blob> {
    let binary = web_sys::window()?.atob(encoded).ok()?;

    let bytes: Vec<u8> = binary.chars().map(|ch| ch as u8).collect();
    let array = js_sys::Uint8Array::new_with_length(u32::try_from(bytes.len()).ok()?);
    array.copy_from(&bytes);

    let parts = js_sys::Array::new();
    parts.push(&array);

    let options = web_sys::BlobPropertyBag::new();
    options.set_type("image/png");

    web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options).ok()
}

/// POST the blob and read back the row it created.
async fn post(blob: &web_sys::Blob) -> Result<Uuid, String> {
    let window = web_sys::window().ok_or_else(|| "No browser window.".to_owned())?;

    let form = web_sys::FormData::new().map_err(describe)?;
    // The field name the upload route looks for, and a name for the file. The
    // name is a courtesy - the server renames everything it stores - but it is
    // what a log line and the files list will show.
    form.append_with_blob_and_filename("file", blob, "profile-picture.png")
        .map_err(describe)?;

    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_body(&form);

    let request =
        web_sys::Request::new_with_str_and_init(&upload_url("avatars"), &init).map_err(describe)?;

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
            // Still queued or being checked. Normally this branch is taken
            // once, if at all: the verifier is dispatched the moment the bytes
            // land, so the answer is usually ready before the first poll.
            Ok(_) => sleep(POLL_INTERVAL_MS).await,
            Err(err) => return Err(err.to_string()),
        }
    }

    Err("That picture is taking longer than expected to check. It may appear shortly.".to_owned())
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

/// Release a blob URL.
///
/// Each one pins the whole file in memory until it is revoked, and somebody
/// trying five photographs would otherwise be holding all five.
fn revoke(object_url: &str) {
    let _ = web_sys::Url::revoke_object_url(object_url);
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
