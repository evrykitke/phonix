//! Page furniture: the heading block, the panel, the empty state.
//!
//! Every screen inside the shell opens the same way - an icon, a title, a line
//! of explanation, and whatever actions belong to the whole page. Writing that
//! per screen produces five variants within a month, none of them wrong and no
//! two the same.

use leptos::prelude::*;
use leptos_router::components::A;

use crate::icons::{Icon, IconSize};

/// The block at the top of a screen.
#[component]
pub fn page_header(
    #[prop(into)] title: String,
    #[prop(optional, into)] subtitle: Option<String>,
    #[prop(optional)] icon: Option<Icon>,
    /// A link back to where this screen was opened from, for a detail page.
    #[prop(optional)]
    back: Option<(&'static str, String)>,
    /// Buttons belonging to the whole page, right-aligned.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class="mb-4 space-y-2">
            {back
                .map(|(href, label)| {
                    view! {
                        <A
                            href=href
                            attr:class="inline-flex items-center gap-1 text-xs text-content-subtle hover:text-content"
                        >
                            <Icon icon=Icon::ArrowLeft size=IconSize::Xs />
                            {label}
                        </A>
                    }
                })}

            <div class="flex flex-wrap items-start justify-between gap-3">
                <div class="flex min-w-0 items-start gap-2">
                    {icon
                        .map(|icon| {
                            view! {
                                <span class="mt-0.5 grid size-7 shrink-0 place-items-center rounded-control bg-brand-subtle text-brand">
                                    <Icon icon=icon size=IconSize::Sm />
                                </span>
                            }
                        })}
                    <div class="min-w-0">
                        <h1 class="text-xl font-semibold tracking-tight text-content">{title}</h1>
                        {subtitle
                            .map(|subtitle| {
                                view! {
                                    <p class="mt-0.5 text-sm text-content-muted">{subtitle}</p>
                                }
                            })}
                    </div>
                </div>

                {children.map(|children| view! { <div class="flex items-center gap-2">{children()}</div> })}
            </div>
        </div>
    }
}

/// A bordered card with an optional heading strip.
#[component]
pub fn panel(
    #[prop(optional, into)] title: Option<String>,
    #[prop(optional, into)] description: Option<String>,
    children: Children,
) -> impl IntoView {
    view! {
        <section class="overflow-hidden rounded-card border border-edge bg-surface-raised">
            {title
                .map(|title| {
                    view! {
                        <header class="border-b border-edge px-4 py-3">
                            <h2 class="text-sm font-semibold text-content">{title}</h2>
                            {description
                                .map(|description| {
                                    view! {
                                        <p class="mt-0.5 text-xs text-content-subtle">
                                            {description}
                                        </p>
                                    }
                                })}
                        </header>
                    }
                })}
            <div class="p-4">{children()}</div>
        </section>
    }
}

/// What a list shows when it has nothing in it.
#[component]
pub fn empty_state(
    icon: Icon,
    #[prop(into)] title: String,
    #[prop(optional, into)] detail: Option<String>,
) -> impl IntoView {
    view! {
        <div class="flex flex-col items-center gap-1 px-4 py-10 text-center">
            <span class="grid size-9 place-items-center rounded-full bg-surface-sunken text-content-subtle">
                <Icon icon=icon size=IconSize::Md />
            </span>
            <p class="text-sm font-medium text-content">{title}</p>
            {detail.map(|detail| view! { <p class="text-xs text-content-subtle">{detail}</p> })}
        </div>
    }
}

/// A short status word: active, suspended, owner, locked.
#[component]
pub fn badge(
    #[prop(into)] label: String,
    /// One of `neutral`, `brand`, `success`, `warning`, `danger`.
    #[prop(optional)]
    tone: Tone,
    #[prop(optional)] icon: Option<Icon>,
) -> impl IntoView {
    view! {
        <span class=format!(
            "inline-flex items-center gap-1 rounded-full px-1.5 py-0.5 text-2xs font-medium {}",
            tone.classes(),
        )>
            {icon.map(|icon| view! { <Icon icon=icon size=IconSize::Xs /> })}
            {label}
        </span>
    }
}

/// How loud a [`Badge`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Neutral,
    Brand,
    Success,
    Warning,
    Danger,
}

/// How one tone is drawn, wherever it is drawn.
///
/// Every surface that says something went right or wrong reads this rather
/// than choosing its own icon and colour: the inline [`Notice`], the toast, the
/// message box and the confirmation dialog. See [`crate::ui::alert`] for why
/// that matters - four surfaces that each picked their own green is four
/// different greens, and a reader who has to learn the vocabulary once per
/// screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Face {
    /// The glyph. Never the only carrier of the meaning - the words say it too.
    pub icon: Icon,
    /// Colour for the icon and for text that is the message itself.
    pub accent: &'static str,
    /// A tinted disc behind the icon, for the surfaces big enough to have one.
    pub disc: &'static str,
    /// The border of a bordered surface.
    pub edge: &'static str,
}

impl Tone {
    /// How this tone looks on any alert surface.
    pub const fn face(self) -> Face {
        match self {
            // Danger is the one tone that colours its own border. Everything
            // else keeps the ordinary edge, so a page of notices does not read
            // as a page of boxes.
            Self::Danger => Face {
                icon: Icon::CircleAlert,
                accent: "text-danger",
                disc: "bg-danger-subtle text-danger",
                edge: "border-danger",
            },
            Self::Success => Face {
                icon: Icon::CircleCheck,
                accent: "text-success",
                disc: "bg-surface-sunken text-success",
                edge: "border-edge",
            },
            Self::Warning => Face {
                icon: Icon::TriangleAlert,
                accent: "text-warning",
                disc: "bg-surface-sunken text-warning",
                edge: "border-edge",
            },
            Self::Brand => Face {
                icon: Icon::Info,
                accent: "text-brand",
                disc: "bg-brand-subtle text-brand",
                edge: "border-edge",
            },
            Self::Neutral => Face {
                icon: Icon::Info,
                accent: "text-content-muted",
                disc: "bg-surface-sunken text-content-muted",
                edge: "border-edge",
            },
        }
    }

    const fn classes(self) -> &'static str {
        match self {
            Self::Neutral => "bg-surface-sunken text-content-muted",
            Self::Brand => "bg-brand-subtle text-brand",
            // The status colours carry meaning, so they are stated as text on a
            // tinted background rather than as a colour alone - a badge nobody
            // can distinguish is a badge that says nothing.
            Self::Success => "bg-surface-sunken text-success",
            Self::Warning => "bg-surface-sunken text-warning",
            Self::Danger => "bg-danger-subtle text-danger",
        }
    }
}

/// The bar of buttons at the bottom of a form.
#[component]
pub fn form_actions(children: Children) -> impl IntoView {
    view! {
        <div class="flex flex-wrap items-center justify-end gap-2 border-t border-edge px-4 py-3">
            {children()}
        </div>
    }
}

/// The primary action of a screen.
#[component]
pub fn primary_button(
    #[prop(into)] label: String,
    #[prop(optional, into)] pending: Signal<bool>,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional)] icon: Option<Icon>,
    #[prop(optional, into)] button_type: Option<&'static str>,
    #[prop(optional)] on_click: Option<Callback<()>>,
) -> impl IntoView {
    view! {
        <button
            type=button_type.unwrap_or("button")
            class="inline-flex h-8 items-center gap-1.5 rounded-control bg-brand px-3 text-sm font-medium text-on-brand hover:bg-brand-hover focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-60"
            disabled=move || pending.get() || disabled.get()
            on:click=move |_| {
                if let Some(on_click) = on_click {
                    on_click.run(());
                }
            }
        >
            {icon.map(|icon| view! { <Icon icon=icon size=IconSize::Xs /> })}
            {move || if pending.get() { "Saving...".to_owned() } else { label.clone() }}
        </button>
    }
}

/// A quieter button beside the primary one.
#[component]
pub fn ghost_button(
    #[prop(into)] label: String,
    #[prop(optional)] icon: Option<Icon>,
    #[prop(optional, into)] disabled: Signal<bool>,
    /// [`Tone::Danger`] for the one that removes something. Only that tone
    /// changes anything: a ghost button is quiet by definition, and a green or
    /// amber one would be a badge with a click handler.
    #[prop(optional)]
    tone: Tone,
    on_click: Callback<()>,
) -> impl IntoView {
    let colours = match tone {
        Tone::Danger => "border-danger/40 text-danger hover:bg-danger-subtle",
        _ => "border-edge text-content-muted hover:bg-surface-hover hover:text-content",
    };

    view! {
        <button
            type="button"
            class=format!(
                "inline-flex h-8 items-center gap-1.5 rounded-control border px-3 text-sm \
                 disabled:cursor-not-allowed disabled:opacity-60 {colours}",
            )
            disabled=move || disabled.get()
            on:click=move |_| on_click.run(())
        >
            {icon.map(|icon| view! { <Icon icon=icon size=IconSize::Xs /> })}
            {label}
        </button>
    }
}

/// A line of text explaining that something went wrong, or went right.
///
/// The quietest of the four alert surfaces, and the only one that sits in the
/// document rather than over it. It is the right one when the message belongs
/// beside the thing it is about - a short form, the head of a table - and the
/// wrong one when the person's eyes have already left that part of the screen.
/// See [`crate::ui::alert`] for choosing between them.
#[component]
pub fn notice(
    #[prop(into)] message: Signal<Option<String>>,
    #[prop(optional)] tone: Tone,
) -> impl IntoView {
    let face = tone.face();

    view! {
        {move || {
            message
                .get()
                .map(|message| {
                    view! {
                        <div
                            role="status"
                            class=format!(
                                "flex items-start gap-2 rounded-control border px-3 py-2 text-sm {} {}",
                                face.edge,
                                face.accent,
                            )
                        >
                            <span class="mt-0.5 shrink-0">
                                <Icon icon=face.icon size=IconSize::Xs />
                            </span>
                            <span>{message}</span>
                        </div>
                    }
                })
        }}
    }
}
