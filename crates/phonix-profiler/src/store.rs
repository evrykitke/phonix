//! Where profiles live: a bounded ring, in memory, for this process only.
//!
//! See `docs/adr/0004-development-profiler.md` section 5 for why it is not
//! files, and what would change that.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::profile::{Profile, Token};

/// How many profiles are kept when nothing says otherwise.
///
/// A few hundred is several minutes of clicking around, and a page load is
/// worth roughly a dozen rows once assets are counted.
pub const DEFAULT_CAPACITY: usize = 250;

/// The ring, plus the counter that mints tokens.
#[derive(Debug)]
pub struct Store {
    profiles: Mutex<VecDeque<Arc<Profile>>>,
    capacity: usize,
    next: AtomicU64,
}

impl Store {
    /// A store holding at most `capacity` profiles.
    ///
    /// A capacity of zero is accepted and means "collect nothing", which is a
    /// coherent thing to configure and cheaper than a second flag.
    pub fn new(capacity: usize) -> Self {
        Self {
            profiles: Mutex::new(VecDeque::with_capacity(capacity.min(DEFAULT_CAPACITY))),
            capacity,
            next: AtomicU64::new(seed()),
        }
    }

    /// The next token.
    ///
    /// `Relaxed` because nothing is ordered against this - the only property
    /// required is that two threads never get the same number.
    pub fn mint(&self) -> Token {
        Token(self.next.fetch_add(1, Ordering::Relaxed))
    }

    /// File a finished profile, evicting the oldest if the ring is full.
    pub fn push(&self, profile: Profile) {
        if self.capacity == 0 {
            return;
        }

        let mut profiles = self.lock();

        while profiles.len() >= self.capacity {
            profiles.pop_front();
        }

        profiles.push_back(Arc::new(profile));
    }

    /// One profile by its token, if it has not been evicted yet.
    pub fn get(&self, token: Token) -> Option<Arc<Profile>> {
        self.lock()
            .iter()
            .rev()
            .find(|profile| profile.token == token)
            .map(Arc::clone)
    }

    /// The most recent profiles, newest first.
    pub fn recent(&self, limit: usize) -> Vec<Arc<Profile>> {
        self.lock().iter().rev().take(limit).map(Arc::clone).collect()
    }

    /// Every profile belonging to one page load, oldest first.
    ///
    /// Oldest first because a page load is read as a sequence: the document,
    /// then what it asked for. The index is newest first for the opposite
    /// reason - there, the last thing that happened is the thing being
    /// debugged.
    pub fn page(&self, page: &str) -> Vec<Arc<Profile>> {
        self.lock()
            .iter()
            .filter(|profile| profile.page.as_deref() == Some(page))
            .map(Arc::clone)
            .collect()
    }

    /// How many profiles are held.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Forget everything.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// The ring, with a poisoned lock treated as usable.
    ///
    /// A panic in a handler poisons this mutex, and a poisoned profiler that
    /// refuses to answer is the worst possible behaviour: the panic is exactly
    /// what somebody opened the profiler to look at. The data is a queue of
    /// immutable records, so there is no invariant a panic could have left
    /// half-applied.
    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<Arc<Profile>>> {
        self.profiles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A per-process starting point for tokens.
///
/// So that a restart - which `cargo leptos watch` does on every save - hands
/// out a different range, and a token the browser is still holding resolves to
/// "gone" rather than to some unrelated request that happens to have reused
/// the number.
fn seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Kind;
    use chrono::Utc;
    use std::time::Duration;

    fn profile(store: &Store, path: &str, page: Option<&str>) -> Profile {
        Profile {
            token: store.mint(),
            at: Utc::now(),
            kind: Kind::of(path),
            method: "GET".into(),
            path: path.into(),
            query_string: None,
            route: None,
            status: 200,
            duration: Duration::from_millis(1),
            tenant: None,
            page: page.map(str::to_owned),
            response_bytes: None,
            queries: Vec::new(),
            logs: Vec::new(),
            rss_bytes: None,
        }
    }

    #[test]
    fn tokens_are_unique() {
        let store = Store::new(4);

        assert_ne!(store.mint(), store.mint());
    }

    #[test]
    fn the_ring_evicts_the_oldest() {
        let store = Store::new(2);
        let first = profile(&store, "/one", None);
        let first_token = first.token;

        store.push(first);
        store.push(profile(&store, "/two", None));
        store.push(profile(&store, "/three", None));

        assert_eq!(store.len(), 2, "the cap is a cap");
        assert!(store.get(first_token).is_none(), "the oldest is gone");
    }

    /// A capacity of zero has to mean "keep nothing" rather than "keep
    /// everything", which is what an unchecked `while len >= 0` loop would do
    /// after popping the queue empty.
    #[test]
    fn a_capacity_of_zero_collects_nothing() {
        let store = Store::new(0);

        store.push(profile(&store, "/one", None));

        assert!(store.is_empty());
    }

    #[test]
    fn a_page_gathers_only_its_own_requests() {
        let store = Store::new(8);

        store.push(profile(&store, "/admin/users", Some("p1")));
        store.push(profile(&store, "/api/users/list", Some("p1")));
        store.push(profile(&store, "/api/roles/list", Some("p2")));
        store.push(profile(&store, "/favicon.svg", None));

        assert_eq!(store.page("p1").len(), 2);
        assert_eq!(store.page("p2").len(), 1);
    }

    /// Oldest first, because a page load is read as a sequence.
    #[test]
    fn a_page_is_returned_in_the_order_it_happened() {
        let store = Store::new(8);

        store.push(profile(&store, "/admin/users", Some("p1")));
        store.push(profile(&store, "/api/users/list", Some("p1")));

        let page = store.page("p1");

        assert_eq!(page.first().map(|p| p.path.as_str()), Some("/admin/users"));
    }

    #[test]
    fn recent_is_newest_first() {
        let store = Store::new(8);

        store.push(profile(&store, "/one", None));
        store.push(profile(&store, "/two", None));

        let recent = store.recent(10);

        assert_eq!(recent.first().map(|p| p.path.as_str()), Some("/two"));
    }
}
