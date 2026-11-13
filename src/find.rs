//! Find/replace bar state and the pure navigation rules. The searching itself
//! lives in `search`; the app glues both to the editor and the worker tasks.

use std::sync::Arc;

use crate::{editor::TabId, search::Query};

/// Which buffer/version/query a result list was computed for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub tab: TabId,
    pub version: u64,
    pub query: Query,
}

#[derive(Default)]
pub struct Find {
    pub open: bool,
    /// Show the replace row.
    pub replace: bool,
    pub query: Query,
    pub replacement: String,
    /// `(start, end)` char ranges, ascending.
    pub matches: Arc<Vec<(usize, usize)>>,
    /// The key `matches` belongs to (`None` = nothing computed).
    pub matches_key: Option<Key>,
    /// Key of the search currently running.
    pub requested: Option<Key>,
    pub error: Option<String>,
    /// Select the next match as soon as results arrive (typing in the box).
    pub jump: bool,
}

impl Find {
    /// The matches, only when they describe exactly this buffer state.
    pub fn valid_matches(&self, tab: TabId, version: u64) -> Option<&[(usize, usize)]> {
        self.matches_key
            .as_ref()
            .filter(|k| k.tab == tab && k.version == version && k.query == self.query)
            .map(|_| self.matches.as_slice())
    }

    pub fn wants_search(&self, tab: TabId, version: u64) -> bool {
        let key = Key { tab, version, query: self.query.clone() };
        self.open && self.requested.as_ref() != Some(&key) && self.matches_key.as_ref() != Some(&key)
    }

    pub fn key(&self, tab: TabId, version: u64) -> Key {
        Key { tab, version, query: self.query.clone() }
    }
}

/// Index of the match to go to from `selection` (`a..b`, ascending order).
/// `forward`: next match; otherwise previous. Wraps around.
pub fn step(matches: &[(usize, usize)], (a, b): (usize, usize), forward: bool) -> Option<usize> {
    let n = matches.len();
    if n == 0 {
        return None;
    }
    let exact = matches.binary_search_by(|m| m.0.cmp(&a)).ok().filter(|&i| matches[i].1 == b);
    Some(match (forward, exact) {
        (true, Some(i)) => (i + 1) % n,
        (false, Some(i)) => (i + n - 1) % n,
        (true, None) => {
            let from = if a == b { a } else { b };
            let i = matches.partition_point(|m| m.0 < from);
            if i == n { 0 } else { i }
        }
        (false, None) => match matches.partition_point(|m| m.1 <= a) {
            0 => n - 1,
            i => i - 1,
        },
    })
}

/// The index of the match equal to the selection, if there is one.
pub fn current(matches: &[(usize, usize)], (a, b): (usize, usize)) -> Option<usize> {
    matches.binary_search_by(|m| m.0.cmp(&a)).ok().filter(|&i| matches[i].1 == b)
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: [(usize, usize); 3] = [(2, 4), (10, 12), (20, 22)];

    #[test]
    fn next_and_previous_wrap_and_respect_the_selection() {
        // A caret between matches.
        assert_eq!(step(&M, (5, 5), true), Some(1));
        assert_eq!(step(&M, (5, 5), false), Some(0));
        // Caret before the first / after the last: wrap.
        assert_eq!(step(&M, (0, 0), false), Some(2));
        assert_eq!(step(&M, (30, 30), true), Some(0));
        // A selected match steps to its neighbour (and wraps).
        assert_eq!(step(&M, (10, 12), true), Some(2));
        assert_eq!(step(&M, (20, 22), true), Some(0));
        assert_eq!(step(&M, (2, 4), false), Some(2));
        // A caret *at* a match start finds that match going forward.
        assert_eq!(step(&M, (10, 10), true), Some(1));
        // A non-matching selection counts from its end going forward.
        assert_eq!(step(&M, (3, 11), true), Some(2));
        assert_eq!(step(&[], (0, 0), true), None);
        assert_eq!(current(&M, (10, 12)), Some(1));
        assert_eq!(current(&M, (10, 11)), None);
    }

    #[test]
    fn matches_are_only_valid_for_their_own_buffer_state() {
        let mut f = Find { open: true, query: Query { text: "x".into(), ..Query::default() }, ..Find::default() };
        assert!(f.wants_search(1, 5));
        f.matches = Arc::new(vec![(0, 1)]);
        f.matches_key = Some(f.key(1, 5));
        assert!(f.valid_matches(1, 5).is_some());
        assert!(f.valid_matches(1, 6).is_none(), "edited since");
        assert!(f.valid_matches(2, 5).is_none(), "other tab");
        assert!(!f.wants_search(1, 5));
        assert!(f.wants_search(1, 6));
        f.query.case_sensitive = true;
        assert!(f.valid_matches(1, 5).is_none(), "other options");
        f.requested = Some(f.key(1, 6));
        assert!(!f.wants_search(1, 6), "already running");
    }
}
