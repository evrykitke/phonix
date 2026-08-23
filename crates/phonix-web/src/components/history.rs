//! "What has happened to this" - the history section on a detail page.
//!
//! The other half of what `entity_events` was built for. The
//! [trail](crate::ui::table::config::changes) answers "what happened on
//! Tuesday"; this answers "what has ever happened to *this record*", which is
//! the question somebody is already looking at the record to ask.
//!
//! # Why it is a section and not a screen
//!
//! It shows the most recent entries - `phonix_services::audit::HISTORY_LIMIT`
//! of them - and no more. Somebody who needs the twenty-first is asking a
//! question the full trail answers better, and a detail page that can scroll
//! for a year is a detail page whose actual content is off the top of the
//! screen.
//!
//! # Why it is quiet when there is nothing to show
//!
//! A record created before this trail existed has no history, and so does one
//! created a second ago. Neither is worth a panel saying so: the section
//! renders nothing at all rather than an empty state, because an empty state
//! here would appear on every record in a workspace that has just upgraded.
//!
//! It is also quiet when the viewer may not read the trail. The server refuses
//! either way - `phonix_services::audit::history` requires the same permission
//! the trail does - so this is the cosmetic half of that, and the panel simply
//! is not drawn rather than drawn around an error.

use leptos::prelude::*;
use phonix_core::audit::{EntityAction, EntityChange, EntityKind};
use phonix_core::i18n::datetime;
use phonix_core::permissions;

use crate::components::page::{Badge, Panel, Tone};
use crate::components::user_link::UserLink;
use crate::i18n::{Locale, t};
use crate::l;
use crate::server_fns::admin_fns::entity_history;
use crate::ui::viewer::Viewer;

/// Everything that has happened to one record.
///
/// The kind is the declared [`EntityKind`], so a call site cannot invent a name
/// the server will refuse; the id is whatever that kind records - a UUID for a
/// role, its own name for a singleton.
#[component]
pub fn record_history(
    kind: EntityKind,
    /// `None` while the page is still working out which record it is on. The
    /// section waits rather than fetching the history of an empty id.
    #[prop(into)]
    id: Signal<Option<String>>,
) -> impl IntoView {
    let viewer = Viewer::get();

    let may_read = move || {
        viewer
            .get()
            .is_some_and(|user| user.can(permissions::AUDIT_LOGS))
    };

    // The kind's name is read into the source rather than the fetcher: the
    // fetcher outlives this function, and a `&'static str` off a value it does
    // not own is a lifetime it cannot promise.
    let name = kind.name;

    let history = Resource::new(
        move || (may_read(), name.to_owned(), id.get()),
        |(may_read, name, id)| async move {
            match (may_read, id) {
                (true, Some(id)) if !id.is_empty() => {
                    entity_history(name, id).await.unwrap_or_default()
                }
                // Not an error: the section is simply not this viewer's to see,
                // or the page does not know its record yet.
                _ => Vec::new(),
            }
        },
    );

    view! {
        // No fallback: a heading that appears and then vanishes when the
        // record turns out to have no history is worse than one that arrives
        // a moment late.
        <Suspense fallback=|| ()>
            {move || Suspend::new(async move {
                let entries = history.await;

                (!entries.is_empty())
                    .then(|| {
                        view! {
                            <Panel
                                title=l!("history.title")
                                description=l!("history.description")
                            >
                                <ol class="divide-y divide-edge">
                                    {entries
                                        .into_iter()
                                        .map(|entry| view! { <Entry entry=entry /> })
                                        .collect::<Vec<_>>()}
                                </ol>
                            </Panel>
                        }
                    })
            })}
        </Suspense>
    }
}

/// One line of the history: who, what, when, and a way into the detail.
#[component]
fn entry(entry: EntityChange) -> impl IntoView {
    let href = format!("/admin/changes/{}", entry.id);
    let action = entry.action;
    let who = entry.actor_email.clone();
    let who_id = entry.actor_id;
    let when = datetime::moment_short(Locale::get().catalog(), entry.occurred_at);
    let summary = entry.summary.clone();

    view! {
        <li class="py-2.5 first:pt-0 last:pb-0">
            // The row is not one link any more: the info button sits inside it,
            // and a button inside an anchor is markup a browser is entitled to
            // render either way. So the anchor wraps only the part that is
            // actually a link to the change.
            <div class="flex flex-wrap items-center gap-2 text-sm">
                <Badge label=t(&action.name()) tone=tone(action) />
                <UserLink email=who user_id=who_id absent=l!("audit.actor.system") />
                <span class="text-xs text-content-muted">{when}</span>
                <a
                    href=href
                    class="ml-auto text-xs font-medium text-brand hover:underline"
                >
                    {l!("common.open")}
                </a>
            </div>

            {summary
                .map(|summary| {
                    view! { <p class="mt-0.5 text-xs text-content-subtle">{summary}</p> }
                })}
        </li>
    }
}

/// The verb as a badge reads it. The same words the change page uses - a
/// history that calls it something else reads as a different kind of entry.
const fn tone(action: EntityAction) -> Tone {
    match action {
        EntityAction::Created => Tone::Success,
        EntityAction::Updated => Tone::Neutral,
        EntityAction::Deleted => Tone::Danger,
    }
}
