//! Drawing a `{from, to}` diff, wherever one is being read.
//!
//! One differ produces these, so one renderer draws them. `phonix-services`
//! computes a [`FieldChange`] the same way for the security trail and the
//! change trail - see `phonix_services::audit::diff` - and two screens drawing
//! that the same way is what makes "what changed" read identically whether it
//! is reached from an account or from a record.
//!
//! What is *not* here is the page around it: which panels, which heading, what
//! the sentence above the table says. Those differ between the two trails
//! because the two trails are answering different questions, and folding them
//! together would be a component with a flag on it.

use leptos::prelude::*;
use phonix_core::audit::{Change, ChangeKind, FieldChange};

use crate::components::page::{Badge, Tone};
use crate::icons::{Icon, IconSize};
use crate::l;

/// Every field that moved, in order.
///
/// Empty renders as nothing rather than as an empty list, so the caller can put
/// it on a page without first asking whether there is anything in it.
#[component]
pub fn change_list(changes: Vec<FieldChange>) -> impl IntoView {
    (!changes.is_empty()).then(|| {
        view! {
            <ul class="divide-y divide-edge">
                {changes
                    .into_iter()
                    .map(|change| view! { <ChangeRow change=change /> })
                    .collect::<Vec<_>>()}
            </ul>
        }
    })
}

/// One field of the diff: what it was, and what it is.
///
/// Side by side above `sm` and stacked below it, because a before and an after
/// squeezed into half a phone each are two columns of wrapped fragments.
#[component]
pub fn change_row(change: FieldChange) -> impl IntoView {
    let label = change.label();
    let kind = change.kind();

    view! {
        <li class="py-2.5 first:pt-0 last:pb-0">
            <div class="flex flex-wrap items-center gap-2">
                <span class="text-sm font-medium text-content">{label}</span>
                {match kind {
                    ChangeKind::Added => {
                        view! { <Badge label=l!("diff.added") tone=Tone::Success /> }
                    }
                    ChangeKind::Removed => {
                        view! { <Badge label=l!("diff.removed") tone=Tone::Danger /> }
                    }
                    ChangeKind::Modified => {
                        view! { <Badge label=l!("diff.changed") tone=Tone::Warning /> }
                    }
                }}
            </div>

            {match change.change {
                Change::Value { before, after } => {
                    view! {
                        <div class="mt-1.5 grid gap-1.5 sm:grid-cols-[1fr_auto_1fr] sm:items-center">
                            <Side
                                value=before
                                absent=l!("diff.not_set")
                                tint="bg-danger-subtle text-danger"
                            />
                            <span class="hidden text-content-subtle sm:inline" aria-hidden="true">
                                <Icon icon=Icon::ArrowRight size=IconSize::Xs />
                            </span>
                            <Side
                                value=after
                                absent=l!("diff.not_set")
                                tint="bg-surface-sunken text-success"
                            />
                        </div>
                    }
                        .into_any()
                }
                Change::Members { added, removed } => {
                    view! {
                        <div class="mt-1.5 space-y-1">
                            <Members items=added prefix="+" tone=Tone::Success />
                            <Members items=removed prefix="-" tone=Tone::Danger />
                        </div>
                    }
                        .into_any()
                }
            }}
        </li>
    }
}

/// One half of a before-and-after.
///
/// The two sides are tinted rather than badged: a value can be a sentence, and
/// a badge that wraps onto three lines stops looking like a badge.
#[component]
fn side(value: Option<String>, #[prop(into)] absent: String, tint: &'static str) -> impl IntoView {
    match value {
        Some(value) => view! {
            <span class=format!(
                "block break-words rounded-control px-2 py-1 text-sm {tint}",
            )>{value}</span>
        }
        .into_any(),
        // "Not set" is a fact, and it is not the same fact as an empty string.
        None => view! {
            <span class="block px-2 py-1 text-sm italic text-content-subtle">{absent}</span>
        }
        .into_any(),
    }
}

/// The members a collection gained or lost.
#[component]
fn members(items: Vec<String>, prefix: &'static str, tone: Tone) -> impl IntoView {
    (!items.is_empty()).then(|| {
        view! {
            <div class="flex flex-wrap items-center gap-1">
                {items
                    .into_iter()
                    .map(|item| view! { <Badge label=format!("{prefix} {item}") tone=tone /> })
                    .collect::<Vec<_>>()}
            </div>
        }
    })
}

/// One labelled fact in a `<dl>`.
///
/// Absent values are left out rather than drawn as a dash: an audit page with
/// six empty rows reads as if six things were missing.
#[component]
pub fn line(
    #[prop(into)] label: String,
    #[prop(into)] value: String,
    #[prop(optional)] mono: bool,
    /// Spans both columns, for a value that will not fit in one - a user agent.
    #[prop(optional)]
    wide: bool,
) -> impl IntoView {
    (!value.is_empty()).then(|| {
        let class = if mono {
            "break-all font-mono text-xs text-content"
        } else {
            "break-words text-content"
        };

        view! {
            <div class=if wide { "sm:col-span-2" } else { "" }>
                <dt class="text-2xs uppercase tracking-wide text-content-subtle">{label}</dt>
                <dd class=class>{value}</dd>
            </div>
        }
    })
}
