//! The editing core: rope buffer, any number of selections, undo/redo history
//! and the scroll model. Pure logic - no iced types - so everything here is
//! unit-tested without a UI. The widget (`ui::editor_view`) only translates
//! input into [`Action`]s and draws what this exposes.
//!
//! Every text change is a *transaction*: a list of non-overlapping pieces
//! (one per cursor) applied together and undone together.

use std::{
    cell::{Cell, RefCell},
    time::{Duration, Instant},
};

use ropey::Rope;

mod fold;
mod split;
mod wrap;
#[cfg(test)]
pub use wrap::WRAP_MAX_CHARS;
use unicode_width::UnicodeWidthChar;

/// Max undo entries kept (older ones are dropped).
const HISTORY_CAP: usize = 2000;
/// Typing/backspacing within this window merges into one undo step.
const COALESCE: Duration = Duration::from_millis(1000);
/// Columns beyond this are neither drawn nor hit-tested (minified files).
pub const MAX_COLS: usize = 10_000;
/// Bracket matching gives up after this many characters.
const BRACKET_SCAN: usize = 50_000;
/// Safety valve for "add cursor" loops.
const MAX_CURSORS: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol {
    Lf,
    CrLf,
}

impl Eol {
    pub fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "\n",
            Eol::CrLf => "\r\n",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Eol::Lf => "LF",
            Eol::CrLf => "CRLF",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Selection {
    /// Fixed end (char index).
    pub anchor: usize,
    /// Moving end = the caret.
    pub head: usize,
}

impl Selection {
    pub fn caret(at: usize) -> Self {
        Self { anchor: at, head: at }
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.head
    }

    pub fn range(self) -> (usize, usize) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    /// Smart Home: first non-blank, then column 0.
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    PageUp,
    PageDown,
}

/// Everything the widget can ask the editor to do.
#[derive(Clone, Debug)]
pub enum Action {
    Insert(String),
    Enter,
    Backspace,
    Delete,
    DeleteWordBack,
    DeleteWordForward,
    DeleteSelection,
    Indent,
    Unindent,
    Move { motion: Motion, extend: bool },
    SelectAll,
    /// Mouse press at a (line, display column); `count` 1/2/3 = click/word/line.
    /// `add` (Ctrl+click) adds a cursor instead of replacing the selection.
    Click { line: usize, col: usize, extend: bool, count: u8, add: bool },
    Drag { line: usize, col: usize },
    /// Ctrl+D: select the word, then add the next occurrence of the selection.
    SelectNextOccurrence,
    AddCursorAbove,
    AddCursorBelow,
    /// Esc: back to a single caret.
    Collapse,
    Undo,
    Redo,
    /// Relative scroll in pixels.
    Scroll { dx: f32, dy: f32 },
    /// Absolute vertical scroll in pixels (minimap).
    ScrollTo { y: f32 },
    /// Scrollbar drag: absolute horizontal offset.
    ScrollToX { x: f32 },
    /// Code folding: the gutter arrow of `line`, Ctrl+Shift+[ / ], and the fold-all commands.
    ToggleFold { line: usize },
    /// Ctrl+wheel: font zoom by `delta` steps (handled by the app, which owns the setting).
    Zoom(i32),
    Fold,
    Unfold,
    FoldAll,
    FoldLevel(usize),
    UnfoldAll,
}

/// Layout facts the widget measures each frame and the model needs for
/// paging / scroll clamping / "keep caret visible". Written from `&self`
/// (during `layout`), hence a `Cell`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metrics {
    pub char_w: f32,
    pub line_h: f32,
    /// Height of the whole widget.
    pub view_h: f32,
    /// Width of the scrolling text area (without gutter and minimap).
    pub text_w: f32,
}

// ------------------------------------------------------------------ internals

#[derive(Clone, Copy, Debug)]
struct Cursor {
    sel: Selection,
    /// Display column kept while moving vertically.
    desired: Option<usize>,
}

impl Cursor {
    fn new(sel: Selection) -> Self {
        Self { sel, desired: None }
    }
}

/// All selections plus which one is the primary (the one the view follows).
#[derive(Clone, Debug, PartialEq)]
struct SelSet {
    sels: Vec<Selection>,
    primary: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Typing,
    Backspace,
    DeleteFwd,
    Other,
}

/// One cursor's share of a transaction. `pos` is in the coordinates of the
/// document *before* the transaction; pieces are ascending and disjoint.
struct Piece {
    pos: usize,
    removed: String,
    inserted: String,
    rem_n: usize,
    ins_n: usize,
}

impl Piece {
    fn new(pos: usize, removed: String, inserted: String) -> Self {
        let (rem_n, ins_n) = (removed.chars().count(), inserted.chars().count());
        Self { pos, removed, inserted, rem_n, ins_n }
    }

    fn is_noop(&self) -> bool {
        self.rem_n == 0 && self.ins_n == 0
    }
}

struct Edit {
    pieces: Vec<Piece>,
    before: SelSet,
    after: SelSet,
    at: Instant,
    kind: Kind,
    /// A change of the document's line-ending style `(before, after)`: undo and redo restore it
    /// together with the text.
    eol: Option<(Eol, Eol)>,
}

#[derive(Default)]
struct History {
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    /// `undo.len()` when the buffer matched the file on disk. `None` once the
    /// saved state became unreachable (edit after undoing past it).
    saved_len: Option<usize>,
}

pub struct EditorState {
    rope: Rope,
    eol: Eol,
    /// Sorted, disjoint, never empty.
    cursors: Vec<Cursor>,
    primary: usize,
    history: History,
    /// Bumps on every content change (edit, undo, redo).
    version: u64,
    tab_size: usize,
    pub scroll_x: f32,
    pub scroll_y: f32,
    metrics: Cell<Metrics>,
    /// `(version, widest line in display columns)`.
    max_cols: Cell<(u64, usize)>,
    /// Wrap columns requested by the widget (`None` = off) and the row table built for them.
    wrap: Cell<Option<usize>>,
    wrap_cache: RefCell<wrap::WrapCache>,
    /// Collapsed code folds (sorted by start line) and a counter that changes with them.
    folds: Vec<fold::Fold>,
    fold_gen: u64,
    /// Edits not yet replayed on a split view's mirror (`None` while there is no split).
    sync: Option<split::ChangeLog>,
}

impl EditorState {
    pub fn new(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            eol: detect_eol(text),
            cursors: vec![Cursor::new(Selection::default())],
            primary: 0,
            history: History { saved_len: Some(0), ..History::default() },
            version: 0,
            tab_size: 4,
            scroll_x: 0.0,
            scroll_y: 0.0,
            metrics: Cell::new(Metrics::default()),
            max_cols: Cell::new((u64::MAX, 0)),
            wrap: Cell::new(None),
            wrap_cache: RefCell::default(),
            folds: Vec::new(),
            fold_gen: 0,
            sync: None,
        }
    }

    // ------------------------------------------------------------- queries

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Byte offset of the first line (at least partly) visible at the top.
    pub fn top_byte(&self) -> usize {
        let m = self.metrics.get();
        let row = if m.line_h > 0.0 { (self.scroll_y / m.line_h).max(0.0) as usize } else { 0 };
        let row = row.min(self.total_rows().saturating_sub(1));
        self.rope.line_to_byte(self.line_of_row(row).min(self.line_count().saturating_sub(1)))
    }

    /// The text between two byte offsets (rounded down to character boundaries).
    pub fn byte_slice(&self, start: usize, end: usize) -> String {
        let end = end.min(self.rope.len_bytes());
        let start = start.min(end);
        self.rope.byte_slice(start..end).to_string()
    }

    /// 0-based line containing byte offset `byte`.
    pub fn line_of_byte(&self, byte: usize) -> usize {
        self.rope.byte_to_line(byte.min(self.rope.len_bytes()))
    }

    /// The first non-blank line as a short name for an unsaved tab: whitespace collapsed,
    /// cut to `max` characters (a `…` marks the cut). `None` while the buffer holds nothing
    /// but blanks. Looks at the first few lines only, and a bounded stretch of each, so
    /// even a huge paste costs next to nothing.
    pub fn headline(&self, max: usize) -> Option<String> {
        const LINES: usize = 64;
        const RAW: usize = 400;
        for i in 0..self.rope.len_lines().min(LINES) {
            let raw: String = self.rope.line(i).chars().take(RAW).filter(|c| c.is_whitespace() || !c.is_control()).collect();
            let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            if name.is_empty() {
                continue;
            }
            return Some(if name.chars().count() > max {
                let cut: String = name.chars().take(max.saturating_sub(1)).collect();
                format!("{}\u{2026}", cut.trim_end())
            } else {
                name
            });
        }
        None
    }

    /// Calls `f(byte offset, line without its line ending)` for every line.
    pub fn for_each_line(&self, mut f: impl FnMut(usize, &str)) {
        let mut offset = 0;
        for line in self.rope.lines() {
            let text = std::borrow::Cow::from(line);
            f(offset, text.trim_end_matches(['\n', '\r']));
            offset += line.len_bytes();
        }
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    /// Widest of the given lines in display columns (cheap: only those lines).
    pub fn widest_of(&self, lines: std::ops::Range<usize>) -> usize {
        // Callers may pass visual rows: never index past the last line.
        let n = self.line_count();
        (lines.start.min(n)..lines.end.min(n))
            .map(|i| display_width(self.rope.line(i).chars().take(self.line_len(i).min(MAX_COLS)), self.tab_size))
            .max()
            .unwrap_or(0)
    }

    /// Number of lines (a trailing newline yields a final empty line).
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn eol(&self) -> Eol {
        self.eol
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn set_version(&mut self, v: u64) {
        self.version = v;
    }

    pub fn tab_size(&self) -> usize {
        self.tab_size
    }

    pub fn set_tab_size(&mut self, n: usize) {
        self.tab_size = n.clamp(1, 16);
    }

    /// The primary selection.
    pub fn selection(&self) -> Selection {
        self.cursors[self.primary].sel
    }

    /// All selections, ascending.
    pub fn selections(&self) -> impl Iterator<Item = Selection> + '_ {
        self.cursors.iter().map(|c| c.sel)
    }

    pub fn cursor_count(&self) -> usize {
        self.cursors.len()
    }

    pub fn metrics(&self) -> Metrics {
        self.metrics.get()
    }

    pub fn set_metrics(&self, m: Metrics) {
        self.metrics.set(m);
    }

    pub fn is_dirty(&self) -> bool {
        self.history.saved_len != Some(self.history.undo.len())
    }

    /// Declares the current content as what is on disk.
    pub fn mark_saved(&mut self) {
        self.history.saved_len = Some(self.history.undo.len());
    }

    /// Text of line `i` without its line ending.
    pub fn line_string(&self, i: usize) -> String {
        if i >= self.line_count() {
            return String::new();
        }
        let s = self.rope.line(i);
        s.slice(..self.line_len(i)).to_string()
    }

    /// Chars in line `i` excluding the line ending.
    pub fn line_len(&self, i: usize) -> usize {
        if i >= self.line_count() {
            return 0;
        }
        let s = self.rope.line(i);
        let mut n = s.len_chars();
        if n > 0 && s.char(n - 1) == '\n' {
            n -= 1;
            if n > 0 && s.char(n - 1) == '\r' {
                n -= 1;
            }
        }
        n
    }

    pub fn line_start(&self, i: usize) -> usize {
        self.rope.line_to_char(i.min(self.line_count().saturating_sub(1)))
    }

    fn line_of(&self, idx: usize) -> usize {
        self.rope.char_to_line(idx.min(self.len_chars()))
    }

    /// `(line, char column)` of the primary caret, both 0-based.
    pub fn caret_line_col(&self) -> (usize, usize) {
        let head = self.selection().head;
        let line = self.line_of(head);
        (line, head - self.line_start(line))
    }

    /// `(line, display column)` of an arbitrary char index.
    pub fn line_and_display_col(&self, idx: usize) -> (usize, usize) {
        let idx = idx.min(self.len_chars());
        let line = self.line_of(idx);
        let n = idx - self.line_start(line);
        (line, display_width(self.rope.line(line).chars().take(n), self.tab_size))
    }

    /// The selected text; several selections are joined by the line ending
    /// (pasting that text into as many cursors distributes it again).
    pub fn selected_text(&self) -> Option<String> {
        let parts: Vec<String> = self
            .cursors
            .iter()
            .map(|c| c.sel.range())
            .filter(|(a, b)| a < b)
            .map(|(a, b)| self.rope.slice(a..b).to_string())
            .collect();
        (!parts.is_empty()).then(|| parts.join(self.eol.as_str()))
    }

    /// Is the character under (`line`, display column) inside a selection?
    /// (A right click inside a selection keeps it; outside it moves the caret.)
    pub fn point_in_selection(&self, line: usize, col: usize) -> bool {
        let idx = self.idx_at(line, col);
        self.cursors.iter().map(|c| c.sel.range()).any(|(a, b)| a < b && (a..b).contains(&idx))
    }

    /// Selected display columns on `line`, one entry per selection touching it:
    /// `(start, end, includes_newline)`.
    pub fn selections_on_line(&self, line: usize) -> Vec<(usize, usize, bool)> {
        let ls = self.line_start(line);
        let le = ls + self.line_len(line);
        let next = ls + self.rope.line(line.min(self.line_count() - 1)).len_chars();
        let first = self.cursors.partition_point(|c| c.sel.range().1 <= ls);
        let mut out = Vec::new();
        for c in &self.cursors[first..] {
            let (a, b) = c.sel.range();
            if a >= next {
                break;
            }
            if a == b || b <= ls {
                continue;
            }
            let from = a.max(ls).min(le);
            let to = b.min(le);
            let c0 = display_width(self.rope.line(line).chars().take(from - ls), self.tab_size);
            let c1 = display_width(self.rope.line(line).chars().take(to - ls), self.tab_size);
            let nl = b > le && line + 1 < self.line_count();
            if c1 > c0 || nl {
                out.push((c0, c1, nl));
            }
        }
        out
    }

    /// Caret positions `(line, display column)` inside the given line range
    /// (primary flag last).
    pub fn carets_in(&self, lines: std::ops::Range<usize>) -> Vec<(usize, usize, bool)> {
        let lo = self.line_start(lines.start);
        let hi = if lines.end >= self.line_count() { self.len_chars() } else { self.line_start(lines.end) };
        let first = self.cursors.partition_point(|c| c.sel.range().1 < lo);
        self.cursors[first..]
            .iter()
            .enumerate()
            .take_while(|(_, c)| c.sel.head <= hi || c.sel.range().0 <= hi)
            .filter(|(_, c)| (lo..=hi).contains(&c.sel.head))
            .map(|(i, c)| {
                let (l, col) = self.line_and_display_col(c.sel.head);
                (l, col, first + i == self.primary)
            })
            .collect()
    }

    /// The bracket matching the one at/before the primary caret, as
    /// `(this, partner)` char indices. Bounded scan; ignores strings/comments.
    pub fn matching_bracket(&self) -> Option<(usize, usize)> {
        let head = self.selection().head;
        let is_open = |c: char| matches!(c, '(' | '[' | '{');
        let is_close = |c: char| matches!(c, ')' | ']' | '}');
        let at = [head.checked_sub(1), (head < self.len_chars()).then_some(head)]
            .into_iter()
            .flatten()
            .find(|&i| {
                let c = self.rope.char(i);
                is_open(c) || is_close(c)
            })?;
        let c = self.rope.char(at);
        let want = match c {
            '(' => ')',
            '[' => ']',
            '{' => '}',
            ')' => '(',
            ']' => '[',
            _ => '{',
        };
        let mut depth = 0usize;
        if is_open(c) {
            for (n, ch) in self.rope.chars_at(at + 1).enumerate().take(BRACKET_SCAN) {
                if ch == c {
                    depth += 1;
                } else if ch == want {
                    if depth == 0 {
                        return Some((at, at + 1 + n));
                    }
                    depth -= 1;
                }
            }
        } else {
            let mut it = self.rope.chars_at(at);
            for n in 1..=BRACKET_SCAN.min(at) {
                let ch = it.prev()?;
                if ch == c {
                    depth += 1;
                } else if ch == want {
                    if depth == 0 {
                        return Some((at, at - n));
                    }
                    depth -= 1;
                }
            }
        }
        None
    }

    /// Widest line in display columns (cached per version).
    pub fn max_cols(&self) -> usize {
        let (v, n) = self.max_cols.get();
        if v == self.version {
            return n;
        }
        let w = (0..self.line_count())
            .map(|i| {
                let l = self.rope.line(i);
                let len = self.line_len(i);
                display_width(l.chars().take(len.min(MAX_COLS)), self.tab_size)
            })
            .max()
            .unwrap_or(0);
        self.max_cols.set((self.version, w));
        w
    }

    // ------------------------------------------------------------ selections

    /// Collapses to exactly this selection.
    pub fn set_selection(&mut self, sel: Selection) {
        let n = self.len_chars();
        self.cursors = vec![Cursor::new(Selection { anchor: sel.anchor.min(n), head: sel.head.min(n) })];
        self.primary = 0;
        self.reveal_cursors();
    }

    /// Selects `a..b` and brings it into view (centered when off screen).
    pub fn select_range(&mut self, a: usize, b: usize) {
        self.set_selection(Selection { anchor: a, head: b });
        let m = self.metrics.get();
        if m.line_h > 0.0 {
            let (row, _) = self.row_of(self.selection().head);
            let y = row as f32 * m.line_h;
            if y < self.scroll_y || y + m.line_h > self.scroll_y + m.view_h {
                self.scroll_y = (y - m.view_h / 2.0).max(0.0);
            }
        }
        self.scroll_into_view();
    }

    fn sel_set(&self) -> SelSet {
        SelSet { sels: self.selections().collect(), primary: self.primary }
    }

    fn set_sel_set(&mut self, s: SelSet) {
        let n = self.len_chars();
        self.cursors = s
            .sels
            .into_iter()
            .map(|x| Cursor::new(Selection { anchor: x.anchor.min(n), head: x.head.min(n) }))
            .collect();
        self.primary = s.primary.min(self.cursors.len().saturating_sub(1));
        self.normalize();
    }

    /// Sorts the cursors and merges overlapping ones (touching carets, or a
    /// caret touching a selection, merge too; two touching selections do not).
    fn normalize(&mut self) {
        let n = self.len_chars();
        for c in &mut self.cursors {
            c.sel.anchor = c.sel.anchor.min(n);
            c.sel.head = c.sel.head.min(n);
        }
        if self.cursors.len() <= 1 {
            self.primary = 0;
            return;
        }
        let primary_sel = self.cursors[self.primary].sel;
        let mut sorted = std::mem::take(&mut self.cursors);
        sorted.sort_by_key(|c| c.sel.range());
        let mut out: Vec<Cursor> = Vec::with_capacity(sorted.len());
        let mut primary = 0;
        let mut primary_found = false;
        for c in sorted {
            let is_primary = !primary_found && c.sel == primary_sel;
            if is_primary {
                primary_found = true;
            }
            if let Some(last) = out.last_mut() {
                let (a0, a1) = last.sel.range();
                let (b0, b1) = c.sel.range();
                if b0 < a1 || (b0 == a1 && (a0 == a1 || b0 == b1)) {
                    let (lo, hi) = (a0.min(b0), a1.max(b1));
                    let forward = if is_primary { c.sel.head >= c.sel.anchor } else { last.sel.head >= last.sel.anchor };
                    last.sel = if forward { Selection { anchor: lo, head: hi } } else { Selection { anchor: hi, head: lo } };
                    last.desired = None;
                    if is_primary {
                        primary = out.len() - 1;
                    }
                    continue;
                }
            }
            if is_primary {
                primary = out.len();
            }
            out.push(c);
        }
        self.cursors = out;
        self.primary = primary;
    }

    // -------------------------------------------------------------- editing

    /// Replaces the whole content (reload from disk): history is dropped.
    pub fn reset(&mut self, text: &str) {
        let (sx, sy, v, tab, m, wrap) = (self.scroll_x, self.scroll_y, self.version, self.tab_size, self.metrics.get(), self.wrap.get());
        let sync = self.sync.take();
        *self = Self::new(text);
        self.sync = sync;
        self.note_replaced();
        self.version = v + 1;
        self.tab_size = tab;
        self.metrics.set(m);
        self.wrap.set(wrap);
        self.scroll_x = sx;
        self.scroll_y = sy;
        self.clamp_scroll(false);
    }

    /// Makes the buffer hold `text` as an *undoable* edit of just the part that differs: the
    /// common beginning and end stay, so the caret, the scroll position and the folds around
    /// them survive, and `Ctrl+Z` brings the previous text back (a reload from disk must
    /// never be a way to lose work). The result counts as the saved state.
    ///
    /// `false` when the difference is too big to keep in the history; nothing was changed
    /// then and the caller starts over with [`Self::reset`].
    pub fn replace_text(&mut self, text: &str, now: Instant) -> bool {
        /// Characters (removed + inserted) one such edit may hold.
        const KEEP: usize = 1 << 20;
        let (old_len, new_len) = (self.rope.len_chars(), text.chars().count());
        let prefix = self.rope.chars().zip(text.chars()).take_while(|(a, b)| a == b).count();
        let room = old_len.min(new_len) - prefix;
        let (mut old_tail, mut new_tail) = (self.rope.chars_at(old_len).reversed(), text.chars().rev());
        let mut suffix = 0;
        while suffix < room && old_tail.next().is_some_and(|a| new_tail.next() == Some(a)) {
            suffix += 1;
        }
        if (old_len - prefix - suffix) + (new_len - prefix - suffix) > KEEP {
            return false;
        }
        let byte = |chars: usize| text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i);
        let piece = self.piece(prefix, old_len - suffix, &text[byte(prefix)..byte(new_len - suffix)]);
        let pieces = vec![piece];
        let after = self.mapped_selections(&pieces);
        self.commit(pieces, after, Kind::Other, now);
        self.eol = detect_eol(text);
        self.mark_saved();
        self.reveal_cursors();
        self.clamp_scroll(false);
        true
    }

    /// Replaces `start..end` with `text` as one undo step (find/replace),
    /// leaving a caret after the new text.
    pub fn replace_range(&mut self, start: usize, end: usize, text: &str, now: Instant) {
        let (start, end) = (start.min(self.len_chars()), end.min(self.len_chars()));
        let p = self.piece(start, end.max(start), text);
        let caret = Selection::caret(start + p.ins_n);
        self.commit(vec![p], SelSet { sels: vec![caret], primary: 0 }, Kind::Other, now);
        self.scroll_into_view();
    }

    /// Applies an action. Returns `true` when the text changed.
    pub fn apply(&mut self, action: Action, now: Instant) -> bool {
        let before = self.version;
        let keep_visible = !matches!(
            action,
            Action::Scroll { .. } | Action::ScrollTo { .. } | Action::ScrollToX { .. } | Action::ToggleFold { .. } | Action::UnfoldAll | Action::Zoom(_)
        );
        match action {
            Action::Insert(text) => self.insert_text(&text, now),
            Action::Enter => self.enter(now),
            Action::Backspace => self.backspace(now),
            Action::Delete => self.delete_forward(now),
            Action::DeleteWordBack => self.delete_word(false, now),
            Action::DeleteWordForward => self.delete_word(true, now),
            Action::DeleteSelection => {
                self.replace_each(Kind::Other, now, |_, _, sel, _, _| {
                    let (a, b) = sel.range();
                    (a < b).then(|| (a, b, String::new()))
                });
            }
            Action::Indent => self.indent(now),
            Action::Unindent => self.unindent(now),
            Action::Move { motion, extend } => self.movement(motion, extend),
            Action::SelectAll => {
                let n = self.len_chars();
                self.set_selection(Selection { anchor: 0, head: n });
            }
            Action::Click { line, col, extend, count, add } => self.click(line, col, extend, count, add),
            Action::Drag { line, col } => {
                let idx = self.idx_at(line, col);
                let p = self.primary;
                self.cursors[p].sel.head = idx;
                self.cursors[p].desired = None;
                self.normalize();
            }
            Action::SelectNextOccurrence => self.select_next_occurrence(),
            Action::AddCursorAbove => self.add_cursor(-1),
            Action::AddCursorBelow => self.add_cursor(1),
            Action::Collapse => {
                let s = self.selection();
                if self.cursors.len() > 1 {
                    self.set_selection(s);
                } else if !s.is_empty() {
                    self.set_selection(Selection::caret(s.head));
                }
            }
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::Scroll { dx, dy } => {
                self.scroll_x += dx;
                self.scroll_y += dy;
                self.clamp_scroll(dx != 0.0);
            }
            Action::ScrollTo { y } => {
                self.scroll_y = y;
                self.clamp_scroll(false);
            }
            Action::ScrollToX { x } => {
                self.scroll_x = x;
                self.clamp_scroll(true);
            }
            Action::ToggleFold { line } => self.toggle_fold(line),
            Action::Zoom(_) => {}
            Action::Fold => self.fold_at_caret(),
            Action::Unfold => self.unfold_at_caret(),
            Action::FoldAll => self.fold_all(None),
            Action::FoldLevel(n) => self.fold_all(Some(n)),
            Action::UnfoldAll => self.unfold_all(),
        }
        self.reveal_cursors();
        if keep_visible {
            self.scroll_into_view();
        }
        self.version != before
    }

    fn piece(&self, start: usize, end: usize, text: &str) -> Piece {
        let removed = if end > start { self.rope.slice(start..end).to_string() } else { String::new() };
        Piece::new(start, removed, text.to_owned())
    }

    /// Applies `pieces` (ascending, disjoint) and installs `after`. A
    /// transaction without pieces only changes the selection.
    fn commit(&mut self, pieces: Vec<Piece>, after: SelSet, kind: Kind, now: Instant) {
        let pieces: Vec<Piece> = pieces.into_iter().filter(|p| !p.is_noop()).collect();
        if pieces.is_empty() {
            self.set_sel_set(after);
            return;
        }
        let before = self.sel_set();
        let plan = self.fold_plan(pieces.iter().map(|p| (p.pos, p.removed.as_str(), p.inserted.as_str())));
        for p in pieces.iter().rev() {
            if p.rem_n > 0 {
                self.rope.remove(p.pos..p.pos + p.rem_n);
            }
            if p.ins_n > 0 {
                self.rope.insert(p.pos, &p.inserted);
            }
        }
        self.version += 1;
        self.fold_apply(plan);
        self.note_change(|| pieces.iter().map(|p| (p.pos, p.removed.clone(), p.inserted.clone())).collect());
        self.set_sel_set(after.clone());
        self.record(Edit { pieces, before, after, at: now, kind, eol: None });
    }

    /// One replacement per cursor. `make(state, index, selection, lo, hi)`
    /// returns `(start, end, text)` or `None` to leave that cursor alone; the
    /// result is clamped to `lo..hi` so pieces can never overlap. Afterwards
    /// every edited cursor is a caret behind its new text.
    fn replace_each(
        &mut self,
        kind: Kind,
        now: Instant,
        make: impl Fn(&Self, usize, Selection, usize, usize) -> Option<(usize, usize, String)>,
    ) {
        let n = self.cursors.len();
        let mut pieces = Vec::new();
        let mut after = Vec::with_capacity(n);
        let (mut shift, mut prev_end) = (0isize, 0usize);
        for i in 0..n {
            let sel = self.cursors[i].sel;
            let (_, b) = sel.range();
            let hi = self.cursors.get(i + 1).map_or(self.len_chars(), |c| c.sel.range().0);
            let moved = |s: Selection, shift: isize| Selection {
                anchor: (s.anchor as isize + shift) as usize,
                head: (s.head as isize + shift) as usize,
            };
            let piece = make(self, i, sel, prev_end, hi).map(|(s, e, text)| {
                let s = s.max(prev_end);
                let e = e.min(hi).max(s);
                self.piece(s, e, &text)
            });
            match piece {
                Some(p) if !p.is_noop() => {
                    after.push(Selection::caret((p.pos as isize + shift) as usize + p.ins_n));
                    shift += p.ins_n as isize - p.rem_n as isize;
                    prev_end = p.pos + p.rem_n;
                    pieces.push(p);
                }
                _ => {
                    after.push(moved(sel, shift));
                    prev_end = prev_end.max(b);
                }
            }
        }
        let set = SelSet { sels: after, primary: self.primary };
        self.commit(pieces, set, kind, now);
    }

    fn record(&mut self, n: Edit) {
        let h = &mut self.history;
        if h.saved_len.is_some_and(|s| s > h.undo.len()) {
            h.saved_len = None; // diverged from the saved state
        }
        h.redo.clear();
        let breaks_at_save = h.saved_len == Some(h.undo.len());
        if let Some(l) = h.undo.last_mut().filter(|l| !breaks_at_save && l.kind == n.kind && n.at.duration_since(l.at) < COALESCE) {
            if try_merge(l, &n) {
                l.after = n.after;
                l.at = n.at;
                return;
            }
        }
        h.undo.push(n);
        if h.undo.len() > HISTORY_CAP {
            h.undo.remove(0);
            h.saved_len = h.saved_len.and_then(|s| s.checked_sub(1));
        }
    }

    fn undo(&mut self) {
        let Some(e) = self.history.undo.pop() else { return };
        // Post-transaction coordinates of each piece.
        let mut delta = 0isize;
        let posts: Vec<usize> = e
            .pieces
            .iter()
            .map(|p| {
                let post = (p.pos as isize + delta) as usize;
                delta += p.ins_n as isize - p.rem_n as isize;
                post
            })
            .collect();
        // For the folds an undo removes what the edit inserted and re-inserts what it removed.
        let plan = self.fold_plan(e.pieces.iter().zip(&posts).map(|(p, &post)| (post, p.inserted.as_str(), p.removed.as_str())));
        for (p, &post) in e.pieces.iter().zip(&posts).rev() {
            if p.ins_n > 0 {
                self.rope.remove(post..post + p.ins_n);
            }
            if p.rem_n > 0 {
                self.rope.insert(post, &p.removed);
            }
        }
        self.version += 1;
        self.fold_apply(plan);
        self.note_change(|| e.pieces.iter().zip(&posts).map(|(p, &post)| (post, p.inserted.clone(), p.removed.clone())).collect());
        self.set_sel_set(e.before.clone());
        if let Some((before, _)) = e.eol {
            self.eol = before;
        }
        self.reveal_cursors();
        self.history.redo.push(e);
    }

    fn redo(&mut self) {
        let Some(e) = self.history.redo.pop() else { return };
        let plan = self.fold_plan(e.pieces.iter().map(|p| (p.pos, p.removed.as_str(), p.inserted.as_str())));
        for p in e.pieces.iter().rev() {
            if p.rem_n > 0 {
                self.rope.remove(p.pos..p.pos + p.rem_n);
            }
            if p.ins_n > 0 {
                self.rope.insert(p.pos, &p.inserted);
            }
        }
        self.version += 1;
        self.fold_apply(plan);
        self.note_change(|| e.pieces.iter().map(|p| (p.pos, p.removed.clone(), p.inserted.clone())).collect());
        self.set_sel_set(e.after.clone());
        if let Some((_, after)) = e.eol {
            self.eol = after;
        }
        self.reveal_cursors();
        self.history.undo.push(e);
    }

    fn all_empty(&self) -> bool {
        self.cursors.iter().all(|c| c.sel.is_empty())
    }

    fn insert_text(&mut self, text: &str, now: Instant) {
        if text.is_empty() {
            return;
        }
        let text = self.normalize_newlines(text);
        let n = self.cursors.len();
        // One line per cursor: paste each line into its own cursor.
        let parts: Option<Vec<String>> = (n > 1 && text.contains('\n'))
            .then(|| text.split(self.eol.as_str()).map(str::to_owned).collect())
            .filter(|p: &Vec<String>| p.len() == n);
        let kind = if self.all_empty() && text.chars().count() == 1 { Kind::Typing } else { Kind::Other };
        self.replace_each(kind, now, |_, i, sel, _, _| {
            let (a, b) = sel.range();
            let t = parts.as_ref().map_or_else(|| text.clone(), |p| p[i].clone());
            Some((a, b, t))
        });
    }

    fn normalize_newlines(&self, text: &str) -> String {
        if !text.contains(['\r', '\n']) {
            return text.to_owned();
        }
        let lf = text.replace("\r\n", "\n").replace('\r', "\n");
        match self.eol {
            Eol::Lf => lf,
            Eol::CrLf => lf.replace('\n', "\r\n"),
        }
    }

    fn enter(&mut self, now: Instant) {
        let (eol, tab) = (self.eol.as_str(), self.tab_size);
        self.replace_each(Kind::Other, now, |me, _, sel, _, _| {
            let (a, b) = sel.range();
            let line = me.line_of(a);
            let before: String = me.rope.slice(me.line_start(line)..a).to_string();
            let mut indent: String = before.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            if before.trim_end().ends_with(['{', '[', '(', ':']) {
                indent.push_str(&" ".repeat(tab));
            }
            Some((a, b, format!("{eol}{indent}")))
        });
    }

    fn backspace(&mut self, now: Instant) {
        let kind = if self.all_empty() { Kind::Backspace } else { Kind::Other };
        self.replace_each(kind, now, |me, _, sel, _, _| {
            let (a, b) = sel.range();
            if a < b {
                Some((a, b, String::new()))
            } else if a > 0 {
                Some((me.step_left(a), a, String::new()))
            } else {
                None
            }
        });
    }

    fn delete_forward(&mut self, now: Instant) {
        let kind = if self.all_empty() { Kind::DeleteFwd } else { Kind::Other };
        self.replace_each(kind, now, |me, _, sel, _, _| {
            let (a, b) = sel.range();
            if a < b {
                Some((a, b, String::new()))
            } else if a < me.len_chars() {
                Some((a, me.step_right(a), String::new()))
            } else {
                None
            }
        });
    }

    fn delete_word(&mut self, forward: bool, now: Instant) {
        self.replace_each(Kind::Other, now, |me, _, sel, _, _| {
            let (a, b) = sel.range();
            if a < b {
                Some((a, b, String::new()))
            } else if forward {
                Some((a, me.word_right(a), String::new()))
            } else {
                Some((me.word_left(a), a, String::new()))
            }
        });
    }

    /// One char left, never stopping inside a CRLF pair.
    fn step_left(&self, idx: usize) -> usize {
        if idx == 0 {
            return 0;
        }
        if self.rope.char(idx - 1) == '\n' && idx >= 2 && self.rope.char(idx - 2) == '\r' {
            idx - 2
        } else {
            idx - 1
        }
    }

    fn step_right(&self, idx: usize) -> usize {
        let len = self.len_chars();
        if idx >= len {
            return len;
        }
        if self.rope.char(idx) == '\r' && idx + 1 < len && self.rope.char(idx + 1) == '\n' {
            idx + 2
        } else {
            idx + 1
        }
    }

    // ---------------------------------------------------------- indentation

    /// Lines touched by `sel` (a selection ending at column 0 of a line does
    /// not include that line).
    fn lines_of(&self, sel: Selection) -> (usize, usize) {
        let (a, b) = sel.range();
        let first = self.line_of(a);
        let mut last = self.line_of(b);
        if b > a && last > first && b == self.line_start(last) {
            last -= 1;
        }
        (first, last)
    }

    /// Union of the lines of all non-empty selections, ascending.
    fn selected_line_set(&self) -> Vec<usize> {
        let mut lines: Vec<usize> = self
            .cursors
            .iter()
            .filter(|c| !c.sel.is_empty())
            .flat_map(|c| {
                let (f, l) = self.lines_of(c.sel);
                f..=l
            })
            .collect();
        lines.dedup();
        lines
    }

    fn indent(&mut self, now: Instant) {
        if self.all_empty() {
            // Carets: spaces up to the next tab stop, at every cursor.
            let tab = self.tab_size;
            return self.replace_each(Kind::Other, now, |me, _, sel, _, _| {
                let (_, col) = me.line_and_display_col(sel.head);
                Some((sel.head, sel.head, " ".repeat(tab - col % tab)))
            });
        }
        // Selections indent the lines they touch; they never delete text.
        let unit = " ".repeat(self.tab_size);
        let pieces: Vec<Piece> = self
            .selected_line_set()
            .into_iter()
            .filter(|&l| self.line_len(l) > 0)
            .map(|l| Piece::new(self.line_start(l), String::new(), unit.clone()))
            .collect();
        let after = self.mapped_selections(&pieces);
        self.commit(pieces, after, Kind::Other, now);
    }

    fn unindent(&mut self, now: Instant) {
        let lines: Vec<usize> = if self.all_empty() {
            let mut v: Vec<usize> = self.cursors.iter().map(|c| self.line_of(c.sel.head)).collect();
            v.dedup();
            v
        } else {
            self.selected_line_set()
        };
        let pieces: Vec<Piece> = lines
            .into_iter()
            .filter_map(|l| {
                let start = self.line_start(l);
                let line = self.rope.line(l);
                let n = if line.len_chars() > 0 && line.char(0) == '\t' {
                    1
                } else {
                    line.chars().take(self.tab_size).take_while(|c| *c == ' ').count()
                };
                (n > 0).then(|| Piece::new(start, self.rope.slice(start..start + n).to_string(), String::new()))
            })
            .collect();
        let after = self.mapped_selections(&pieces);
        self.commit(pieces, after, Kind::Other, now);
    }

    /// Removes the blanks (spaces, tabs, other horizontal white space) at the end of every
    /// line, as one undoable step; line endings stay as they are. Returns the number of
    /// lines that changed - `0` leaves the text and the history alone.
    ///
    /// `keep_breaks` is for Markdown, where two or more spaces at the end of a line that is
    /// followed by more text are a hard line break: such a line is left as it is.
    pub fn trim_trailing_whitespace(&mut self, now: Instant, keep_breaks: bool) -> usize {
        let mut pieces = Vec::new();
        for l in 0..self.line_count() {
            let len = self.line_len(l);
            if len == 0 {
                continue;
            }
            let line = self.rope.line(l);
            let mut keep = len;
            while keep > 0 && is_trailing_blank(line.char(keep - 1)) {
                keep -= 1;
            }
            if keep == len || (keep_breaks && self.is_hard_break(l, keep, len)) {
                continue;
            }
            let start = self.line_start(l);
            pieces.push(self.piece(start + keep, start + len, ""));
        }
        let n = pieces.len();
        let after = self.mapped_selections(&pieces);
        self.commit(pieces, after, Kind::Other, now);
        n
    }

    /// Rewrites the indentation of every line - the tabs and spaces it starts with - as spaces
    /// (`to_spaces`) or as tabs (a tab per `tab_size` columns, the remainder as spaces), keeping
    /// each line's indentation width. One undo step; the number of lines changed.
    pub fn convert_indentation(&mut self, to_spaces: bool, now: Instant) -> usize {
        let tab = self.tab_size.max(1);
        let mut pieces = Vec::new();
        for l in 0..self.line_count() {
            let len = self.line_len(l);
            let line = self.rope.line(l);
            let run = (0..len).take_while(|&i| matches!(line.char(i), ' ' | '\t')).count();
            if run == 0 {
                continue;
            }
            let width = (0..run).fold(0, |col, i| col + if line.char(i) == '\t' { tab - col % tab } else { 1 });
            let new = if to_spaces { " ".repeat(width) } else { format!("{}{}", "\t".repeat(width / tab), " ".repeat(width % tab)) };
            if line.slice(..run).to_string() != new {
                let start = self.line_start(l);
                pieces.push(self.piece(start, start + run, &new));
            }
        }
        let n = pieces.len();
        let after = self.mapped_selections(&pieces);
        self.commit(pieces, after, Kind::Other, now);
        n
    }

    /// Makes every line end the way `eol` says (a mixed file becomes uniform) and new lines
    /// follow it from now on. One undo step - undo brings back the old endings *and* the old
    /// style; the number of lines changed. A text with no line break at all only gets the style.
    pub fn set_eol(&mut self, eol: Eol, now: Instant) -> usize {
        let before = self.eol;
        let mut pieces = Vec::new();
        for l in 0..self.line_count() {
            let len = self.line_len(l);
            let ending = self.rope.line(l).len_chars() - len;
            let at = self.line_start(l) + len;
            match (eol, ending) {
                // "\r\n": drop the "\r".
                (Eol::Lf, 2) => pieces.push(self.piece(at, at + 1, "")),
                // "\n": put a "\r" in front of it.
                (Eol::CrLf, 1) => pieces.push(self.piece(at, at, "\r")),
                _ => {}
            }
        }
        let n = pieces.len();
        let after = self.selections_before_insertions(&pieces);
        self.commit(pieces, after, Kind::Other, now);
        if n > 0 {
            if let Some(edit) = self.history.undo.last_mut() {
                edit.eol = Some((before, eol));
            }
        }
        self.eol = eol;
        n
    }

    /// Markdown's hard line break: text, then two or more spaces (`keep..len` of line `l`,
    /// nothing but spaces), and another line of text after it - at a paragraph's end or at
    /// the end of the file the spaces mean nothing.
    fn is_hard_break(&self, l: usize, keep: usize, len: usize) -> bool {
        if keep == 0 || len - keep < 2 || l + 1 >= self.line_count() {
            return false;
        }
        let line = self.rope.line(l);
        if (keep..len).any(|i| line.char(i) != ' ') {
            return false;
        }
        let next = self.rope.line(l + 1);
        (0..self.line_len(l + 1)).any(|i| !is_trailing_blank(next.char(i)))
    }

    /// Existing selections moved through `pieces`: a selection's start stays
    /// put when text is inserted exactly there (so it also covers the new
    /// indent), its end and carets move behind it.
    fn mapped_selections(&self, pieces: &[Piece]) -> SelSet {
        let sels = self
            .cursors
            .iter()
            .map(|c| {
                let s = c.sel;
                let (a, b) = s.range();
                let (na, nb) = (map_pos(pieces, a, a == b), map_pos(pieces, b, true));
                if s.head >= s.anchor { Selection { anchor: na, head: nb } } else { Selection { anchor: nb, head: na } }
            })
            .collect();
        SelSet { sels, primary: self.primary }
    }

    /// Like [`mapped_selections`](Self::mapped_selections), but nothing moves behind text that
    /// is inserted exactly at its position. For a change of line endings: the new `\r` belongs
    /// to the line break, so a caret at a line's end stays in front of it, never between the
    /// `\r` and the `\n`.
    fn selections_before_insertions(&self, pieces: &[Piece]) -> SelSet {
        let sels = self
            .cursors
            .iter()
            .map(|c| {
                let s = c.sel;
                let (na, nb) = (map_pos(pieces, s.anchor, false), map_pos(pieces, s.head, false));
                Selection { anchor: na, head: nb }
            })
            .collect();
        SelSet { sels, primary: self.primary }
    }

    // ------------------------------------------------------------- movement

    fn movement(&mut self, motion: Motion, extend: bool) {
        let page = self.page_lines();
        for i in 0..self.cursors.len() {
            let Cursor { sel, desired } = self.cursors[i];
            let (a, b) = sel.range();
            let head = sel.head;
            let collapse = !extend && a < b;
            let mut desired = desired;
            let vertical = matches!(motion, Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown);
            let target = match motion {
                Motion::Left if collapse => a,
                Motion::Right if collapse => b,
                Motion::Left => self.step_left(head),
                Motion::Right => self.step_right(head),
                Motion::WordLeft => self.word_left(head),
                Motion::WordRight => self.word_right(head),
                Motion::LineStart => self.smart_home(head),
                Motion::LineEnd => {
                    let l = self.line_of(head);
                    self.line_start(l) + self.line_len(l)
                }
                Motion::DocStart => 0,
                Motion::DocEnd => self.len_chars(),
                Motion::Up => self.vertical(head, -1, &mut desired),
                Motion::Down => self.vertical(head, 1, &mut desired),
                Motion::PageUp => self.vertical(head, -(page as isize), &mut desired),
                Motion::PageDown => self.vertical(head, page as isize, &mut desired),
            };
            let head = target.min(self.len_chars());
            let anchor = if extend { sel.anchor } else { head };
            self.cursors[i] = Cursor { sel: Selection { anchor, head }, desired: if vertical { desired } else { None } };
        }
        self.normalize();
    }

    fn page_lines(&self) -> usize {
        let m = self.metrics.get();
        if m.line_h > 0.0 { ((m.view_h / m.line_h) as usize).saturating_sub(1).max(1) } else { 20 }
    }

    fn vertical(&self, from: usize, delta: isize, desired: &mut Option<usize>) -> usize {
        if self.wrapping() || self.has_folds() {
            // Soft wrap / folded lines: move by visual rows.
            let (row, col) = self.row_of(from);
            let col = *desired.get_or_insert(col);
            let target = row as isize + delta;
            if target < 0 {
                return 0;
            }
            if target as usize >= self.total_rows() {
                return self.len_chars();
            }
            return self.idx_at_row_col(target as usize, col);
        }
        let line = self.line_of(from);
        let col = *desired.get_or_insert_with(|| {
            display_width(self.rope.line(line).chars().take(from - self.rope.line_to_char(line)), self.tab_size)
        });
        let target = line as isize + delta;
        if target < 0 {
            return 0;
        }
        if target as usize >= self.line_count() {
            return self.len_chars();
        }
        self.idx_at(target as usize, col)
    }

    fn smart_home(&self, head: usize) -> usize {
        let l = self.line_of(head);
        let ls = self.line_start(l);
        let first_non_blank = ls
            + self.rope.line(l).chars().take(self.line_len(l)).take_while(|c| *c == ' ' || *c == '\t').count();
        if head == first_non_blank { ls } else { first_non_blank }
    }

    fn word_left(&self, from: usize) -> usize {
        let mut i = from;
        while i > 0 && class(self.rope.char(i - 1)) == Class::Space {
            i -= 1;
        }
        if i > 0 {
            let c = class(self.rope.char(i - 1));
            while i > 0 && class(self.rope.char(i - 1)) == c {
                i -= 1;
            }
        }
        i
    }

    fn word_right(&self, from: usize) -> usize {
        let len = self.len_chars();
        let mut i = from;
        if i < len {
            let c = class(self.rope.char(i));
            if c != Class::Space {
                while i < len && class(self.rope.char(i)) == c {
                    i += 1;
                }
            }
        }
        while i < len && class(self.rope.char(i)) == Class::Space {
            i += 1;
        }
        i
    }

    /// Char index for a display column on `line` (clamped to the line); a
    /// column inside a wide glyph or tab snaps to the nearest boundary.
    fn idx_at(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.line_count().saturating_sub(1));
        let ls = self.rope.line_to_char(line);
        let (mut acc, mut n) = (0usize, 0usize);
        for ch in self.rope.line(line).chars().take(self.line_len(line)) {
            let w = char_width(ch, acc, self.tab_size);
            if col < acc + w {
                return ls + if col - acc > w / 2 { n + 1 } else { n };
            }
            acc += w;
            n += 1;
        }
        ls + n
    }

    fn click(&mut self, line: usize, col: usize, extend: bool, count: u8, add: bool) {
        let idx = self.idx_at(line, col);
        let sel = match count {
            0 | 1 => {
                if extend && !add {
                    let p = self.selection();
                    Selection { anchor: p.anchor, head: idx }
                } else {
                    Selection::caret(idx)
                }
            }
            2 => {
                let (s, e) = self.word_at(idx);
                Selection { anchor: s, head: e }
            }
            _ => {
                let l = self.line_of(idx);
                let s = self.line_start(l);
                let e = if l + 1 < self.line_count() { self.line_start(l + 1) } else { self.len_chars() };
                Selection { anchor: s, head: e }
            }
        };
        if add && self.cursors.len() < MAX_CURSORS {
            self.cursors.push(Cursor::new(sel));
            self.primary = self.cursors.len() - 1;
            self.normalize();
        } else {
            self.set_selection(sel);
        }
    }

    fn word_at(&self, idx: usize) -> (usize, usize) {
        let len = self.len_chars();
        if len == 0 {
            return (0, 0);
        }
        let probe = if idx < len { idx } else { len - 1 };
        let c = class(self.rope.char(probe));
        let mut s = probe;
        while s > 0 && class(self.rope.char(s - 1)) == c {
            s -= 1;
        }
        let mut e = probe;
        while e < len && class(self.rope.char(e)) == c {
            e += 1;
        }
        (s, e)
    }

    /// Ctrl+D.
    fn select_next_occurrence(&mut self) {
        let p = self.selection();
        let (a, b) = p.range();
        if a == b {
            let (s, e) = self.word_at(a);
            if s < e && class(self.rope.char(s)) == Class::Word {
                let i = self.primary;
                self.cursors[i] = Cursor::new(Selection { anchor: s, head: e });
                self.normalize(); // other carets inside the same word merge into it
            }
            return;
        }
        let needle = self.rope.slice(a..b).to_string();
        if needle.is_empty() || self.cursors.len() >= MAX_CURSORS {
            return;
        }
        let hay = self.rope.to_string();
        let from = self.rope.char_to_byte(b);
        let taken = |c0: usize, c1: usize| self.cursors.iter().any(|c| {
            let (x, y) = c.sel.range();
            x < c1 && c0 < y
        });
        let mut found = None;
        // After the primary selection first, then wrap around to the start.
        for (lo, hi) in [(from, hay.len()), (0, from)] {
            let mut at = lo;
            while let Some(i) = hay[at..hi].find(&needle) {
                let byte = at + i;
                let (c0, c1) = (self.rope.byte_to_char(byte), self.rope.byte_to_char(byte + needle.len()));
                if !taken(c0, c1) {
                    found = Some((c0, c1));
                    break;
                }
                at = byte + needle.len().max(1);
                if at >= hi {
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        if let Some((c0, c1)) = found {
            self.cursors.push(Cursor::new(Selection { anchor: c0, head: c1 }));
            self.primary = self.cursors.len() - 1;
            self.normalize();
        }
    }

    /// Ctrl+Alt+Up/Down: a new caret above the first / below the last cursor.
    fn add_cursor(&mut self, dir: isize) {
        if self.cursors.len() >= MAX_CURSORS {
            return;
        }
        let edge = if dir < 0 { self.cursors.first() } else { self.cursors.last() }.copied().expect("never empty");
        let (line, col) = self.line_and_display_col(edge.sel.head);
        let col = edge.desired.unwrap_or(col);
        let target = line as isize + dir;
        if target < 0 || target as usize >= self.line_count() {
            return;
        }
        let idx = self.idx_at(target as usize, col);
        self.cursors.push(Cursor { sel: Selection::caret(idx), desired: Some(col) });
        self.primary = self.cursors.len() - 1;
        self.normalize();
    }

    // -------------------------------------------------------------- scrolling

    /// The font size changed: keep the same lines at the top of the view.
    pub fn rescale_scroll(&mut self, factor: f32) {
        if factor.is_finite() && factor > 0.0 {
            self.scroll_y *= factor;
            self.scroll_x *= factor;
        }
    }

    /// Largest vertical scroll offset (the last line can scroll up to a third of the view).
    pub fn max_scroll_y(&self) -> f32 {
        let m = self.metrics.get();
        if m.line_h <= 0.0 {
            return 0.0;
        }
        (self.total_rows() as f32 * m.line_h - m.view_h / 3.0).max(0.0)
    }

    /// `with_x`: also clamp horizontally (needs the widest line, which is
    /// O(lines) after an edit - so only done when actually scrolling sideways).
    fn clamp_scroll(&mut self, with_x: bool) {
        let m = self.metrics.get();
        self.scroll_x = self.scroll_x.max(0.0);
        if m.line_h <= 0.0 {
            self.scroll_y = self.scroll_y.max(0.0);
            return;
        }
        let content_h = self.total_rows() as f32 * m.line_h;
        let max_y = (content_h - m.view_h / 3.0).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max_y);
        if self.wrapping() {
            self.scroll_x = 0.0;
        } else if with_x {
            let content_w = (self.max_cols().min(MAX_COLS) as f32 + 4.0) * m.char_w;
            self.scroll_x = self.scroll_x.min((content_w - m.text_w).max(0.0));
        }
    }

    /// Scrolls so the primary caret is on screen.
    pub fn scroll_into_view(&mut self) {
        let m = self.metrics.get();
        if m.line_h <= 0.0 || m.view_h <= 0.0 {
            return;
        }
        let (row, col) = self.row_of(self.selection().head);
        let y = row as f32 * m.line_h;
        if y < self.scroll_y {
            self.scroll_y = y;
        } else if y + m.line_h > self.scroll_y + m.view_h {
            self.scroll_y = y + m.line_h - m.view_h;
        }
        let x = col.min(MAX_COLS) as f32 * m.char_w;
        let margin = m.char_w * 3.0;
        if self.wrapping() {
            self.scroll_x = 0.0;
        } else if x < self.scroll_x {
            self.scroll_x = (x - margin).max(0.0);
        } else if x + m.char_w > self.scroll_x + m.text_w {
            self.scroll_x = x + m.char_w + margin - m.text_w;
        }
        self.clamp_scroll(false);
    }
}

/// Merges transaction `n` into the previous one `l` when they are the same
/// kind of continuous typing/deleting at the same cursors.
fn try_merge(l: &mut Edit, n: &Edit) -> bool {
    if l.pieces.len() != n.pieces.len() || l.kind != n.kind {
        return false;
    }
    // Where each of `l`'s pieces sits in the coordinates *after* `l`.
    let mut shift = 0isize;
    let post: Vec<usize> = l
        .pieces
        .iter()
        .map(|p| {
            let at = (p.pos as isize + shift) as usize;
            shift += p.ins_n as isize - p.rem_n as isize;
            at
        })
        .collect();
    let ok = l.pieces.iter().zip(&n.pieces).zip(&post).all(|((lp, np), &at)| match l.kind {
        Kind::Typing => {
            let starts_ws = np.inserted.chars().next().is_some_and(char::is_whitespace);
            let l_ends_ws = lp.inserted.chars().last().is_some_and(char::is_whitespace);
            lp.rem_n == 0
                && np.rem_n == 0
                && np.pos == at + lp.ins_n
                && !np.inserted.contains('\n')
                && !lp.inserted.contains('\n')
                && (!starts_ws || l_ends_ws)
        }
        Kind::Backspace => lp.ins_n == 0 && np.ins_n == 0 && np.pos + np.rem_n == at,
        Kind::DeleteFwd => lp.ins_n == 0 && np.ins_n == 0 && np.pos == at,
        Kind::Other => false,
    });
    if !ok {
        return false;
    }
    let mut shift = 0isize;
    for (lp, np) in l.pieces.iter_mut().zip(&n.pieces) {
        match l.kind {
            Kind::Typing => {
                lp.inserted.push_str(&np.inserted);
                lp.ins_n += np.ins_n;
            }
            Kind::Backspace => {
                // `np.pos` is in post-`l` coordinates: undo earlier pieces' shift.
                lp.pos = (np.pos as isize - shift) as usize;
                lp.removed.insert_str(0, &np.removed);
                lp.rem_n += np.rem_n;
            }
            Kind::DeleteFwd => {
                lp.removed.push_str(&np.removed);
                lp.rem_n += np.rem_n;
            }
            Kind::Other => unreachable!(),
        }
        // Shift contributed by the *old* piece, as used for coordinate mapping.
        match l.kind {
            Kind::Typing => shift += (lp.ins_n - np.ins_n) as isize,
            Kind::Backspace | Kind::DeleteFwd => shift -= (lp.rem_n - np.rem_n) as isize,
            Kind::Other => {}
        }
    }
    true
}

/// Moves `pos` through `pieces` (ascending, in pre-edit coordinates).
/// `right`: text inserted exactly at `pos` goes before it (caret ends up behind).
/// White space that may be cut off a line's end: everything blank except the characters
/// that end a line (ropey counts VT, FF, NEL, LS and PS as line breaks too).
fn is_trailing_blank(c: char) -> bool {
    c == '\t' || (c.is_whitespace() && !matches!(c, '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'))
}

fn map_pos(pieces: &[Piece], pos: usize, right: bool) -> usize {
    let mut shift = 0isize;
    for p in pieces {
        let (s, e) = (p.pos, p.pos + p.rem_n);
        if s > pos {
            break;
        }
        if s == pos && p.rem_n == 0 {
            if right {
                shift += p.ins_n as isize;
                continue;
            }
            break;
        }
        if e <= pos {
            shift += p.ins_n as isize - p.rem_n as isize;
            continue;
        }
        // `pos` lies inside the removed text.
        return (s as isize + shift) as usize + if right { p.ins_n } else { 0 };
    }
    (pos as isize + shift) as usize
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    Other,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Other
    }
}

/// Width of `ch` when it starts at display column `col`.
pub fn char_width(ch: char, col: usize, tab_size: usize) -> usize {
    match ch {
        '\t' => tab_size - col % tab_size,
        c => UnicodeWidthChar::width(c).unwrap_or(1),
    }
}

/// Total display width of `chars` starting at column 0.
pub fn display_width(chars: impl Iterator<Item = char>, tab_size: usize) -> usize {
    chars.fold(0, |col, c| col + char_width(c, col, tab_size))
}

fn detect_eol(text: &str) -> Eol {
    let head = &text.as_bytes()[..text.len().min(64 * 1024)];
    let crlf = head.windows(2).filter(|w| w == b"\r\n").count();
    let lf = head.iter().filter(|&&b| b == b'\n').count();
    if crlf > 0 && crlf * 2 >= lf { Eol::CrLf } else { Eol::Lf }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
