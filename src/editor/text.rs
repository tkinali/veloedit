//! Text document state (wraps iced's `text_editor::Content`).

use std::sync::Arc;

use crate::{
    editor::state::EditorState,
    git::GitState,
    syntax::{Lang, LineSpans, ParseState},
};

/// Syntax-highlighting state of one buffer (parse tree + spans live here;
/// the work happens on a worker, see `App::start_highlight`).
pub struct Highlight {
    pub lang: Option<Lang>,
    /// Previous parse: lets the next one be incremental.
    pub parse: Option<ParseState>,
    pub spans: Option<Arc<LineSpans>>,
    /// Buffer version last sent to the worker (`u64::MAX` = never).
    pub requested: u64,
    pub in_flight: bool,
    /// Too large, or the grammar failed to load.
    pub disabled: bool,
}

impl Default for Highlight {
    fn default() -> Self {
        Self { lang: None, parse: None, spans: None, requested: u64::MAX, in_flight: false, disabled: false }
    }
}

impl Highlight {
    pub fn set_lang(&mut self, lang: Option<Lang>) {
        if self.lang != lang {
            *self = Highlight { lang, ..Highlight::default() };
        }
    }

    /// Needs a (re)parse for buffer `version`?
    pub fn wants(&self, version: u64) -> bool {
        self.lang.is_some() && !self.disabled && !self.in_flight && self.requested != version
    }
}

/// Stable content hash used to tell "file changed by someone else" apart from
/// "file changed by our own save".
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// How long after a save the watcher's events about the file are taken for its echo.
const SAVE_ECHO: std::time::Duration = std::time::Duration::from_millis(2500);

pub struct TextDoc {
    pub state: EditorState,
    /// Dirty regardless of undo history (restored unsaved drafts) until saved.
    pub force_dirty: bool,
    /// A save of this document is being written (see `App::fs_changed`).
    pub saving: bool,
    /// When the last save finished.
    pub saved_at: Option<std::time::Instant>,
    /// `state.version()` that was last written to the draft store.
    pub flushed_version: u64,
    /// Hash of the file as last read from / written to disk (0 = unknown).
    pub disk_hash: u64,
    /// The file changed on disk while this buffer has unsaved edits.
    pub conflict: bool,
    pub hl: Highlight,
    pub git: GitState,
    /// Only for Markdown documents while the preview is shown.
    pub preview: Option<super::preview::Preview>,
}

impl TextDoc {
    pub fn new(text: &str, dirty: bool, disk_hash: u64) -> Self {
        let mut state = EditorState::new(text);
        if dirty {
            // Restored buffers start at version 1 so the first draft flush persists them.
            state.set_version(1);
        }
        Self {
            state,
            force_dirty: dirty,
            saving: false,
            saved_at: None,
            flushed_version: 0,
            disk_hash,
            conflict: false,
            hl: Highlight::default(),
            git: GitState::default(),
            preview: None,
        }
    }

    pub fn with_lang(mut self, lang: Option<Lang>) -> Self {
        self.hl.set_lang(lang);
        self
    }

    /// Empty, never-saved buffer. Version 1 so its (empty) draft gets persisted.
    pub fn untitled() -> Self {
        let mut doc = Self::new("", false, 0);
        doc.state.set_version(1);
        doc
    }

    /// Unsaved changes (or a restored draft that was never saved).
    pub fn dirty(&self) -> bool {
        self.force_dirty || self.state.is_dirty()
    }

    /// Bumps on every content change, undo and redo included.
    pub fn version(&self) -> u64 {
        self.state.version()
    }

    pub fn text(&self) -> String {
        self.state.text()
    }

    /// Cuts the blanks off every line's end (one undo step); the number of lines changed.
    /// `markdown` keeps the two-space hard line breaks.
    pub fn trim_trailing_whitespace(&mut self, markdown: bool) -> usize {
        self.state.trim_trailing_whitespace(std::time::Instant::now(), markdown)
    }

    /// First non-blank line, shortened: what a never-saved tab is called.
    pub fn headline(&self, max: usize) -> Option<String> {
        self.state.headline(max)
    }

    /// Replaces the buffer with fresh disk content. The change is an undo step of its own
    /// (`Ctrl+Z` gets the previous text back); only a huge difference starts over without
    /// history.
    pub fn replace(&mut self, text: &str, disk_hash: u64) {
        if !self.state.replace_text(text, std::time::Instant::now()) {
            self.state.reset(text);
        }
        self.force_dirty = false;
        self.conflict = false;
        self.disk_hash = disk_hash;
    }

    /// True while a save is under way or has only just finished: the watcher's echo of our
    /// own write is no reason to read the file back (over a network mount that read can
    /// return a stale, even empty, view of it).
    pub fn just_saved(&self) -> bool {
        self.saving || self.saved_at.is_some_and(|t| t.elapsed() < SAVE_ECHO)
    }

    /// The content now equals what was written to disk.
    pub fn mark_saved(&mut self, disk_hash: u64) {
        self.state.mark_saved();
        self.force_dirty = false;
        self.conflict = false;
        self.disk_hash = disk_hash;
    }

    /// 1-based `(line, column)` of the caret (columns count characters).
    pub fn caret(&self) -> (usize, usize) {
        let (l, c) = self.state.caret_line_col();
        (l + 1, c + 1)
    }
}
