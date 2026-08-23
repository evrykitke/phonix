//! Where a grid's rows come from.
//!
//! # Two answers to the same question
//!
//! A grid asks for "page 2 of 25, matching `smith`, by last sign-in". Something
//! has to answer, and there are exactly two useful places for the work to
//! happen:
//!
//! * [`Source::in_memory`] - the whole list arrives once and the browser
//!   searches, sorts and pages it. Nothing is fetched again while the viewer
//!   types, so filtering is instant and costs no round trips.
//! * [`Source::paged`] - the server is asked for each page as it is needed. The
//!   browser holds one page and never learns the rest.
//!
//! The choice is about how big the list can get, and it is not a judgement call
//! at a hundred rows and not a debate at a hundred thousand. A workspace's
//! users are in memory today; a year of stock movements will not be. What
//! matters is that **the choice is one line in the configuration** - the grid,
//! the toolbar, the pager and the export do not know which was picked.
//!
//! Search is not a separate source. A source answers a [`PageRequest`], and the
//! search text is part of it, so "the search data source" is the same function
//! with a non-empty `search`. Two functions would be two chances to disagree
//! about which rows exist.
//!
//! # A note on what crosses the wire
//!
//! Both loaders return futures because both are usually server functions. The
//! error is flattened to `String` here: a grid can display a sentence, and
//! anything richer would tie this module to one error type and stop it being
//! reusable, which is the whole point.

use std::fmt::Display;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use phonix_core::query::{Page, PageRequest};

/// A future that can be awaited by a Leptos resource on either target.
pub type Fetch<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

type LoadAll<T> = Arc<dyn Fn() -> Fetch<Vec<T>> + Send + Sync>;
type LoadPage<T> = Arc<dyn Fn(PageRequest) -> Fetch<Page<T>> + Send + Sync>;

/// How a grid gets its rows.
pub enum Source<T: 'static> {
    /// Everything, once. The browser does the searching, sorting and paging.
    InMemory(LoadAll<T>),
    /// One page at a time. The server does the work and reports the total.
    Paged(LoadPage<T>),
}

impl<T: 'static> Clone for Source<T> {
    fn clone(&self) -> Self {
        match self {
            Self::InMemory(load) => Self::InMemory(Arc::clone(load)),
            Self::Paged(load) => Self::Paged(Arc::clone(load)),
        }
    }
}

impl<T: 'static> Source<T> {
    /// Fetch the whole list once and work on it in the browser.
    ///
    /// ```ignore
    /// Source::in_memory(list_users)
    /// ```
    pub fn in_memory<F, Fut, E>(load: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Vec<T>, E>> + Send + 'static,
        E: Display + 'static,
    {
        Self::InMemory(Arc::new(move || {
            let fetch = load();

            Box::pin(async move { fetch.await.map_err(|err| err.to_string()) })
        }))
    }

    /// Ask the server for one page at a time.
    ///
    /// ```ignore
    /// Source::paged(list_requisitions)
    /// ```
    ///
    /// The server function receives the [`PageRequest`] whole - page, size,
    /// search and sort - and is responsible for sanitising it before it reaches
    /// SQL. [`PageRequest::sanitised`] is that, and a `sort.field` must be
    /// matched against known columns rather than interpolated.
    pub fn paged<F, Fut, E>(load: F) -> Self
    where
        F: Fn(PageRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Page<T>, E>> + Send + 'static,
        E: Display + 'static,
    {
        Self::Paged(Arc::new(move |request| {
            let fetch = load(request);

            Box::pin(async move { fetch.await.map_err(|err| err.to_string()) })
        }))
    }

    /// Whether this source hands over every row at once.
    ///
    /// The grid asks because two things depend on it: whether a keystroke costs
    /// a round trip, and whether an export can honestly claim to cover the
    /// whole list.
    pub const fn is_in_memory(&self) -> bool {
        matches!(self, Self::InMemory(_))
    }
}
