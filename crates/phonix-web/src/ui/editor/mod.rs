//! The rich text editor.
//!
//! # Why there is a JavaScript dependency in a Rust application
//!
//! A text editor is not a text box. What looks like "make the selection bold"
//! is a document model, a transform algebra, a selection that survives those
//! transforms, an undo history that groups keystrokes the way a person expects,
//! and the input-method handling that makes any of it work in a language that
//! is not typed one character at a time. ProseMirror is the library that solved
//! that, TipTap is the ergonomic layer over it, and writing either again in
//! Rust is a project rather than a component.
//!
//! It is paid for in one number: **131 KiB gzipped**, fetched the first time an
//! editor appears on a page and never on a page without one. That is not
//! nothing - the entire icon set is 8 KiB by comparison - which is why the
//! loading is deferred and why this is the only dependency of its kind.
//!
//! # The line between the two halves
//!
//! JavaScript owns the document. Rust owns everything a person sees. There is
//! no toolbar, no dialog and no word of English in `tools/editor/index.js`: it
//! exposes five functions and a vocabulary of command names, and the toolbar
//! here is ordinary Leptos - the application's own icons, its own tokens, its
//! own catalog, and its own permission gating if it ever needs any.
//!
//! Keeping the line there is what stops the editor being the one control that
//! ignores the theme and speaks only English.
//!
//! # What is stored
//!
//! HTML, and **it is not safe until a server has said so**. Everything reaching
//! this component came out of a browser, and a browser is not a trustworthy
//! source of markup however carefully the editor was configured. Whatever
//! persists a value from here sanitises it on the way in and renders it through
//! `.rich-text`, which is the class that also styles what the editor writes -
//! so the page it is read back on cannot drift away from the page it was
//! written on.
//!
//! # Hydration
//!
//! The mount point is an empty `<div>` in the server's HTML and an empty
//! `<div>` at hydration; the editor is put inside it afterwards, by an effect.
//! That is the whole of the arrangement, and it is why there is no `Suspense`
//! here and no markup that differs between the two renders. ProseMirror owns
//! every node below the mount point from that moment on, and Leptos must never
//! be given a reason to patch one - which is why the mount point has no
//! children in the `view!` below.

mod bundle;
mod state;

#[cfg(feature = "hydrate")]
mod browser;

pub use bundle::{EDITOR_BYTES, EDITOR_GZIP_BYTES, EDITOR_SRC};
pub use state::{Command, EditorState};

use leptos::prelude::*;

use crate::components::page::{GhostButton, Tone};
use crate::icons::{Icon, IconSize};
use crate::l;

#[cfg(feature = "hydrate")]
use browser::Dispatch;

/// The server's counterpart to the browser's [`browser::Dispatch`].
///
/// Declared here rather than as a `cfg` inside every handler: the split is in
/// one place, and the toolbar below is written once.
#[cfg(not(feature = "hydrate"))]
#[derive(Clone, Copy)]
struct Dispatch;

#[cfg(not(feature = "hydrate"))]
impl Dispatch {
    fn install(
        _host: NodeRef<leptos::html::Div>,
        _value: RwSignal<String>,
        _state: RwSignal<EditorState>,
        _ready: RwSignal<bool>,
        _disabled: Signal<bool>,
        _label: Option<String>,
    ) -> Self {
        Self
    }

    fn run(self, _command: Command, _argument: Option<String>) {}
}

/// A rich text field.
///
/// ```ignore
/// let terms = RwSignal::new(String::new());
/// view! { <RichText value=terms /> }
/// ```
#[component]
pub fn rich_text(
    /// The document, as HTML. Read once when the editor mounts and written on
    /// every change; setting it from outside replaces what is on screen.
    value: RwSignal<String>,
    /// Read-only, not hidden. A field somebody may see and not change keeps
    /// its value on screen and stops taking keystrokes - the same rule the
    /// rest of [`crate::ui::form`] follows.
    #[prop(optional, into)]
    disabled: Signal<bool>,
    /// The writing area's accessible name.
    ///
    /// Not a `<label for>`: the control is a contenteditable created by the
    /// bundle after this component has rendered, so there is no id here to
    /// point at. It reaches the right element as an `aria-label`, which is
    /// what a screen reader reads instead of "edit region".
    #[prop(optional, into)]
    label: Option<String>,
) -> impl IntoView {
    let host = NodeRef::<leptos::html::Div>::new();
    let state = RwSignal::new(EditorState::default());
    // False on the server, false at hydration, true once the bundle has landed
    // and the editor is in the page. The toolbar is drawn either way and is
    // inert until this turns over, which is what stops a button being pressed
    // before there is anything to press it against.
    let ready = RwSignal::new(false);

    let dispatch = Dispatch::install(host, value, state, ready, disabled, label);

    // `None` closed, `Some` open and holding what is being typed. Separate from
    // the state's `link_href`, which is what the document says: the box has to
    // hold an edit in progress that the document has not been told about yet.
    let link_box = RwSignal::new(None::<String>);

    let usable = Signal::derive(move || ready.get() && !disabled.get());

    let run = move |command: Command| {
        dispatch.run(command, None);
    };

    view! {
        <div class="overflow-hidden rounded-card border border-edge bg-surface-raised">
            <div
                role="toolbar"
                aria-label=l!("editor.toolbar")
                class="flex flex-wrap items-center gap-0.5 border-b border-edge bg-surface-sunken px-1 py-1"
            >
                <Tool command=Command::Undo icon=Icon::Undo2 label=l!("editor.undo")
                    dispatch=dispatch state=state usable=usable />
                <Tool command=Command::Redo icon=Icon::Redo2 label=l!("editor.redo")
                    dispatch=dispatch state=state usable=usable />

                <Divider />

                <Tool command=Command::Bold icon=Icon::Bold label=l!("editor.bold")
                    dispatch=dispatch state=state usable=usable />
                <Tool command=Command::Italic icon=Icon::Italic label=l!("editor.italic")
                    dispatch=dispatch state=state usable=usable />
                <Tool command=Command::Underline icon=Icon::Underline label=l!("editor.underline")
                    dispatch=dispatch state=state usable=usable />
                <Tool command=Command::Strike icon=Icon::Strikethrough label=l!("editor.strike")
                    dispatch=dispatch state=state usable=usable />

                <Divider />

                <Tool command=Command::Heading2 icon=Icon::Heading2 label=l!("editor.heading_2")
                    dispatch=dispatch state=state usable=usable />
                <Tool command=Command::Heading3 icon=Icon::Heading3 label=l!("editor.heading_3")
                    dispatch=dispatch state=state usable=usable />
                <Tool command=Command::Blockquote icon=Icon::TextQuote label=l!("editor.blockquote")
                    dispatch=dispatch state=state usable=usable />

                <Divider />

                <Tool command=Command::BulletList icon=Icon::List label=l!("editor.bullet_list")
                    dispatch=dispatch state=state usable=usable />
                <Tool command=Command::OrderedList icon=Icon::ListOrdered label=l!("editor.ordered_list")
                    dispatch=dispatch state=state usable=usable />
                <Tool command=Command::HorizontalRule icon=Icon::Minus label=l!("editor.horizontal_rule")
                    dispatch=dispatch state=state usable=usable />

                <Divider />

                // Not a `Tool`: it opens a box rather than running a command,
                // and the command it eventually runs takes an argument.
                <button
                    type="button"
                    class=move || tool_class(state.with(|state| state.link))
                    title=l!("editor.link")
                    aria-label=l!("editor.link")
                    disabled=move || !usable.get()
                    on:click=move |_| {
                        link_box
                            .update(|open| {
                                *open = match open.take() {
                                    Some(_) => None,
                                    // Seeded from the document, so pressing it
                                    // inside an existing link edits that link
                                    // instead of offering an empty box.
                                    None => Some(state.with(|state| state.link_href.clone())),
                                };
                            });
                    }
                >
                    <Icon icon=Icon::Link size=IconSize::Sm />
                </button>

                <Tool command=Command::InsertTable icon=Icon::Table label=l!("editor.table")
                    dispatch=dispatch state=state usable=usable />

                <Divider />

                <Tool command=Command::Clear icon=Icon::RemoveFormatting label=l!("editor.clear")
                    dispatch=dispatch state=state usable=usable />
            </div>

            <Show when=move || link_box.with(Option::is_some) fallback=|| ()>
                <div class="flex flex-wrap items-center gap-2 border-b border-edge px-2 py-2">
                    // Wrapping, not `for`: an id would have to be unique per
                    // editor, and the only ways to make one are a counter that
                    // the server and the browser can disagree about or a prop
                    // every caller has to remember. A label that contains its
                    // control needs neither.
                    <label class="flex items-center gap-2">
                        <span class="text-xs text-content-muted">
                            {l!("editor.link_address")}
                        </span>
                        <input
                            type="url"
                            class="w-64 max-w-full"
                            // A scheme, not a sentence - there is nothing here
                            // to translate.
                            placeholder="https://"
                            prop:value=move || link_box.get().unwrap_or_default()
                            on:input=move |ev| {
                                link_box.set(Some(event_target_value(&ev)));
                            }
                            on:keydown=move |ev| {
                                if ev.key() == "Enter" {
                                    ev.prevent_default();
                                    apply_link(dispatch, link_box);
                                }
                            }
                        />
                    </label>
                    <GhostButton
                        label=l!("editor.link_apply")
                        on_click=Callback::new(move |()| apply_link(dispatch, link_box))
                    />
                    <GhostButton
                        label=l!("editor.link_remove")
                        icon=Icon::Link2Off
                        tone=Tone::Danger
                        on_click=Callback::new(move |()| {
                            dispatch.run(Command::Link, Some(String::new()));
                            link_box.set(None);
                        })
                    />
                </div>
            </Show>

            // Only while the caret is in a table. Words rather than icons: these
            // are precise, infrequent operations, and there is no glyph for
            // "delete the column I am in" that anybody reads faster than the
            // sentence.
            <Show when=move || state.with(|state| state.in_table) fallback=|| ()>
                <div class="flex flex-wrap items-center gap-1 border-b border-edge px-2 py-1.5">
                    <span class="mr-1 text-2xs uppercase tracking-wide text-content-subtle">
                        {l!("editor.table")}
                    </span>
                    <GhostButton
                        label=l!("editor.table_row_add")
                        on_click=Callback::new(move |()| run(Command::AddRowAfter))
                    />
                    <GhostButton
                        label=l!("editor.table_row_remove")
                        on_click=Callback::new(move |()| run(Command::DeleteRow))
                    />
                    <GhostButton
                        label=l!("editor.table_column_add")
                        on_click=Callback::new(move |()| run(Command::AddColumnAfter))
                    />
                    <GhostButton
                        label=l!("editor.table_column_remove")
                        on_click=Callback::new(move |()| run(Command::DeleteColumn))
                    />
                    <GhostButton
                        label=l!("editor.table_remove")
                        tone=Tone::Danger
                        on_click=Callback::new(move |()| run(Command::DeleteTable))
                    />
                </div>
            </Show>

            // The mount point. It has no children here and must never be given
            // any: everything inside it belongs to ProseMirror from the moment
            // the effect runs, and Leptos patching one of those nodes is the
            // single way this arrangement comes apart.
            <div node_ref=host class="rich-text" />

            <Show when=move || !ready.get() fallback=|| ()>
                <p class="px-3 py-6 text-sm text-content-subtle">{l!("editor.loading")}</p>
            </Show>
        </div>
    }
}

fn apply_link(dispatch: Dispatch, link_box: RwSignal<Option<String>>) {
    let href = link_box.get_untracked().unwrap_or_default().trim().to_owned();
    dispatch.run(Command::Link, Some(href));
    link_box.set(None);
}

/// One toolbar button.
#[component]
fn tool(
    dispatch: Dispatch,
    command: Command,
    icon: Icon,
    #[prop(into)] label: String,
    state: RwSignal<EditorState>,
    usable: Signal<bool>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class=move || tool_class(state.with(|state| state.is_active(command)))
            // Both, and they say the same thing: `title` is the tooltip a mouse
            // gets and `aria-label` is the name a screen reader reads, and an
            // icon-only button has no text to fall back on for either.
            title=label.clone()
            aria-label=label
            aria-pressed=move || {
                if state.with(|state| state.is_active(command)) { "true" } else { "false" }
            }
            disabled=move || !usable.get()
            on:click=move |_| dispatch.run(command, None)
        >
            <Icon icon=icon size=IconSize::Sm />
        </button>
    }
}

fn tool_class(active: bool) -> String {
    let state = if active {
        "bg-brand-subtle text-brand"
    } else {
        "text-content-muted hover:bg-surface-hover hover:text-content"
    };

    format!(
        "grid size-7 shrink-0 place-items-center rounded-control transition-colors \
         disabled:pointer-events-none disabled:opacity-40 {state}"
    )
}

/// The hairline between groups of buttons.
#[component]
fn divider() -> impl IntoView {
    view! { <span class="mx-1 h-5 w-px shrink-0 bg-edge" aria-hidden="true" /> }
}
