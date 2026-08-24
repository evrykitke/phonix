//! The frame around every screen somebody reaches before they are signed in.
//!
//! A slim top bar and a footer, wrapped around sign-in, signup, invitation
//! acceptance, password reset and the steps that finish a sign-in. It is the
//! signed-out counterpart to [`crate::components::shell`], and it is
//! deliberately much less: there is no navigation, because there is nowhere
//! this visitor may go yet.
//!
//! # Why these screens were bare, and why that was wrong
//!
//! They used to render as a form floating on an empty page. That reads as an
//! unfinished application rather than a product - and worse, it leaves nowhere
//! to put the things a signed-out visitor actually needs: which product this
//! is, which copy of it they are looking at, how to change the language, and
//! where the privacy policy is. A person who arrives at a sign-in form with no
//! surrounding page has no way to tell a real deployment from a phishing copy.
//!
//! # The layout is a column, not a page
//!
//! `min-h-dvh` with the content in a `flex-1` middle row, so the footer sits at
//! the bottom of a short page and *below* a long one, without `position:
//! fixed`. `dvh` rather than `vh` because mobile browsers shrink the viewport
//! as their address bar retracts, and `vh` leaves the footer floating a bar's
//! height up from the bottom.
//!
//! # Nothing here is a media query in Rust
//!
//! Every responsive decision below is a Tailwind class, which is CSS. The
//! server and the browser therefore render exactly the same tree at every
//! width. Choosing markup from a measured viewport is how a hydration mismatch
//! becomes a wasm panic, and a wasm panic freezes the whole application.

use leptos::prelude::*;

use crate::components::language::LanguagePicker;
use crate::l;
use crate::server_fns::public_fns::{PublicBranding, public_branding};

/// Wrap a signed-out screen in a top bar and a footer.
#[component]
pub fn public_chrome(children: Children) -> impl IntoView {
    // Blocking, and it costs nothing: `public_branding` reads `state.config`,
    // which is already in memory - no database, no network, no file.
    //
    // The first attempt mirrored the resource into a signal through an
    // `Effect`, and was wrong for a reason worth writing down: effects do not
    // run during SSR. The server rendered a footer with an empty product name,
    // so the copyright line was simply not in the HTML and appeared a moment
    // after hydration. Chrome that arrives late is worse than chrome that is
    // plain, because the page it frames visibly jumps.
    let branding = OnceResource::new_blocking(public_branding());

    view! {
        <div class="flex min-h-dvh flex-col bg-surface text-content">
            // Each bar has its own boundary, and `{children()}` is outside
            // both. A route outlet inside an async boundary stops swapping on
            // navigation - see the note in `crate::components::layout` - so the
            // one thing that must never be wrapped is the page itself.
            <Suspense fallback=|| ()>
                {move || Suspend::new(async move {
                    view! { <PublicTopBar branding=resolved(branding.await) /> }
                })}
            </Suspense>

            // `flex-1` so the middle row takes the slack and the footer is
            // pushed to the bottom of a short page. `min-w-0` because a flex
            // child defaults to min-content width, and anything wider than the
            // phone inflates the viewport and drags the whole layout sideways.
            <main class="flex w-full flex-1 flex-col items-center px-4 py-8 sm:px-6 sm:py-12">
                <div class="w-full min-w-0">{children()}</div>
            </main>

            <Suspense fallback=|| ()>
                {move || Suspend::new(async move {
                    view! { <PublicFooter branding=resolved(branding.await) /> }
                })}
            </Suspense>
        </div>
    }
}

/// The branding, or an empty one if the call failed.
///
/// A failure here means a deployment that cannot read its own configuration,
/// which is a far larger problem than a missing wordmark - and the right thing
/// for this component to do about it is still to draw the frame and get out of
/// the way. Nothing below renders an empty string as anything at all.
fn resolved(answer: Result<PublicBranding, ServerFnError>) -> PublicBranding {
    answer.unwrap_or_else(|err| {
        leptos::logging::error!("could not read the deployment's branding: {err}");
        PublicBranding::unknown()
    })
}

/// The bar across the top: which product, and which copy of it.
#[component]
fn public_top_bar(branding: PublicBranding) -> impl IntoView {
    let environment = branding.environment.clone();

    view! {
        <header class="flex h-topbar shrink-0 items-center justify-between gap-3 border-b border-edge px-4 sm:px-6">
            <Wordmark branding=branding />

            // The environment badge, and only when there is one. The final
            // production deployment sets no label and nothing renders - a badge
            // that is always there is furniture nobody reads.
            //
            // Warning-toned, because its whole job is to catch the eye of
            // somebody who thinks they are looking at the real thing. A grey
            // chip in a grey bar does not do that, and the deployment this
            // matters most for is the one running production's hardening while
            // not being production.
            //
            // `role="status"` rather than a bare span: a screen reader should
            // be told which copy of the application this is, and the visual
            // treatment says nothing to somebody who cannot see it.
            {(!environment.is_empty())
                .then(|| {
                    view! {
                        <span
                            role="status"
                            class="shrink-0 rounded-full border border-warning/40 bg-warning/15 px-2 py-0.5 text-2xs font-semibold uppercase tracking-wide text-warning"
                        >
                            {environment}
                        </span>
                    }
                })}
        </header>
    }
}

/// The mark and the product's name.
///
/// A link only when a website is configured, and a plain `<div>` otherwise - an
/// anchor with an empty `href` reloads the page, and a wordmark that silently
/// reloads the sign-in form is a bug people report as "it logged me out".
#[component]
fn wordmark(branding: PublicBranding) -> impl IntoView {
    // The first letter of the product's name, so a deployment that renames
    // itself does not keep somebody else's initial.
    let initial = branding
        .product
        .chars()
        .next()
        .map(|first| first.to_uppercase().to_string())
        .unwrap_or_default();

    let mark = view! {
        <span
            class="grid size-6 shrink-0 place-items-center rounded-control bg-brand text-2xs font-bold text-on-brand"
            aria-hidden="true"
        >
            {initial}
        </span>
    };

    // `min-w-0` and `truncate-fade`: a long product name must not be what
    // widens the viewport on a phone.
    let name = view! {
        <span class="truncate-fade text-sm font-semibold tracking-tight">
            {branding.product.clone()}
        </span>
    };

    match branding.website_url {
        Some(href) => view! {
            <a href=href class="flex min-w-0 items-center gap-2 rounded-control hover:opacity-80">
                {mark}
                {name}
            </a>
        }
        .into_any(),
        None => {
            view! { <div class="flex min-w-0 items-center gap-2">{mark} {name}</div> }.into_any()
        }
    }
}

/// Branding, the links a deployment has, and the language picker.
///
/// The picker lives here rather than on the sign-in form, which is where it
/// used to be. It belongs to the page and not to one screen: somebody who
/// cannot read the *reset* form has the same problem as somebody who cannot
/// read the sign-in form, and it was only on one of them.
#[component]
fn public_footer(branding: PublicBranding) -> impl IntoView {
    let product = branding.product.clone();
    let has_links = branding.has_links();

    view! {
        <footer class="shrink-0 border-t border-edge px-4 py-5 sm:px-6">
            // Stacked on a phone, one row from `sm` up. Not a media query in
            // Rust - the same markup at every width, laid out by CSS.
            <div class="mx-auto flex w-full max-w-long flex-col items-center gap-4 text-xs text-content-subtle sm:flex-row sm:justify-between">
                <p class="text-center sm:text-left">
                    // Nothing at all rather than a stray "©" with no name after
                    // it.
                    {(!product.is_empty()).then(|| l!("public.footer.rights", product = product))}
                </p>

                <div class="flex flex-wrap items-center justify-center gap-x-4 gap-y-2">
                    {has_links
                        .then(|| {
                            view! {
                                <FooterLink
                                    href=branding.privacy_url
                                    label=l!("public.footer.privacy")
                                />
                                <FooterLink
                                    href=branding.terms_url
                                    label=l!("public.footer.terms")
                                />
                                <FooterLink
                                    href=branding.support_url
                                    label=l!("public.footer.support")
                                />
                            }
                        })}

                    <LanguagePicker />
                </div>
            </div>
        </footer>
    }
}

/// One footer link, or nothing when this deployment has not configured it.
#[component]
fn footer_link(href: Option<String>, #[prop(into)] label: String) -> impl IntoView {
    href.map(|href| {
        view! {
            <a href=href class="hover:text-content hover:underline">
                {label}
            </a>
        }
    })
}
