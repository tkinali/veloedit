//! Two views of one document (split right / split down).
//!
//! A document has one [`EditorState`] that owns the text, the undo history and the view of
//! the *focused* pane (cursors, scroll, folds, wrap rows). The other pane is a second
//! `EditorState` — the *mirror* — holding a cheap clone of the rope (ropey shares its
//! nodes) and that pane's own view.
//!
//! * Focusing the other pane swaps the two views ([`EditorState::swap_view`]); everything
//!   that reads `doc.state` (find, go to line, status bar, undo ...) then follows the pane
//!   the user works in, and no editing code needs to know that a split exists.
//! * Every text change the document makes is recorded as a [`Change`] while a mirror
//!   exists, and replayed on the mirror ([`EditorState::follow`]), which also moves *its*
//!   cursors and folds along, exactly as the edit would have moved them in the document.
//! * Nothing of this costs anything without a split: no recording, no clone.

use super::*;

/// One transaction: `(position, removed text, inserted text)` per piece, ascending and
/// disjoint, in the coordinates of the text *before* the transaction (what `commit` takes).
pub type Change = Vec<(usize, String, String)>;

/// Recording is dropped (and the mirror rebuilt from scratch) beyond this many
/// unreplayed transactions — a runaway macro cannot make the log grow without bound.
const MAX_LOG: usize = 2000;

#[derive(Default)]
pub(super) struct ChangeLog {
    changes: Vec<Change>,
    /// The log is incomplete (overflow, or the text was replaced): rebuild the mirror.
    lost: bool,
}

impl EditorState {
    /// A second view of the same text, starting where this one is.
    pub fn mirror(&self) -> EditorState {
        EditorState {
            rope: self.rope.clone(),
            eol: self.eol,
            cursors: self.cursors.clone(),
            primary: self.primary,
            history: History { saved_len: Some(0), ..History::default() },
            version: self.version,
            tab_size: self.tab_size,
            scroll_x: self.scroll_x,
            scroll_y: self.scroll_y,
            metrics: Cell::new(self.metrics.get()),
            max_cols: Cell::new((u64::MAX, 0)),
            wrap: Cell::new(None),
            wrap_cache: RefCell::default(),
            folds: self.folds.clone(),
            fold_gen: self.fold_gen,
            sync: None,
        }
    }

    /// Starts (or stops) recording the changes a mirror has to replay.
    pub fn track_changes(&mut self, on: bool) {
        match (on, self.sync.is_some()) {
            (true, false) => self.sync = Some(ChangeLog::default()),
            (false, true) => self.sync = None,
            _ => {}
        }
    }

    /// Records one transaction (when someone is listening).
    pub(super) fn note_change(&mut self, change: impl FnOnce() -> Change) {
        let Some(s) = self.sync.as_mut() else { return };
        if s.lost {
            return;
        }
        if s.changes.len() >= MAX_LOG {
            s.changes.clear();
            s.lost = true;
        } else {
            s.changes.push(change());
        }
    }

    /// The text was replaced wholesale: whoever follows must start over.
    pub(super) fn note_replaced(&mut self) {
        if let Some(s) = self.sync.as_mut() {
            s.changes.clear();
            s.lost = true;
        }
    }

    /// Hands over what changed since the last call. `None` means "no complete record":
    /// the follower must resync. Starts recording if nobody was.
    pub fn take_changes(&mut self) -> Option<Vec<Change>> {
        match self.sync.as_mut() {
            Some(s) if !s.lost => Some(std::mem::take(&mut s.changes)),
            Some(s) => {
                s.lost = false;
                s.changes.clear();
                None
            }
            None => {
                self.sync = Some(ChangeLog::default());
                None
            }
        }
    }

    /// Brings `mirror` up to date with every change made here since the last call.
    pub fn feed(&mut self, mirror: &mut EditorState) {
        let changes = self.take_changes();
        mirror.follow(self, changes);
    }

    /// Exchanges the *views* of two states of the same document: cursors, scroll, folds
    /// and the layout caches that belong to them. The text, history and version stay.
    pub fn swap_view(&mut self, other: &mut EditorState) {
        std::mem::swap(&mut self.cursors, &mut other.cursors);
        std::mem::swap(&mut self.primary, &mut other.primary);
        std::mem::swap(&mut self.scroll_x, &mut other.scroll_x);
        std::mem::swap(&mut self.scroll_y, &mut other.scroll_y);
        self.metrics.swap(&other.metrics);
        self.wrap.swap(&other.wrap);
        self.wrap_cache.swap(&other.wrap_cache);
        std::mem::swap(&mut self.folds, &mut other.folds);
        std::mem::swap(&mut self.fold_gen, &mut other.fold_gen);
    }

    /// Brings this mirror's text up to date with `src` (the document's state), replaying
    /// `changes` — what `src.take_changes()` returned — and moving this view's cursors and
    /// folds along. Falls back to a full resync whenever the replay cannot be trusted.
    pub fn follow(&mut self, src: &EditorState, changes: Option<Vec<Change>>) {
        if self.tab_size != src.tab_size {
            self.tab_size = src.tab_size;
        }
        match changes {
            Some(list) if list.is_empty() && self.version == src.version && self.rope.len_chars() == src.rope.len_chars() => return,
            Some(list) => {
                for change in &list {
                    self.replay(change);
                }
                // Cheap guard (a full comparison is O(n) per keystroke): the length must agree.
                if self.rope.len_chars() != src.rope.len_chars() {
                    self.resync(src);
                }
            }
            None => self.resync(src),
        }
        debug_assert!(self.rope == src.rope, "the mirror drifted away from the document");
        self.version = src.version;
        self.eol = src.eol;
        self.clamp_scroll(true);
    }

    /// Applies one recorded transaction to the text and to this view.
    fn replay(&mut self, change: &Change) {
        let plan = self.fold_plan(change.iter().map(|(pos, removed, inserted)| (*pos, removed.as_str(), inserted.as_str())));
        let len = self.rope.len_chars();
        if change.iter().any(|(pos, removed, _)| pos + removed.chars().count() > len) {
            return; // cannot be applied; the length guard in `follow` will resync
        }
        for (pos, removed, inserted) in change.iter().rev() {
            let n = removed.chars().count();
            if n > 0 {
                self.rope.remove(*pos..*pos + n);
            }
            if !inserted.is_empty() {
                self.rope.insert(*pos, inserted);
            }
        }
        self.version += 1;
        self.fold_apply(plan);
        for c in &mut self.cursors {
            c.sel.anchor = map_through(c.sel.anchor, change);
            c.sel.head = map_through(c.sel.head, change);
        }
        self.normalize();
        self.reveal_cursors();
    }

    /// Takes the text over from `src` and keeps this view as well as it can be kept.
    fn resync(&mut self, src: &EditorState) {
        self.rope = src.rope.clone();
        self.eol = src.eol;
        self.version = src.version;
        self.folds.clear();
        self.fold_gen += 1;
        self.normalize(); // clamps the cursors to the new length
        self.reveal_cursors();
    }
}

/// Where a position ends up after `change`. A position inside replaced text lands behind
/// the new text; one exactly at the start of an edit stays in front of it.
fn map_through(pos: usize, change: &Change) -> usize {
    let mut shift = 0isize;
    for (at, removed, inserted) in change {
        let (rem, ins) = (removed.chars().count(), inserted.chars().count());
        if pos <= *at {
            break;
        }
        if pos >= at + rem {
            shift += ins as isize - rem as isize;
        } else {
            return (*at as isize + shift) as usize + ins;
        }
    }
    (pos as isize + shift) as usize
}
