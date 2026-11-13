//! Open documents and the tab strip that owns them.

pub mod hex;
pub mod state;
pub mod image;
pub mod loader;
pub mod preview;
pub mod text;

use std::path::{Path, PathBuf};

pub use hex::HexDoc;
pub use image::ImageDoc;
pub use text::TextDoc;

pub type TabId = u64;

/// How a split tab lays out its two panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDir {
    /// Side by side.
    Right,
    /// One above the other.
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    /// Left / top.
    First,
    /// Right / bottom.
    Second,
}

/// A second view of the tab's text (see `state/split.rs`).
pub struct Split {
    pub dir: SplitDir,
    /// The pane the user works in: the document's own state holds *its* view.
    pub focus: Pane,
    /// The other pane's view of the same text.
    pub mirror: state::EditorState,
    /// Share of the area the first pane gets.
    pub frac: f32,
}

pub const SPLIT_MIN: f32 = 0.15;
pub const SPLIT_MAX: f32 = 0.85;

/// A tab shows exactly one kind of document view.
pub enum Doc {
    Text(TextDoc),
    Hex(HexDoc),
    Image(ImageDoc),
}

/// Longest name derived from the text, in the tab strip and in lists / dialogs / window title.
pub const TAB_LABEL: usize = 28;
pub const LONG_LABEL: usize = 60;

pub struct Tab {
    pub id: TabId,
    pub path: Option<PathBuf>,
    pub title: String,
    pub doc: Doc,
    /// Two panes on one text (text tabs only).
    pub split: Option<Split>,
    /// A preview tab: opened by a single click in the sidebar, replaced by the next such
    /// click until it is pinned (double click, edit, ...). Shown in italics.
    pub preview: bool,
}

impl Tab {
    pub fn is_dirty(&self) -> bool {
        match &self.doc {
            Doc::Text(t) => t.dirty(),
            Doc::Hex(h) => h.dirty(),
            Doc::Image(_) => false,
        }
    }

    /// What the tab is called on screen, at most `max` characters for a derived name.
    /// A saved file shows its file name; a never-saved text buffer shows its first
    /// non-blank line (like Sublime Text), or "untitled" while it is empty. `title`
    /// stays the stable name (the save dialog suggests it).
    pub fn label(&self, max: usize) -> std::borrow::Cow<'_, str> {
        if self.path.is_none() {
            if let Doc::Text(t) = &self.doc {
                if let Some(name) = t.headline(max) {
                    return name.into();
                }
            }
        }
        (&self.title).into()
    }

    /// The state to draw in `pane`: the document's own holds the focused pane's view.
    pub fn pane_state(&self, pane: Pane) -> Option<&state::EditorState> {
        let Doc::Text(doc) = &self.doc else { return None };
        match &self.split {
            Some(s) if s.focus != pane => Some(&s.mirror),
            _ => Some(&doc.state),
        }
    }

    /// Makes `pane` the one the document's state (cursor, find, undo ...) works on.
    pub fn focus_pane(&mut self, pane: Pane) {
        if let (Doc::Text(doc), Some(s)) = (&mut self.doc, &mut self.split) {
            if s.focus != pane {
                doc.state.swap_view(&mut s.mirror);
                s.focus = pane;
            }
        }
    }

    /// Splits the text view in two (or turns an existing split). Text tabs only.
    pub fn split(&mut self, dir: SplitDir) {
        let Doc::Text(doc) = &mut self.doc else { return };
        match &mut self.split {
            Some(s) => s.dir = dir,
            None => {
                doc.state.track_changes(true);
                self.split = Some(Split { dir, focus: Pane::First, mirror: doc.state.mirror(), frac: 0.5 });
            }
        }
    }

    /// Back to one pane; the focused pane's view stays.
    pub fn close_split(&mut self) {
        if let (Doc::Text(doc), Some(_)) = (&mut self.doc, self.split.take()) {
            doc.state.track_changes(false);
        }
    }

    /// Replays the document's latest changes on the second pane.
    pub fn sync_split(&mut self) {
        if let (Doc::Text(doc), Some(s)) = (&mut self.doc, &mut self.split) {
            doc.state.feed(&mut s.mirror);
        }
    }

    pub fn has_conflict(&self) -> bool {
        matches!(&self.doc, Doc::Text(t) if t.conflict)
    }
}

#[derive(Default)]
pub struct TabSet {
    tabs: Vec<Tab>,
    active: Option<TabId>,
    next_id: TabId,
    untitled_seq: u32,
}

impl TabSet {
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.tabs.iter_mut()
    }

    pub fn active_id(&self) -> Option<TabId> {
        self.active
    }

    pub fn active(&self) -> Option<&Tab> {
        self.get(self.active?)
    }

    /// Moves tab `id` into gap `slot` (`0` = before the first tab, `len` = after the last,
    /// counted before the tab leaves its place). `false` when nothing changes.
    pub fn move_to_slot(&mut self, id: TabId, slot: usize) -> bool {
        let Some(from) = self.tabs.iter().position(|t| t.id == id) else { return false };
        let to = if slot > from { slot - 1 } else { slot }.min(self.tabs.len() - 1);
        if to == from {
            return false;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        true
    }

    /// The tab a sidebar click may replace: the preview tab, if it holds no unsaved work.
    pub fn replaceable_preview(&self) -> Option<TabId> {
        self.tabs.iter().find(|t| t.preview && !t.is_dirty()).map(|t| t.id)
    }

    pub fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    pub fn active_mut(&mut self) -> Option<&mut Tab> {
        self.get_mut(self.active?)
    }

    pub fn get(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    pub fn get_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn by_path(&self, path: &Path) -> Option<TabId> {
        self.tabs
            .iter()
            .find(|t| t.path.as_deref() == Some(path))
            .map(|t| t.id)
    }

    pub fn activate(&mut self, id: TabId) {
        if self.get(id).is_some() {
            self.active = Some(id);
        }
    }

    pub fn activate_index(&mut self, idx: usize) {
        if let Some(t) = self.tabs.get(idx) {
            self.active = Some(t.id);
        }
    }

    pub fn active_index(&self) -> Option<usize> {
        let id = self.active?;
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Adds a tab at `index` (or at the end) and returns its id.
    pub fn insert(&mut self, index: Option<usize>, path: Option<PathBuf>, doc: Doc) -> TabId {
        self.next_id += 1;
        let id = self.next_id;
        let title = match &path {
            Some(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string()),
            None => {
                self.untitled_seq += 1;
                if self.untitled_seq == 1 {
                    "untitled".to_owned()
                } else {
                    format!("untitled {}", self.untitled_seq)
                }
            }
        };
        let tab = Tab { id, path, title, doc, split: None, preview: false };
        match index {
            Some(i) if i <= self.tabs.len() => self.tabs.insert(i, tab),
            _ => self.tabs.push(tab),
        }
        id
    }

    pub fn set_path(&mut self, id: TabId, path: PathBuf) {
        if let Some(t) = self.get_mut(id) {
            t.title = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            t.path = Some(path);
        }
    }

    /// Removes a tab; focus moves to its right neighbour, else the left one.
    pub fn close(&mut self, id: TabId) -> Option<Tab> {
        let idx = self.tabs.iter().position(|t| t.id == id)?;
        let tab = self.tabs.remove(idx);
        if self.active == Some(id) {
            self.active = self
                .tabs
                .get(idx)
                .or_else(|| idx.checked_sub(1).and_then(|i| self.tabs.get(i)))
                .map(|t| t.id);
        }
        Some(tab)
    }

    /// Cycles the active tab by `delta` (wrapping).
    pub fn cycle(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        if n == 0 {
            return;
        }
        let cur = self.active_index().unwrap_or(0) as isize;
        self.activate_index((cur + delta).rem_euclid(n) as usize);
    }
}
