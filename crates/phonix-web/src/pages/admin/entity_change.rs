//! One change, opened.
//!
//! The change trail's counterpart to [`audit_event`](super::audit_event), and
//! deliberately simpler. That screen has to decide whether an entry is a diff
//! or a sentence, because the security trail carries both. Every row here is a
//! change by construction, so this always draws the same two panels: what
//! happened, and what moved.
//!
//! # The link back to the record
//!
//! A change names its record, which is the whole reason `entity_events` exists,
//! so this page can offer to go and look at it. `EntityChange::href` returns
//! `None` after a deletion - there is nothing at that address any more, and a
//! link to it is a link to an error page - so the button is absent rather than
//! broken.
//!
//! The diff itself is [`crate::components::diff`], shared with the security
//! trail: one differ produces it, and one renderer draws it.

use leptos::prelude::*;
use leptos_meta::Title;
use phonix_core::audit::{EntityAction, EntityChangeDetail};

use crate::components::diff::{ChangeList, Line};
use crate::components::page::{Badge, Notice, PageHeader, Panel, Tone};
use crate::components::user_link::UserLink;
use crate::i18n::Locale;
use crate::icons::Icon;
use crate::l;
use crate::server_fns::admin_fns::entity_change;

/// Where "back" goes, and where the trail this came from lives.
const TRAIL: &str = "/admin/audit-logs?tab=changes";

#[component]
pub fn entity_change_page() -> impl IntoView {
    let params = leptos_router::hooks::use_params_map();

    // Keyed on the parameter rather than read once: the route can change under
    // this component when somebody opens a second change from a link.
    let entry = Resource::new(
        move || params.with(|params| params.get("id").unwrap_or_default()),
        |raw| async move {
            let Ok(id) = raw.parse::<i64>() else {
                return Err(ServerFnError::new("That is not a change."));
            };

            entity_change(id).await
        },
    );

    view! {
        // "Phonix" is the product's name, not a word.
        <Title text=format!("{} | Phonix", l!("audit.change.title")) />

        <Suspense fallback=|| {
            view! { <p class="text-sm text-content-subtle">{l!("common.loading")}</p> }
        }>
            {move || Suspend::new(async move {
                match entry.await {
                    Ok(entry) => view! { <Entry entry=entry /> }.into_any(),
                    Err(err) => {
                        view! {
                            <>
                                <PageHeader
                                    title=l!("audit.change.title")
                                    icon=Icon::ClipboardList
                                    back=(TRAIL, l!("audit.change.back"))
                                />
                                <Notice
                                    message=Signal::derive(move || Some(err.to_string()))
                                    tone=Tone::Danger
                                />
                            </>
                        }
                            .into_any()
                    }
                }
            })}
        </Suspense>
    }
}

#[component]
fn entry(entry: EntityChangeDetail) -> impl IntoView {
    let change = entry.change;

    // One catalog, read once: every one of these composes a sentence out of
    // the same words, and fetching it four times would be four chances to get
    // a different one.
    let catalog = Locale::get().shared();

    let title = change.headline(&catalog);
    let narration = change.narration(&catalog);
    let action = change.action;
    let kind = change.kind_label(&catalog);
    let record = change.record(&catalog);
    let href = change.href();
    let occurred_at = change
        .occurred_at
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string();
    let actor = change.actor_email.clone();
    let actor_id = change.actor_id;
    let ip = change.ip.clone().unwrap_or_default();
    let user_agent = entry.user_agent.clone().unwrap_or_default();
    let changes = entry.changes;
    let facts = entry.facts;

    // A creation and a deletion are diffs against nothing, so "what changed" is
    // the wrong heading for either: on a creation every field is new, and on a
    // deletion every field is what was lost.
    let (heading, description) = match action {
        EntityAction::Created => (
            l!("audit.diff.created.heading"),
            l!("audit.diff.created.description"),
        ),
        EntityAction::Updated => (
            l!("audit.diff.updated.heading"),
            l!("audit.diff.updated.description"),
        ),
        EntityAction::Deleted => (
            l!("audit.diff.deleted.heading"),
            l!("audit.diff.deleted.description"),
        ),
    };

    view! {
        <PageHeader
            title=title
            icon=Icon::ClipboardList
            back=(TRAIL, l!("audit.change.back"))
        >
            <Badge label=crate::i18n::t(&action.name()) tone=tone(action) />
        </PageHeader>

        <div class="space-y-4">
            <Panel title=l!("audit.what_happened")>
                <p class="text-sm leading-relaxed text-content">{narration}</p>

                <dl class="mt-3 grid gap-x-6 gap-y-2 border-t border-edge pt-3 text-sm sm:grid-cols-2">
                    <Line label=l!("field.record") value=record />
                    <Line label=l!("field.kind") value=kind />
                    <Line label=l!("field.recorded") value=occurred_at />
                    <div>
                        <dt class="text-2xs uppercase tracking-wide text-content-subtle">
                            {l!("field.by")}
                        </dt>
                        <dd class="break-words text-content">
                            <UserLink
                                email=actor
                                user_id=actor_id
                                absent=l!("audit.actor.system")
                            />
                        </dd>
                    </div>
                    <Line label=l!("field.address") value=ip mono=true />
                    <Line label=l!("field.browser") value=user_agent mono=true wide=true />

                    {facts
                        .into_iter()
                        .map(|fact| view! { <Line label=fact.label value=fact.value /> })
                        .collect::<Vec<_>>()}
                </dl>

                // Absent after a deletion, because there is nothing left there.
                {href
                    .map(|href| {
                        view! {
                            <a
                                href=href
                                class="mt-3 inline-flex items-center gap-1.5 text-sm font-medium text-brand hover:underline"
                            >
                                {l!("audit.open_record")}
                                <Icon icon=Icon::ArrowRight size=crate::icons::IconSize::Xs />
                            </a>
                        }
                    })}
            </Panel>

            {(!changes.is_empty())
                .then(|| {
                    view! {
                        <Panel title=heading description=description>
                            <ChangeList changes=changes />
                        </Panel>
                    }
                })}
        </div>
    }
}

/// How loudly to draw it. The same choice the grid makes, for the same reason:
/// a deletion is the one that cannot be checked by opening the record.
const fn tone(action: EntityAction) -> Tone {
    match action {
        EntityAction::Created => Tone::Success,
        EntityAction::Updated => Tone::Neutral,
        EntityAction::Deleted => Tone::Danger,
    }
}
