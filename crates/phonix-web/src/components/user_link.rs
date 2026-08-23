//! An email address, with the person behind it one click away.
//!
//! # The problem
//!
//! Trails record addresses, and they are right to: a row has to still name
//! somebody after their account is deleted, so it stores the address rather
//! than joining to it. But `k.ndlovu@example.com` is not a person. Working out
//! who it is means leaving the screen, going to the directory, searching, and
//! coming back having lost your place - for a question that takes one line to
//! answer.
//!
//! [`UserLink`] draws the address with an info button beside it. The card is
//! fetched when the button is pressed and not before: sending a profile with
//! every row would be a query per row to answer a question about one of them,
//! and most rows are never asked about.
//!
//! # Why a dialog and not a hover card
//!
//! Every place this is used is inside a table that scrolls horizontally, and a
//! popover anchored inside `overflow-x: auto` is either clipped by it or
//! escapes it and widens the page. On a phone, anything wider than the viewport
//! inflates the viewport and carries every fixed overlay off the side of the
//! screen.
//!
//! So the card is a centred modal drawn by [`UserCardLayer`], mounted once at
//! the root beside the alert layer and for the same reasons that one gives: it
//! is outside the page's stacking context, so a sticky table header cannot
//! paint over it, and it survives the row that opened it.
//!
//! # It hides when the viewer may not read the directory
//!
//! The card is directory data, and what it is being read *next to* does not
//! change what it is. Somebody who may read a trail but not the directory sees
//! the address the trail recorded and no button.
//!
//! That is cosmetic, as everywhere else in the kit:
//! `phonix_services::identity::directory::card` requires the permission, and
//! that is the refusal.

use leptos::prelude::*;
use phonix_core::i18n::datetime;
use phonix_core::identity::{UserCard, UserId};
use phonix_core::permissions;

use crate::components::avatar::Avatar;
use crate::components::page::{Badge, Tone};
use crate::i18n::{Locale, t};
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::server_fns::admin_fns::user_card;
use crate::ui::viewer::Viewer;

/// Which account's card is open, shared between the links and the layer.
///
/// A context rather than a prop, because the thing that opens the card is in a
/// table cell and the thing that draws it is at the root of the document. The
/// two never meet in the tree.
#[derive(Copy, Clone)]
pub struct OpenCard(RwSignal<Option<UserId>>);

impl OpenCard {
    /// Make the card layer reachable from everything below. The host calls this
    /// once, beside `Alerts::provide`.
    pub fn provide() {
        provide_context(Self(RwSignal::new(None)));
    }

    /// The open account, or a signal that is permanently nobody when no host
    /// has provided one.
    ///
    /// Nobody rather than a panic: a `UserLink` rendered in a test, or in a
    /// tree whose host has not been updated, should draw an address rather than
    /// take the page down.
    pub fn get() -> Self {
        use_context::<Self>().unwrap_or_else(|| Self(RwSignal::new(None)))
    }

    pub fn open(self, user_id: UserId) {
        self.0.set(Some(user_id));
    }

    pub fn close(self) {
        self.0.set(None);
    }
}

/// An address, and the person behind it.
///
/// `user_id` is `None` on a row whose actor has since been deleted, or which
/// nobody was behind - a change the system made. The address still renders;
/// there is simply nothing to open.
#[component]
pub fn user_link(
    /// What the trail recorded. Shown verbatim, because it is what the row
    /// says and the row is the record.
    #[prop(into)]
    email: Option<String>,
    #[prop(into)] user_id: Option<UserId>,
    /// Drawn when there is no address at all: "The system", "Somebody".
    #[prop(into, default = String::new())]
    absent: String,
) -> impl IntoView {
    let viewer = Viewer::get();
    let cards = OpenCard::get();

    let may_look = move || {
        viewer
            .get()
            .is_some_and(|user| user.can(permissions::USERS))
    };

    let Some(email) = email.filter(|email| !email.is_empty()) else {
        return view! {
            <span class="text-content-subtle">{absent}</span>
        }
        .into_any();
    };

    let label = email.clone();

    view! {
        <span class="inline-flex min-w-0 items-center gap-1">
            <span class="truncate">{email}</span>

            {user_id
                .map(|user_id| {
                    view! {
                        <Show when=may_look fallback=|| ()>
                            <button
                                type="button"
                                class="shrink-0 rounded-control p-0.5 text-content-subtle hover:bg-surface-hover hover:text-content"
                                // The address is already beside it, so the
                                // button needs a name of its own or a screen
                                // reader announces a bare "button".
                                aria-label=l!("card.who_is", name = label.clone())
                                title=l!("card.who_is", name = label.clone())
                                on:click=move |event| {
                                    // The link sits inside table rows that are
                                    // themselves clickable. Without this, asking
                                    // who somebody is also navigates away from
                                    // the row that asked.
                                    event.stop_propagation();
                                    event.prevent_default();
                                    cards.open(user_id);
                                }
                            >
                                <Icon icon=Icon::Info size=IconSize::Xs />
                            </button>
                        </Show>
                    }
                })}
        </span>
    }
    .into_any()
}

/// The card itself, mounted once at the root of the application.
///
/// Nothing is rendered - and nothing is *fetched* - until a link opens one.
/// Both halves of that matter:
///
/// * The server cannot open a card, so the server and the browser agree on
///   nothing on the first frame, which is what makes the layer hydration-safe.
/// * The fetch lives in [`CardDialog`], below the `Show`, rather than here. A
///   `Resource` created at mount spawns its future immediately, and this
///   component is mounted while the route list is generated - outside any Tokio
///   runtime, where spawning panics. It is also simply wrong to hold a resource
///   for a dialog nobody has opened.
#[component]
pub fn user_card_layer() -> impl IntoView {
    let cards = OpenCard::get();
    let open = cards.0;

    // Escape closes it. On the window rather than on the dialog, because the
    // key has to work whether or not focus made it into the card - a modal
    // nobody can close without finding a button is a trap.
    Effect::new(move |_| {
        let handle = window_event_listener(leptos::ev::keydown, move |event| {
            if event.key() == "Escape" {
                cards.close();
            }
        });

        on_cleanup(move || handle.remove());
    });

    view! {
        {move || {
            open.get()
                .map(|user_id| {
                    view! {
                        <div
                            class="fixed inset-0 z-[70] grid place-items-center bg-overlay p-4"
                            role="dialog"
                            aria-modal="true"
                            aria-label=l!("card.title")
                            on:click=move |_| cards.close()
                        >
                            <div
                                class="w-[min(24rem,100%)] rounded-card border border-edge bg-surface-raised p-4 shadow-lg"
                                // The backdrop closes; the card must not close
                                // itself when somebody clicks the name they came
                                // to read.
                                on:click=move |event| event.stop_propagation()
                            >
                                <CardDialog user_id=user_id />

                                <div class="mt-4 flex justify-end">
                                    <button
                                        type="button"
                                        class="rounded-control px-3 py-1.5 text-sm font-medium text-content hover:bg-surface-hover"
                                        on:click=move |_| cards.close()
                                    >
                                        {l!("common.close")}
                                    </button>
                                </div>
                            </div>
                        </div>
                    }
                })
        }}
    }
}

/// One open card, and the fetch behind it.
///
/// Its own component so the resource is created when the dialog is, and dropped
/// with it - see the note on [`UserCardLayer`]. Keyed on `user_id` by being
/// rebuilt: opening a second card unmounts this and mounts another, which is
/// what a fresh fetch means here.
#[component]
fn card_dialog(user_id: UserId) -> impl IntoView {
    let card = Resource::new(
        move || user_id,
        |user_id| async move { user_card(user_id).await.ok().flatten() },
    );

    view! {
        <Suspense fallback=|| {
            view! { <p class="text-sm text-content-subtle">"Loading..."</p> }
        }>
            {move || Suspend::new(async move {
                match card.await {
                    Some(card) => view! { <CardBody card=card /> }.into_any(),
                    // Deleted, or not this viewer's to see. One sentence for
                    // both: which of the two it is would itself be telling
                    // somebody whether an account exists.
                    None => {
                        view! {
                            <p class="text-sm text-content-muted">
                                {l!("card.absent")}
                            </p>
                        }
                            .into_any()
                    }
                }
            })}
        </Suspense>
    }
}

/// One person, as the card reads them.
#[component]
fn card_body(card: UserCard) -> impl IntoView {
    let initials = card.initials();
    let name = card.display_name.clone();
    let email = card.email.clone();
    let href = card.href();
    let status = card.status;
    let is_owner = card.is_owner;
    let roles = card.roles.clone();
    let department = card.department.clone();
    let job_title = card.job_title.clone();
    let organizational = card.has_organizational_detail();
    let words = Locale::get().shared();
    let last_login = card
        .last_login_at
        .map(|at| datetime::moment_short(&words, at))
        .unwrap_or_else(|| l!("card.never_signed_in"));
    let joined = datetime::day_short(&words, card.created_at.date_naive());

    view! {
        <div class="flex items-start gap-3">
            <Avatar initials=initials file_id=card.avatar_file_id size="size-12".to_owned() />

            <div class="min-w-0">
                <p class="truncate text-sm font-semibold text-content">{name}</p>
                <p class="truncate text-xs text-content-muted">{email}</p>

                <div class="mt-1.5 flex flex-wrap items-center gap-1">
                    <Badge label=t(&status.label()) tone=status_tone(status) />
                    {is_owner.then(|| view! { <Badge label=l!("account.badge.owner") tone=Tone::Brand /> })}
                </div>
            </div>
        </div>

        // Only when something has filled it in. A heading over two empty rows
        // reads as data that failed to load rather than as data nobody entered.
        {organizational
            .then(|| {
                view! {
                    <dl class="mt-3 grid gap-x-6 gap-y-2 border-t border-edge pt-3 text-sm sm:grid-cols-2">
                        <Row label=l!("card.department") value=department.unwrap_or_default() />
                        <Row label=l!("card.job_title") value=job_title.unwrap_or_default() />
                    </dl>
                }
            })}

        <dl class="mt-3 grid gap-x-6 gap-y-2 border-t border-edge pt-3 text-sm sm:grid-cols-2">
            <Row label=l!("field.last_sign_in") value=last_login />
            <Row label=l!("card.created") value=joined />
        </dl>

        {(!roles.is_empty())
            .then(|| {
                view! {
                    <div class="mt-3 border-t border-edge pt-3">
                        <p class="text-2xs uppercase tracking-wide text-content-subtle">
                            {l!("field.roles")}
                        </p>
                        <div class="mt-1 flex flex-wrap gap-1">
                            {roles
                                .into_iter()
                                .map(|role| view! { <Badge label=role /> })
                                .collect::<Vec<_>>()}
                        </div>
                    </div>
                }
            })}

        <a
            href=href
            class="mt-3 inline-flex items-center gap-1.5 text-sm font-medium text-brand hover:underline"
        >
            {l!("card.open")}
            <Icon icon=Icon::ArrowRight size=IconSize::Xs />
        </a>
    }
}

/// One labelled fact. Absent values are left out rather than drawn as a dash.
#[component]
fn row(#[prop(into)] label: String, #[prop(into)] value: String) -> impl IntoView {
    (!value.is_empty()).then(|| {
        view! {
            <div>
                <dt class="text-2xs uppercase tracking-wide text-content-subtle">{label}</dt>
                <dd class="break-words text-content">{value}</dd>
            </div>
        }
    })
}

/// How loudly to draw it. Only a suspension is a warning: an invitation nobody
/// has accepted yet is an ordinary state, and a deactivated account is a fact
/// rather than a problem.
const fn status_tone(status: phonix_core::identity::UserStatus) -> Tone {
    use phonix_core::identity::UserStatus;

    match status {
        UserStatus::Active => Tone::Success,
        UserStatus::Suspended => Tone::Danger,
        UserStatus::Pending | UserStatus::Deactivated => Tone::Neutral,
    }
}
