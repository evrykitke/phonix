//! Cards: a bordered block that holds one thing.
//!
//! # The collapsible one
//!
//! [`CollapsibleCard`] wears the same clothes as the shortcut tiles on an app's
//! front page - icon, title, a line of explanation - but instead of going
//! somewhere it opens. That resemblance is the point: a page can mix "go here"
//! and "the rest of it is in here" without the eye having to learn two shapes.
//!
//! It is **closed when it arrives**, always. A stack of cards that opened
//! itself would be a stack of headings with a page of prose between each pair,
//! which is the layout the card exists to avoid; and the one thing somebody
//! wants is the one they click.
//!
//! # Why `<details>` and not a signal
//!
//! Openness here is a property of one node with no consequences anywhere else,
//! and the platform already has that element. Taking it means the keyboard, the
//! accessibility tree, `Ctrl+F` in Chromium and the `open` attribute in a
//! printed page all work without a line of code - and, the reason that decided
//! it, nothing about the markup differs between the server's render and the
//! browser's. A `RwSignal<bool>` starting `false` on both sides would hydrate
//! correctly too, but it is a hydration question that has to keep being
//! answered correctly, and this one cannot be got wrong.
//!
//! The cost is that a caller cannot open a card from outside - no "expand all",
//! no opening one because a search matched inside it. When something needs
//! that, it wants a different component and not a prop on this one: a
//! controlled disclosure and an uncontrolled one behave differently under
//! every interaction, and one component pretending to be both is how a kit
//! grows a `controlled: bool`.
//!
//! ```ignore
//! <CollapsibleCard
//!     title=l!("ui.editor.title")
//!     detail=l!("ui.editor.detail")
//!     icon=Icon::Pencil
//!     meta="v0".to_owned()
//! >
//!     <p>"Whatever the card is hiding."</p>
//! </CollapsibleCard>
//! ```

use leptos::prelude::*;

use crate::icons::{Icon, IconSize};

/// A card that opens.
///
/// See the [module documentation](self) for why openness is an attribute
/// rather than a signal, and why there is no way to open it from outside.
#[component]
pub fn collapsible_card(
    #[prop(into)] title: String,
    /// The line under the title. A sentence, not a label - this is the part
    /// somebody reads to decide whether to open it.
    #[prop(optional, into)]
    detail: Option<String>,
    #[prop(optional)] icon: Option<Icon>,
    /// A short word at the right of the header: a count, a version, a state.
    #[prop(optional, into)]
    meta: Option<String>,
    /// Start open. The default - closed - is the one to reach for; see the
    /// module docs. This exists for the single card on a page that *is* the
    /// page, where arriving closed would be a page with nothing on it.
    #[prop(optional)]
    open: bool,
    children: Children,
) -> impl IntoView {
    view! {
        // `group` so the chevron and the icon tile can answer to the card's
        // own `[open]` rather than each carrying state.
        <details class="group rounded-card border border-edge bg-surface-raised" open=open>
            <summary class="flex cursor-pointer items-start gap-3 rounded-card p-4 hover:bg-surface-hover group-open:rounded-b-none">
                {icon
                    .map(|icon| {
                        view! {
                            <span class="grid size-9 shrink-0 place-items-center rounded-control bg-surface-sunken text-content-muted transition-colors group-open:bg-brand-subtle group-open:text-brand">
                                <Icon icon=icon size=IconSize::Sm />
                            </span>
                        }
                    })}

                <span class="min-w-0 flex-1">
                    <span class="block text-sm font-medium text-content">{title}</span>
                    {detail
                        .map(|detail| {
                            view! {
                                <span class="mt-0.5 block text-xs leading-relaxed text-content-muted">
                                    {detail}
                                </span>
                            }
                        })}
                </span>

                {meta
                    .map(|meta| {
                        view! {
                            <span class="mt-0.5 shrink-0 font-mono text-2xs text-content-subtle">
                                {meta}
                            </span>
                        }
                    })}

                // Decoration: the summary is already announced as a disclosure
                // and already says whether it is expanded.
                <span
                    class="mt-0.5 shrink-0 text-content-subtle transition-transform duration-150 group-open:rotate-180"
                    aria-hidden="true"
                >
                    <Icon icon=Icon::ChevronDown size=IconSize::Xs />
                </span>
            </summary>

            // Divided from the header rather than floated below it: the border
            // is what stops an open card reading as two cards.
            <div class="border-t border-edge px-4 py-4">{children()}</div>
        </details>
    }
}
