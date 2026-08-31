//! Minting an API key.
//!
//! # Why this is a page and not a modal over the grid
//!
//! The same reason as [`user_invite`](super::user_invite): what comes back is a
//! **secret shown once and never again**, and it needs somewhere it can be
//! read, selected and copied without a table scrolling underneath it. A modal
//! would also have to decide what closing means while a key is on screen, and
//! every answer is wrong, because closing is how the key is lost.
//!
//! # The scopes offered are the ones the person holds
//!
//! Built from the viewer, so nobody is offered a permission the service would
//! refuse to put on a key. The service checks anyway - this is a narrowed list,
//! not a control - and the two agreeing is what stops the screen offering a
//! choice that always fails.

use leptos::prelude::*;
use leptos_meta::Title;
use phonix_core::identity::{ApiKeyDraft, ApiKeyIssued};

use crate::components::page::{Notice, PageHeader, Panel, Tone};
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::ui::clipboard;
use crate::ui::form::EntityForm;
use crate::ui::form::config::api_keys::{api_key_form, scope_choices};
use crate::ui::viewer::Viewer;

#[component]
pub fn api_key_new_page() -> impl IntoView {
    let viewer = Viewer::get();

    // What the last submission produced. Kept here rather than in the form,
    // because the form edits a draft and has nowhere to put a secret.
    let issued = RwSignal::new(None::<ApiKeyIssued>);
    let on_issued = Callback::new(move |result: ApiKeyIssued| issued.set(Some(result)));

    view! {
        <Title text=format!("{} | Phonix", l!("api_keys.new.title")) />

        <PageHeader
            title=l!("api_keys.new.title")
            subtitle=l!("api_keys.new.subtitle")
            icon=Icon::KeySquare
            back=("/admin/api-keys", l!("api_keys.title"))
        />

        <div class="grid gap-3 lg:grid-cols-2">
            // The scope choices come from the viewer, and a form built while
            // it is nobody would offer no scopes at all - so the panel waits
            // for it. The boundary is what makes that true: the viewer is
            // derived from a *blocking* resource, and reading one outside a
            // Suspense is read at two different moments on the two sides of
            // hydration - `None` on the server, already-serialised in the
            // browser. Here that decides whether a whole panel exists, so the
            // node counts would disagree and the mismatch is unrecoverable.
            //
            // No fallback: a panel outline that appears and is replaced is
            // worse than one that arrives a moment late, and a blocking
            // resource resolves before the first paint anyway.
            <Suspense fallback=|| ()>
                {move || {
                    viewer
                        .get()
                        .map(|user| {
                            view! {
                                <Panel title=l!("api_keys.new.panel")>
                                    <EntityForm
                                        config=api_key_form(scope_choices(&user), on_issued)
                                        value=ApiKeyDraft::blank()
                                    />
                                </Panel>
                            }
                        })
                }}
            </Suspense>

            {move || issued.get().map(|issued| view! { <Issued issued=issued /> })}
        </div>
    }
}

/// The key, the once.
#[component]
fn issued(issued: ApiKeyIssued) -> impl IntoView {
    let name = issued.key.name.clone();
    let secret = issued.secret.clone();
    let header = format!("Authorization: Bearer {}", issued.secret);
    let copied = RwSignal::new(false);

    let to_copy = issued.secret.clone();

    view! {
        <Panel title=l!("api_keys.issued.title")>
            <div class="space-y-3">
                <p class="text-sm text-content">{l!("api_keys.issued.created", name = name)}</p>

                // Warning rather than success: the thing worth saying here is
                // not "it worked", which is obvious, but that this is the only
                // time the key exists anywhere a person can read it.
                <Notice
                    message=Signal::derive(move || Some(l!("api_keys.issued.once")))
                    tone=Tone::Warning
                />

                // Selectable, wrapping, and in a monospace face: this gets
                // copied by hand more often than by button.
                <code class="block break-all rounded-control border border-edge bg-surface-sunken px-3 py-2 text-xs text-content">
                    {secret}
                </code>

                <button
                    type="button"
                    class="inline-flex h-7 items-center gap-1.5 rounded-control border border-edge px-2.5 text-xs text-content-muted hover:bg-surface-hover hover:text-content"
                    on:click=move |_| {
                        clipboard::copy(&to_copy);
                        copied.set(true);
                    }
                >
                    <Icon icon=Icon::Copy size=IconSize::Xs />
                    {move || {
                        if copied.get() {
                            l!("api_keys.issued.copied")
                        } else {
                            l!("api_keys.issued.copy")
                        }
                    }}
                </button>

                // What to do with it, so the first call somebody makes is the
                // right one rather than a guess at the header name.
                <div class="space-y-1.5">
                    <p class="text-xs font-medium text-content">
                        {l!("api_keys.issued.header")}
                    </p>
                    <code class="block break-all rounded-control border border-edge bg-surface-sunken px-3 py-2 text-2xs text-content-muted">
                        {header}
                    </code>
                </div>
            </div>
        </Panel>
    }
}
