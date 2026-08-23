//! One audit entry, opened.
//!
//! # The screen answers whichever question the entry can answer
//!
//! Two kinds of row end up in one table, and reading them the same way serves
//! neither:
//!
//! * **Something changed.** A policy was relaxed, a role's grants were edited.
//!   The question is *what*, and the answer is a diff - field, before, after.
//! * **Something happened.** A sign-in, a lockout, a recovery code spent.
//!   There is no before, so a diff would be an empty table with a heading. The
//!   answer is a sentence.
//!
//! Which one an entry gets is decided by what was recorded, not by the event
//! name: `AuditEventDetail::is_entity_change` is true when the stored detail
//! carried a before and an after.
//!
//! The narration is shown either way. On a change it is the one-line "who,
//! when, from where" that a table of fields cannot say.
//!
//! # The diff rows here are the old ones
//!
//! Record changes are no longer written to this trail; they go to
//! [`entity_change`](super::entity_change), which names the record as well as
//! the person. The branch stays because the rows written before that split are
//! still here, and they still deserve to be read as diffs rather than as a
//! sentence saying that something changed.

use leptos::prelude::*;
use leptos_meta::Title;
use phonix_core::identity::AuditEventDetail;

use crate::components::diff::{ChangeList, Line};
use crate::components::page::{Badge, Notice, PageHeader, Panel, Tone};
use crate::i18n::Locale;
use crate::icons::Icon;
use crate::l;
use crate::server_fns::admin_fns::audit_event;

#[component]
pub fn audit_event_page() -> impl IntoView {
    let params = leptos_router::hooks::use_params_map();

    // Keyed on the parameter rather than read once: the route can change under
    // this component when somebody opens a second entry from a link.
    let entry = Resource::new(
        move || params.with(|params| params.get("id").unwrap_or_default()),
        |raw| async move {
            let Ok(id) = raw.parse::<i64>() else {
                return Err(ServerFnError::new("That is not an audit entry."));
            };

            audit_event(id).await
        },
    );

    view! {
        // "Phonix" is the product's name, not a word.
        <Title text=format!("{} | Phonix", l!("audit.entry.title")) />

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
                                    title=l!("audit.entry.title")
                                    icon=Icon::ScrollText
                                    back=("/admin/audit-logs", l!("audit.entry.back"))
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
fn entry(entry: AuditEventDetail) -> impl IntoView {
    // One catalog for both: the heading and the sentence under it are the
    // same event said twice, and they must not disagree.
    let catalog = Locale::get().shared();
    let title = catalog.render(&entry.event.label());
    let narration = entry.event.narration(&catalog);
    let failed = !entry.event.succeeded;
    let notable = entry.event.is_notable();
    let name = entry.event.event.clone();
    let occurred_at = entry
        .event
        .occurred_at
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string();
    let email = entry.event.email.clone().unwrap_or_default();
    let ip = entry.event.ip.clone().unwrap_or_default();
    let user_agent = entry.user_agent.clone().unwrap_or_default();
    let has_changes = entry.is_entity_change();
    let changes = entry.changes;
    let facts = entry.facts;

    view! {
        <PageHeader
            title=title
            icon=Icon::ScrollText
            back=("/admin/audit-logs", l!("audit.entry.back"))
        >
            {if failed {
                view! {
                    <Badge
                        label=l!("audit.outcome.failed")
                        tone=Tone::Danger
                        icon=Icon::CircleAlert
                    />
                }
                    .into_any()
            } else if notable {
                view! { <Badge label=l!("audit.outcome.notable") tone=Tone::Warning /> }.into_any()
            } else {
                view! { <Badge label=l!("audit.outcome.succeeded") tone=Tone::Success /> }
                    .into_any()
            }}
        </PageHeader>

        <div class="space-y-4">
            <Panel title=l!("audit.what_happened")>
                <p class="text-sm leading-relaxed text-content">{narration}</p>

                <dl class="mt-3 grid gap-x-6 gap-y-2 border-t border-edge pt-3 text-sm sm:grid-cols-2">
                    <Line label=l!("field.event") value=name mono=true />
                    <Line label=l!("field.recorded") value=occurred_at />
                    <Line label=l!("field.account") value=email />
                    <Line label=l!("field.address") value=ip mono=true />
                    <Line label=l!("field.browser") value=user_agent mono=true wide=true />

                    {facts
                        .into_iter()
                        .map(|fact| view! { <Line label=fact.label value=fact.value /> })
                        .collect::<Vec<_>>()}
                </dl>
            </Panel>

            {has_changes
                .then(|| {
                    view! {
                        <Panel
                            title=l!("audit.diff.updated.heading")
                            description=l!("audit.diff.updated.description")
                        >
                            <ChangeList changes=changes />
                        </Panel>
                    }
                })}
        </div>
    }
}
