//! The profile picture: showing one, and choosing one.
//!
//! # The crop happens in the browser, and that is not a shortcut
//!
//! A phone camera produces a 4032 x 3024 photograph of four megabytes. An
//! avatar is displayed at forty pixels square. Sending the original would mean
//! four megabytes over the wire to render sixteen hundred pixels, storing it at
//! full size for ever, and asking a server to decode it - which is the one
//! operation an image bomb is aimed at.
//!
//! So the picture is cropped and scaled where it already is. What leaves the
//! browser is a square PNG of [`OUTPUT_PIXELS`] pixels a side, typically a
//! couple of hundred kilobytes, and the person choosing it decides which part
//! of their face is in it rather than being centre-cropped by a rule.
//!
//! # And the server still checks everything
//!
//! None of the above is a control. A hand-written request skips every line of
//! this file, so the `avatars` bucket has its own byte limit, its own pixel
//! limit, and a category list that refuses anything that is not a picture -
//! including SVG, which is a picture that can carry a script. See
//! `phonix_core::files::bucket`. What the cropper buys is that the limits are
//! never reached by an honest user, not that they cannot be exceeded.
//!
//! # Why the flow has three steps rather than one
//!
//! ```text
//!   POST /files/upload?bucket=avatars   the bytes; answers with an id
//!   upload_status(id)                   polled until the job has decided
//!   set_profile_picture(id)             only then is it anybody's picture
//! ```
//!
//! An upload is a job - see `phonix_services::files` - so the response to the
//! POST says "received", not "accepted". Attaching a file to an account before
//! anything had looked at it would put an unverified picture beside somebody's
//! name, which is precisely what the quarantine exists to prevent.

use leptos::prelude::*;
use uuid::Uuid;

use crate::components::page::{Notice, Panel, Tone};
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::server_fns::file_fns::{content_url, my_profile_picture, remove_profile_picture};

/// The side of the square that is uploaded.
///
/// Enough for a retina display at every size the application shows an avatar,
/// and small enough that the PNG lands well inside the bucket's two megabytes.
pub const OUTPUT_PIXELS: u32 = 512;

/// The side of the crop window, in CSS pixels.
const VIEWPORT: f64 = 256.0;

/// How far in the zoom slider goes. One is "the picture exactly covers the
/// window", which is the least it may ever be - below that there would be a
/// gap, and a gap in a square crop is a transparent corner.
const MAX_ZOOM: f64 = 3.0;

/// Somebody's picture, or their initials.
///
/// The fallback is not a placeholder image: initials on a coloured circle are
/// recognisable at every size, load instantly, and say something true about
/// whose account it is.
#[component]
pub fn avatar(
    /// Shown when there is no picture.
    #[prop(into)]
    initials: Signal<String>,
    #[prop(into)] file_id: Signal<Option<Uuid>>,
    /// A Tailwind size class - `size-10` in a list, `size-20` on a profile.
    #[prop(into, default = "size-10".to_owned())]
    size: String,
) -> impl IntoView {
    let shape = format!("{size} shrink-0 rounded-full");

    view! {
        {move || match file_id.get() {
            Some(id) => {
                view! {
                    <img
                        src=content_url(id)
                        alt=""
                        // `object-cover` matters: the file is square, but a
                        // picture set before this cropper existed may not be,
                        // and a stretched face is worse than a cropped one.
                        class=format!("{shape} object-cover bg-surface-sunken")
                        // The picture is decorative beside a name that is
                        // already text, so it is hidden from a screen reader
                        // rather than announced as "image".
                        aria-hidden="true"
                    />
                }
                    .into_any()
            }
            None => {
                view! {
                    <span
                        class=format!(
                            "{shape} grid place-items-center bg-brand text-sm font-semibold text-on-brand",
                        )
                        aria-hidden="true"
                    >
                        {initials.get()}
                    </span>
                }
                    .into_any()
            }
        }}
    }
}

/// Read the caller's stored picture once, into a signal the page can share.
///
/// A signal rather than a resource at the point of use, because two panels show
/// the same picture and saving a new one has to update both. Refetching would
/// tell the page what it already knows and leave a visible flicker doing it.
pub fn stored_picture() -> RwSignal<Option<Uuid>> {
    let current = RwSignal::new(None::<Uuid>);
    let stored = Resource::new(|| (), |()| my_profile_picture());
    let seeded = RwSignal::new(false);

    Effect::new(move |_| {
        // Seeded once. Without the guard, a later resource refetch would
        // overwrite a picture the person had just chosen with the one the
        // server knew about when the page loaded.
        if let Some(Ok(id)) = stored.get()
            && !seeded.get()
        {
            current.set(id);
            seeded.set(true);
        }
    });

    current
}

/// The panel on the account page: what your picture is, and changing it.
#[component]
pub fn profile_picture(
    #[prop(into)] initials: Signal<String>,
    /// The stored picture, shared with whatever else on the page shows it.
    current: RwSignal<Option<Uuid>>,
) -> impl IntoView {
    // The picked file, while it is being cropped. `None` means the panel is
    // showing the current picture rather than a chooser.
    let picked = RwSignal::new(None::<PickedImage>);
    let offset = RwSignal::new((0.0_f64, 0.0_f64));
    let zoom = RwSignal::new(1.0_f64);
    let dragging = RwSignal::new(None::<(f64, f64)>);

    let busy = RwSignal::new(false);
    let message = RwSignal::new(None::<(String, Tone)>);

    let reset = move || {
        picked.set(None);
        offset.set((0.0, 0.0));
        zoom.set(1.0);
        dragging.set(None);
    };

    view! {
        <Panel title=l!("avatar.title") description=l!("avatar.description")>
            <div class="space-y-3">
                {move || {
                    message
                        .get()
                        .map(|(text, tone)| {
                            view! {
                                <Notice
                                    message=Signal::derive(move || Some(text.clone()))
                                    tone=tone
                                />
                            }
                        })
                }}

                <Show
                    when=move || picked.get().is_some()
                    fallback=move || {
                        view! {
                            <CurrentPicture
                                initials=initials
                                current=current
                                busy=busy
                                message=message
                                picked=picked
                                offset=offset
                                zoom=zoom
                            />
                        }
                    }
                >
                    <Cropper
                        picked=picked
                        offset=offset
                        zoom=zoom
                        dragging=dragging
                        busy=busy
                        message=message
                        current=current
                        on_cancel=Callback::new(move |()| reset())
                    />
                </Show>
            </div>
        </Panel>
    }
}

/// What the panel shows when nothing is being cropped.
#[component]
fn current_picture(
    #[prop(into)] initials: Signal<String>,
    current: RwSignal<Option<Uuid>>,
    busy: RwSignal<bool>,
    message: RwSignal<Option<(String, Tone)>>,
    picked: RwSignal<Option<PickedImage>>,
    offset: RwSignal<(f64, f64)>,
    zoom: RwSignal<f64>,
) -> impl IntoView {
    let remove = Action::new(move |(): &()| async move { remove_profile_picture().await });

    Effect::new(move |_| match remove.value().get() {
        Some(Ok(())) => {
            current.set(None);
            message.set(Some((
                "Your picture has been removed.".to_owned(),
                Tone::Success,
            )));
        }
        Some(Err(err)) => message.set(Some((err.to_string(), Tone::Danger))),
        None => {}
    });

    view! {
        <div class="flex flex-wrap items-center gap-4">
            <Avatar initials=initials file_id=current size="size-20" />

            <div class="space-y-2">
                <label
                    class="inline-flex h-8 cursor-pointer items-center gap-1.5 rounded-control border border-edge px-3 text-sm text-content-muted hover:bg-surface-hover hover:text-content"
                    for="avatar-file"
                >
                    <Icon icon=Icon::Upload size=IconSize::Xs />
                    {move || {
                        if current.get().is_some() {
                            l!("avatar.change")
                        } else {
                            l!("avatar.choose")
                        }
                    }}
                </label>

                // The input itself is hidden rather than styled: a file input
                // cannot be made to look like anything, and a `<label for>` is
                // the one way to trigger it that keyboards and screen readers
                // both already understand.
                <input
                    id="avatar-file"
                    type="file"
                    class="sr-only"
                    // The list the bucket would actually take, generated from
                    // the same table the server checks against. A courtesy, not
                    // a control - see the module docs.
                    accept=avatar_accept()
                    on:change=move |ev| {
                        message.set(None);
                        offset.set((0.0, 0.0));
                        zoom.set(1.0);
                        pick_file(ev, picked, message);
                    }
                />

                <p class="text-xs text-content-subtle">{size_hint()}</p>

                <Show when=move || current.get().is_some()>
                    <button
                        type="button"
                        class="text-xs text-danger hover:underline disabled:opacity-60"
                        disabled=move || busy.get() || remove.pending().get()
                        on:click=move |_| {
                            remove.dispatch(());
                        }
                    >
                        {l!("avatar.remove")}
                    </button>
                </Show>
            </div>
        </div>
    }
}

/// The crop window, the zoom, and the two buttons.
#[component]
fn cropper(
    picked: RwSignal<Option<PickedImage>>,
    offset: RwSignal<(f64, f64)>,
    zoom: RwSignal<f64>,
    dragging: RwSignal<Option<(f64, f64)>>,
    busy: RwSignal<bool>,
    message: RwSignal<Option<(String, Tone)>>,
    current: RwSignal<Option<Uuid>>,
    on_cancel: Callback<()>,
) -> impl IntoView {
    let source = move || picked.get().map(|image| image.object_url);

    // The transform that puts the picture where the person dragged it. Read
    // from signals, so a drag is a style change rather than a redraw - and it
    // is the *same* arithmetic the canvas uses when saving, which is what makes
    // what you see what you get.
    let style = move || {
        let (x, y) = offset.get();
        format!(
            "position:absolute;left:0;top:0;transform:translate({x}px,{y}px) scale({});\
             transform-origin:top left;max-width:none;",
            scale_of(picked.get().as_ref(), zoom.get())
        )
    };

    view! {
        <div class="space-y-3">
            <div class="flex flex-wrap items-start gap-4">
                <div
                    class="relative shrink-0 cursor-grab overflow-hidden rounded-full border border-edge bg-surface-sunken active:cursor-grabbing"
                    style=format!("width:{VIEWPORT}px;height:{VIEWPORT}px;touch-action:none;")
                    on:pointerdown=move |ev| begin_drag(&ev, offset, dragging)
                    on:pointermove=move |ev| continue_drag(&ev, picked, offset, zoom, dragging)
                    on:pointerup=move |_| dragging.set(None)
                    on:pointerleave=move |_| dragging.set(None)
                >
                    {move || {
                        source()
                            .map(|url| {
                                view! { <img src=url alt="" style=style() draggable="false" /> }
                            })
                    }}

                    // The circle the picture will be seen through. Drawn over
                    // the image and ignoring the pointer, so dragging still
                    // reaches the image underneath it.
                    <div
                        class="pointer-events-none absolute inset-0 rounded-full ring-2 ring-brand/60"
                        aria-hidden="true"
                    ></div>
                </div>

                <div class="min-w-0 flex-1 space-y-3">
                    <div class="space-y-1">
                        <label
                            class="block text-sm font-medium text-content"
                            for="avatar-zoom"
                        >
                            {l!("avatar.zoom")}
                        </label>
                        <input
                            id="avatar-zoom"
                            type="range"
                            min="1"
                            max=MAX_ZOOM.to_string()
                            step="0.01"
                            class="w-full max-w-measure"
                            prop:value=move || zoom.get().to_string()
                            on:input=move |ev| {
                                if let Ok(value) = event_target_value(&ev).parse::<f64>() {
                                    zoom.set(value.clamp(1.0, MAX_ZOOM));
                                    // Zooming out can leave the picture no
                                    // longer covering the window, so the offset
                                    // is pulled back inside on every change
                                    // rather than only while dragging.
                                    clamp_offset(picked, offset, zoom);
                                }
                            }
                        />
                    </div>

                    <p class="text-sm text-content-muted">
                        {l!("avatar.drag_hint")}
                    </p>

                    <div class="flex gap-2">
                        <button
                            type="button"
                            class="inline-flex h-8 items-center gap-1.5 rounded-control bg-brand px-3 text-sm font-medium text-on-brand hover:bg-brand-hover disabled:cursor-not-allowed disabled:opacity-60"
                            disabled=move || busy.get()
                            on:click=move |_| {
                                save_picture(picked, offset, zoom, busy, message, current)
                            }
                        >
                            {move || {
                                busy.get()
                                    .then(|| {
                                        view! {
                                            <span
                                                class="size-3.5 animate-spin rounded-full border border-current border-t-transparent"
                                                aria-hidden="true"
                                            ></span>
                                        }
                                    })
                            }}
                            {move || if busy.get() { l!("common.saving") } else { l!("avatar.save") }}
                        </button>

                        <button
                            type="button"
                            class="inline-flex h-8 items-center rounded-control border border-edge px-3 text-sm text-content-muted hover:bg-surface-hover hover:text-content disabled:opacity-60"
                            disabled=move || busy.get()
                            on:click=move |_| on_cancel.run(())
                        >
                            {l!("common.cancel")}
                        </button>
                    </div>
                </div>
            </div>
        </div>
    }
}

/// The `accept` attribute for the avatar bucket.
fn avatar_accept() -> String {
    phonix_core::files::bucket("avatars")
        .map(|bucket| bucket.accept_attribute())
        .unwrap_or_default()
}

/// The sentence under the chooser, built from the bucket's real limits.
fn size_hint() -> String {
    let Some(bucket) = phonix_core::files::bucket("avatars") else {
        return String::new();
    };

    format!(
        "Up to {}. Your picture is cropped to {OUTPUT_PIXELS} pixels square before it is sent.",
        phonix_core::files::human_size(bucket.max_bytes)
    )
}

/// A picture waiting to be cropped.
#[derive(Debug, Clone, PartialEq)]
pub struct PickedImage {
    /// A `blob:` URL, so the file is displayed without being read into memory
    /// twice.
    pub object_url: String,
    pub natural_width: f64,
    pub natural_height: f64,
}

/// The factor the picture is drawn at.
///
/// `zoom` of one means "exactly covering the window", whatever shape the
/// original is - so the shorter side is what the base is computed from. This is
/// the one piece of arithmetic shared by the preview and the canvas, and
/// sharing it is what makes the saved crop match the one on screen.
fn scale_of(picked: Option<&PickedImage>, zoom: f64) -> f64 {
    let Some(image) = picked else {
        return zoom;
    };

    let shortest = image.natural_width.min(image.natural_height);
    if shortest <= 0.0 {
        return zoom;
    }

    (VIEWPORT / shortest) * zoom
}

/// Keep the picture covering the window.
///
/// Without this a drag can pull the photograph off the edge and leave a
/// transparent wedge, which becomes a transparent wedge in the saved PNG.
fn clamp_offset(
    picked: RwSignal<Option<PickedImage>>,
    offset: RwSignal<(f64, f64)>,
    zoom: RwSignal<f64>,
) {
    let image = picked.get();
    let scale = scale_of(image.as_ref(), zoom.get());
    let Some(image) = image else {
        return;
    };

    let width = image.natural_width * scale;
    let height = image.natural_height * scale;

    let (x, y) = offset.get();
    // The minimum is negative: the far edge of the picture must not come inside
    // the window. `min(0.0)` on the maximum handles a picture that is somehow
    // narrower than the window, where the only legal offset is zero.
    let clamped_x = x.clamp((VIEWPORT - width).min(0.0), 0.0);
    let clamped_y = y.clamp((VIEWPORT - height).min(0.0), 0.0);

    if (clamped_x, clamped_y) != (x, y) {
        offset.set((clamped_x, clamped_y));
    }
}

// ---------------------------------------------------------------------------
// The browser half
//
// Everything below touches the DOM, the canvas or the network, none of which
// exists during server-side rendering. The functions are declared twice: once
// for the wasm build, and once as a no-op so the same markup compiles on the
// server. The alternative - `cfg` inside every event handler - would put the
// split in twenty places instead of one.
// ---------------------------------------------------------------------------

#[cfg(feature = "hydrate")]
mod browser;

#[cfg(feature = "hydrate")]
use browser::{begin_drag, continue_drag, pick_file, save_picture};

#[cfg(not(feature = "hydrate"))]
fn pick_file(
    _ev: leptos::ev::Event,
    _picked: RwSignal<Option<PickedImage>>,
    _message: RwSignal<Option<(String, Tone)>>,
) {
}

#[cfg(not(feature = "hydrate"))]
fn begin_drag(
    _ev: &leptos::ev::PointerEvent,
    _offset: RwSignal<(f64, f64)>,
    _dragging: RwSignal<Option<(f64, f64)>>,
) {
}

#[cfg(not(feature = "hydrate"))]
fn continue_drag(
    _ev: &leptos::ev::PointerEvent,
    _picked: RwSignal<Option<PickedImage>>,
    _offset: RwSignal<(f64, f64)>,
    _zoom: RwSignal<f64>,
    _dragging: RwSignal<Option<(f64, f64)>>,
) {
}

#[cfg(not(feature = "hydrate"))]
fn save_picture(
    _picked: RwSignal<Option<PickedImage>>,
    _offset: RwSignal<(f64, f64)>,
    _zoom: RwSignal<f64>,
    _busy: RwSignal<bool>,
    _message: RwSignal<Option<(String, Tone)>>,
    _current: RwSignal<Option<Uuid>>,
) {
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landscape() -> PickedImage {
        PickedImage {
            object_url: "blob:x".into(),
            natural_width: 4032.0,
            natural_height: 3024.0,
        }
    }

    #[test]
    fn a_zoom_of_one_exactly_covers_the_window() {
        // Whatever shape the original is, the shorter side is what has to reach
        // across - otherwise the crop circle has a gap in it, and a gap becomes
        // a transparent wedge in the saved picture.
        let scale = scale_of(Some(&landscape()), 1.0);

        assert!((landscape().natural_height * scale - VIEWPORT).abs() < 0.001);
        assert!(landscape().natural_width * scale >= VIEWPORT);
    }

    #[test]
    fn zooming_multiplies_the_covering_scale() {
        let base = scale_of(Some(&landscape()), 1.0);
        assert!((scale_of(Some(&landscape()), 2.0) - base * 2.0).abs() < 0.001);
    }

    #[test]
    fn a_picture_with_no_size_yet_does_not_divide_by_zero() {
        // An image element read before it has loaded reports zero, and a scale
        // of infinity would be a NaN transform and a blank circle.
        let empty = PickedImage {
            object_url: "blob:x".into(),
            natural_width: 0.0,
            natural_height: 0.0,
        };

        assert!(scale_of(Some(&empty), 1.5).is_finite());
        assert!(scale_of(None, 1.5).is_finite());
    }

    #[test]
    fn the_hint_quotes_the_limit_the_server_actually_applies() {
        // Read from the bucket table rather than written out, so raising the
        // limit does not leave a sentence saying the old one.
        let hint = size_hint();

        assert!(hint.contains("2.0 MB"), "{hint}");
        assert!(hint.contains("512"), "{hint}");
    }

    #[test]
    fn the_accept_list_offers_nothing_the_bucket_would_refuse() {
        let accept = avatar_accept();

        assert!(accept.contains(".png"));
        assert!(accept.contains(".jpg"));
        // An SVG is a picture that can carry a script, and the avatar bucket
        // takes none - so it must not be on the list either.
        assert!(!accept.contains(".svg"), "{accept}");
        assert!(!accept.contains(".pdf"), "{accept}");
    }
}
