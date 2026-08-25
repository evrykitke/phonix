//! The app store: what this workspace has, and what it could have.
//!
//! # The eight seconds
//!
//! The install itself is one `UPDATE` and a permission sync - somewhere under a
//! tenth of a second. Returning that fast is a button that flashes and a menu
//! that has silently changed, and the reaction to that is not "how quick" but
//! "did that work?". So the dialog walks four named steps over about eight
//! seconds and only then says the app is ready.
//!
//! The delay is honest about what it is, which is why it is *here* and not in
//! the service: the server function returns immediately, and every other
//! caller (a script, the API, a bulk provisioning run) gets the fast answer.
//! What is staged is the *telling*, and the steps name things that genuinely
//! happened, in the order they happened in.
//!
//! # The steps run even if the call fails
//!
//! No: the animation and the request start together, and the dialog will not
//! claim success until *both* the timer has finished and the call has come back
//! `Ok`. A progress bar that completes and then shows an error is worse than
//! one that stops.
//!
//! # Why a switched-off app is still listed
//!
//! With everything about it: name, summary, version. It is a catalog, and a
//! catalog that hid what you have not bought would be a strange catalog.

use std::time::Duration;

use leptos::prelude::*;
use leptos_router::components::A;
use phonix_core::apps::{self, AppDescriptor, AppState, UninstallOutcome};
use phonix_core::i18n::Message;
use phonix_core::permissions;

use crate::apps::icon_of;
use crate::components::page::{Badge, GhostButton, PageHeader, PrimaryButton, Tone};
use crate::components::shell::Shell;
use crate::i18n::t;
use crate::icons::{Icon, IconSize};
use crate::l;
use crate::server_fns::app_fns::{app_catalog, install_app, uninstall_app};
use crate::ui::alert::{Alert, Alerts};

/// How long the install dialog takes to say it is done.
///
/// Long enough to read four steps, short enough that nobody reaches for the
/// tab bar. See the module docs for why there is a delay at all.
const INSTALL_MS: u64 = 8_000;

/// The four things the dialog says are happening, in order.
///
/// Each names something the install actually does. `data` is the one that
/// stretches furthest - the schema was migrated at boot, not now - but "where
/// its records will live" is true of a schema that is being handed to a
/// workspace for the first time, and a step that lied would be worse than a
/// step that abbreviates.
const STEPS: &[&str] = &[
    "apps.installing.step.subscription",
    "apps.installing.step.data",
    "apps.installing.step.permissions",
    "apps.installing.step.menu",
];

#[component]
pub fn apps_page() -> impl IntoView {
    // Bumped after an install or an uninstall, which re-fetches the catalog.
    let version = RwSignal::new(0_u32);
    // The app whose install dialog is up.
    let installing: RwSignal<Option<&'static AppDescriptor>> = RwSignal::new(None);

    let states = Resource::new(
        move || version.get(),
        |_| async move { app_catalog().await.unwrap_or_default() },
    );

    view! {
        <div>
            <PageHeader
                title=l!("apps.title")
                subtitle=l!("apps.subtitle")
                icon=Icon::Blocks
            />

            <Suspense fallback=|| view! { <div class="h-40" /> }>
                {move || Suspend::new(async move {
                    let states = states.await;

                    let is_on = |id: &str| {
                        states.iter().any(|state: &AppState| state.app_id == id && state.enabled)
                    };
                    let state_of = |id: &str| {
                        states.iter().find(|state: &&AppState| state.app_id == id).cloned()
                    };

                    let on: Vec<_> = apps::CATALOG.iter().filter(|app| is_on(app.id)).collect();
                    let off: Vec<_> = apps::CATALOG
                        .iter()
                        .filter(|app| !is_on(app.id))
                        .collect();

                    let installed_cards = on
                        .into_iter()
                        .map(|app| {
                            view! {
                                <AppCard
                                    app=app
                                    state=state_of(app.id)
                                    installed=true
                                    version=version
                                    installing=installing
                                />
                            }
                        })
                        .collect::<Vec<_>>();

                    let available = off
                        .into_iter()
                        .map(|app| {
                            view! {
                                <AppCard
                                    app=app
                                    state=state_of(app.id)
                                    installed=false
                                    version=version
                                    installing=installing
                                />
                            }
                        })
                        .collect::<Vec<_>>();

                    let nothing_left = available.is_empty();

                    view! {
                        <section class="space-y-2">
                            <h2 class="text-xs font-medium uppercase tracking-wide text-content-subtle">
                                {l!("apps.installed")}
                            </h2>
                            <div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
                                {installed_cards}
                            </div>
                        </section>

                        <section class="mt-6 space-y-2">
                            <h2 class="text-xs font-medium uppercase tracking-wide text-content-subtle">
                                {l!("apps.available")}
                            </h2>
                            {if nothing_left {
                                view! {
                                    <p class="rounded-panel border border-dashed border-edge px-4 py-6 text-center text-sm text-content-muted">
                                        {l!("apps.empty.detail")}
                                    </p>
                                }
                                    .into_any()
                            } else {
                                view! {
                                    <div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
                                        {available}
                                    </div>
                                }
                                    .into_any()
                            }}
                        </section>
                    }
                })}
            </Suspense>

            // One dialog for the page rather than one per card: only ever one
            // install is in flight, and a dialog per tile would be a dozen
            // hidden overlays in the document.
            {move || {
                installing
                    .get()
                    .map(|app| view! { <InstallDialog app=app version=version installing=installing /> })
            }}
        </div>
    }
}

/// One app, as a tile.
#[component]
fn app_card(
    app: &'static AppDescriptor,
    state: Option<AppState>,
    installed: bool,
    version: RwSignal<u32>,
    installing: RwSignal<Option<&'static AppDescriptor>>,
) -> impl IntoView {
    let alerts = Alerts::get();
    let viewer = crate::ui::viewer::Viewer::get();
    let may_install = move || {
        viewer
            .get()
            .is_some_and(|user| user.can(permissions::APPS_INSTALL))
    };

    // The version this workspace is running is the compiled one. The one in the
    // row is what it had when it was switched on, and the two differing is the
    // whole reason a changelog exists - but a tile is not where that
    // conversation belongs, so the tile shows what is running.
    let running = app.version;

    let needs: Vec<&'static AppDescriptor> = app
        .requires
        .iter()
        .filter_map(|id| apps::find(id))
        .collect();

    let uninstall = Action::new(move |app_id: &String| {
        let app_id = app_id.clone();
        async move {
            match uninstall_app(app_id).await {
                Ok(UninstallOutcome::SwitchedOff) => {
                    version.update(|value| *value += 1);
                    // The menu has just *lost* entries, which matters more than
                    // gaining them: a link left in the sidebar now leads to a
                    // page that refuses.
                    Shell::get().refresh();
                    alerts
                        .post(Alert::success(t(&Message::new("apps.uninstall.done")
                            .arg("app", t(&Message::new(app.name))))));
                }
                // Both of these name an app, which is why the service answers
                // with an id rather than a sentence: the name is a message key,
                // and the browser is where a key becomes words.
                Ok(UninstallOutcome::AlwaysOn) => alerts
                    .post(Alert::failure(t(&Message::new("apps.error.always_on")
                        .arg("app", t(&Message::new(app.name)))))),
                Ok(UninstallOutcome::NeededBy { app_id }) => {
                    let dependant = apps::find(&app_id)
                        .map_or(app_id.clone(), |other| t(&Message::new(other.name)));
                    alerts.post(Alert::failure(t(
                        &Message::new("apps.error.needed_by").arg("app", dependant)
                    )));
                }
                Err(err) => alerts.post(Alert::failure(err.to_string())),
            }
        }
    });

    view! {
        <article class="flex flex-col rounded-panel border border-edge bg-surface-raised p-4">
            <div class="flex items-start gap-3">
                <span class="grid size-9 shrink-0 place-items-center rounded-control bg-brand-subtle text-brand">
                    <Icon icon=icon_of(app) size=IconSize::Md />
                </span>
                <div class="min-w-0 flex-1">
                    <div class="flex items-center gap-2">
                        <h3 class="truncate-fade text-sm font-semibold text-content">
                            {t(&Message::new(app.name))}
                        </h3>
                        {app
                            .always_on
                            .then(|| {
                                view! { <Badge label=t(&Message::new("apps.included")) tone=Tone::Neutral /> }
                            })}
                    </div>
                    <p class="mt-0.5 text-xs leading-relaxed text-content-muted">
                        {t(&Message::new(app.summary))}
                    </p>
                </div>
            </div>

            <dl class="mt-3 space-y-1 text-2xs text-content-subtle">
                <div>{t(&Message::new("apps.version").arg("version", running))}</div>
                {state
                    .as_ref()
                    .and_then(|state| state.enabled_on)
                    .map(|on| {
                        view! {
                            <div>
                                {t(
                                    &Message::new("apps.installed_on")
                                        .arg("date", on.date_naive().to_string()),
                                )}
                            </div>
                        }
                    })}
                {(!needs.is_empty() && !installed)
                    .then(|| {
                        let names = needs
                            .iter()
                            .map(|needed| t(&Message::new(needed.name)))
                            .collect::<Vec<_>>()
                            .join(", ");

                        view! {
                            <div>
                                {t(&Message::new("apps.requires").arg("apps", names))}
                                " "
                                {l!("apps.requires_note")}
                            </div>
                        }
                    })}
            </dl>

            <div class="mt-4 flex items-center gap-2 border-t border-edge pt-3">
                {if app.always_on {
                    // Nothing to press. Saying so with a disabled button would
                    // be offering a control that has never once been usable.
                    ().into_any()
                } else if installed {
                    view! {
                        <Show when=may_install fallback=|| ()>
                            <GhostButton
                                label=t(&Message::new("apps.uninstall"))
                                icon=Icon::Trash2
                                disabled=Signal::derive(move || uninstall.pending().get())
                                on_click=Callback::new(move |()| {
                                    uninstall.dispatch(app.id.to_owned());
                                })
                            />
                        </Show>
                        {app
                            .home
                            .map(|home| {
                                view! {
                                    <a
                                        href=home
                                        class="inline-flex h-8 items-center gap-1.5 rounded-control px-3 text-sm text-content-muted hover:bg-surface-hover hover:text-content"
                                    >
                                        {l!("common.open")}
                                        <Icon icon=Icon::ArrowRight size=IconSize::Xs />
                                    </a>
                                }
                            })}
                    }
                        .into_any()
                } else {
                    view! {
                        <Show when=may_install fallback=|| ()>
                            <PrimaryButton
                                label=t(&Message::new("apps.install"))
                                icon=Icon::Download
                                on_click=Callback::new(move |()| installing.set(Some(app)))
                            />
                        </Show>
                    }
                        .into_any()
                }}
            </div>
        </article>
    }
}

/// The overlay that walks the four steps.
///
/// Mounted only while an install is in flight - the page creates it when the
/// button is pressed and drops it when the dialog closes - so its timers begin
/// on mount and need no start signal.
#[component]
fn install_dialog(
    app: &'static AppDescriptor,
    version: RwSignal<u32>,
    installing: RwSignal<Option<&'static AppDescriptor>>,
) -> impl IntoView {
    // How far through the steps the dialog is. `STEPS.len()` means done.
    let step = RwSignal::new(0_usize);
    let finished = RwSignal::new(false);
    let failed: RwSignal<Option<String>> = RwSignal::new(None);
    // Anything that came along because this app needs it. Named on the way
    // out: somebody who asked for Books and got a second entry in their menu
    // should be told why rather than left to work it out.
    let dependencies: RwSignal<Vec<String>> = RwSignal::new(Vec::new());

    // The request and the animation start together. Neither waits for the
    // other, and the dialog does not say "ready" until both are through.
    let request = Action::new(move |(): &()| async move {
        match install_app(app.id.to_owned()).await {
            Ok(installed) => {
                dependencies.set(
                    installed
                        .switched_on
                        .into_iter()
                        .filter(|id| id != app.id)
                        .collect(),
                );
                version.update(|value| *value += 1);
                true
            }
            Err(err) => {
                failed.set(Some(err.to_string()));
                false
            }
        }
    });
    request.dispatch(());

    // One timer per step, all set at mount. An interval would need clearing on
    // unmount and would keep running if the person navigated away mid-install;
    // these are `set_timeout` handles that simply do not fire into a dropped
    // scope, because every closure only touches signals owned by this
    // component.
    let each = INSTALL_MS / STEPS.len() as u64;
    for index in 1..=STEPS.len() {
        set_timeout(
            move || step.set(index),
            Duration::from_millis(each * index as u64),
        );
    }

    // "Done" is the *later* of the two: the animation ending and the call
    // returning. Claiming success on the timer alone would be a dialog that
    // congratulates somebody on an install that failed.
    Effect::new(move |_| {
        let animation_done = step.get() >= STEPS.len();
        let call_done = request.value().with(Option::is_some);
        let succeeded = request.value().with(|value| *value == Some(true));

        if animation_done && call_done && succeeded {
            finished.set(true);
        }
    });

    let close = move || installing.set(None);

    // Closing the dialog is not the whole of it. The sidebar, the launcher and
    // every permission-gated control in the shell resolved their session when
    // this document loaded, and the set they resolved against has just gained
    // an app - so the menu on screen does not have the thing that was just
    // installed in it.
    //
    // This reloaded the page at first, which worked and read badly: after
    // eight seconds of progress dialog, throwing the document away looks like
    // the application restarting rather than a menu gaining an entry.
    // `Shell::refresh` re-fetches the two facts that changed, and the sidebar
    // is a `Transition` so the old menu stays up until the new one is ready.
    let dismiss = move || {
        Shell::get().refresh();
        installing.set(None);
    };

    view! {
        <div
            class="fixed inset-0 z-50 grid place-items-center bg-overlay p-4"
            role="dialog"
            aria-modal="true"
        >
            <div class="w-full max-w-sm rounded-panel border border-edge bg-surface-raised p-6 shadow-pop">
                {move || {
                    if let Some(error) = failed.get() {
                        return view! {
                            <div class="space-y-4 text-center">
                                <span class="mx-auto grid size-12 place-items-center rounded-full bg-danger-subtle text-danger">
                                    <Icon icon=Icon::TriangleAlert size=IconSize::Lg />
                                </span>
                                <p class="text-sm text-content">{error}</p>
                                <GhostButton
                                    label=t(&Message::new("common.close"))
                                    on_click=Callback::new(move |()| close())
                                />
                            </div>
                        }
                            .into_any();
                    }

                    if finished.get() {
                        let name = t(&Message::new(app.name));
                        return view! {
                            <div class="space-y-4 text-center">
                                <span class="mx-auto grid size-12 place-items-center rounded-full bg-success-subtle text-success">
                                    <Icon icon=Icon::CircleCheck size=IconSize::Lg />
                                </span>
                                <div>
                                    <h2 class="text-base font-semibold text-content">
                                        {t(
                                            &Message::new("apps.installed.title")
                                                .arg("app", name.clone()),
                                        )}
                                    </h2>
                                    <p class="mt-1 text-sm text-content-muted">
                                        {l!("apps.installed.detail")}
                                    </p>
                                    {(!dependencies.get().is_empty())
                                        .then(|| {
                                            let names = dependencies
                                                .get()
                                                .iter()
                                                .filter_map(|id| apps::find(id))
                                                .map(|other| t(&Message::new(other.name)))
                                                .collect::<Vec<_>>()
                                                .join(", ");

                                            view! {
                                                <p class="mt-2 text-sm text-content-muted">
                                                    {t(
                                                        &Message::new("apps.installed.also")
                                                            .arg("apps", names)
                                                            .arg("app", name.clone()),
                                                    )}
                                                </p>
                                            }
                                        })}
                                </div>
                                <div class="flex justify-center gap-2">
                                    <GhostButton
                                        label=t(&Message::new("apps.installed.stay"))
                                        on_click=Callback::new(move |()| dismiss())
                                    />
                                    // Refreshes the chrome on the way, for the
                                    // reason `dismiss` does: the destination
                                    // is a page this account could not reach a
                                    // moment ago.
                                    <A
                                        href=app.home.unwrap_or("/")
                                        attr:class="inline-flex h-8 items-center gap-1.5 rounded-control bg-brand px-3 text-sm font-medium text-on-brand hover:bg-brand-hover"
                                        on:click=move |_| dismiss()
                                    >
                                        {t(
                                            &Message::new("apps.installed.open")
                                                .arg("app", name.clone()),
                                        )}
                                    </A>
                                </div>
                            </div>
                        }
                            .into_any();
                    }

                    let current = step.get();

                    view! {
                        <div class="space-y-4">
                            <div class="flex items-center gap-3">
                                <span class="grid size-9 shrink-0 place-items-center rounded-control bg-brand-subtle text-brand">
                                    <Icon icon=icon_of(app) size=IconSize::Md />
                                </span>
                                <h2 class="text-base font-semibold text-content">
                                    {t(
                                        &Message::new("apps.installing.title")
                                            .arg("app", t(&Message::new(app.name))),
                                    )}
                                </h2>
                            </div>

                            <div
                                class="h-1 overflow-hidden rounded-full bg-surface-sunken"
                                role="progressbar"
                                aria-valuemin="0"
                                aria-valuemax=STEPS.len().to_string()
                                aria-valuenow=current.to_string()
                            >
                                <div
                                    class="h-full rounded-full bg-brand transition-all duration-700 ease-out"
                                    style=format!(
                                        "width:{}%",
                                        current * 100 / STEPS.len(),
                                    )
                                />
                            </div>

                            <ol class="space-y-2">
                                {STEPS
                                    .iter()
                                    .enumerate()
                                    .map(|(index, key)| {
                                        let done = index < current;
                                        let active = index == current;

                                        view! {
                                            <li class=if done || active {
                                                "flex items-center gap-2 text-sm text-content"
                                            } else {
                                                "flex items-center gap-2 text-sm text-content-subtle"
                                            }>
                                                <span class="grid size-4 shrink-0 place-items-center">
                                                    {if done {
                                                        view! {
                                                            <span class="text-success">
                                                                <Icon icon=Icon::Check size=IconSize::Xs />
                                                            </span>
                                                        }
                                                            .into_any()
                                                    } else if active {
                                                        view! {
                                                            <span class="animate-spin text-brand">
                                                                <Icon
                                                                    icon=Icon::LoaderCircle
                                                                    size=IconSize::Xs
                                                                />
                                                            </span>
                                                        }
                                                            .into_any()
                                                    } else {
                                                        view! {
                                                            <span class="size-1.5 rounded-full bg-edge" />
                                                        }
                                                            .into_any()
                                                    }}
                                                </span>
                                                {t(&Message::new(*key))}
                                            </li>
                                        }
                                    })
                                    .collect::<Vec<_>>()}
                            </ol>

                            <p class="text-xs text-content-subtle">{l!("apps.installing.wait")}</p>
                        </div>
                    }
                        .into_any()
                }}
            </div>
        </div>
    }
}
