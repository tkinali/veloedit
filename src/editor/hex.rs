//! Hex document: lazy windowed reads of a file of any size, a byte cursor,
//! overwrite edits with undo/redo, and saving as in-place patches.
//!
//! Edits never change the file size (hex editing is overwrite-only) and live in
//! a small overlay (`offset -> byte`) until saved, so a 1 GiB file with three
//! edited bytes costs three map entries. The view scrolls by whole rows with a
//! row index kept as `f64`, which stays exact far beyond any realistic file.

use std::{
    cell::Cell,
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io,
    path::Path,
};
#[cfg(unix)]
use std::os::unix::fs::FileExt;

/// `pread` / `pwrite` under the names `std::os::unix::fs::FileExt` has, for Windows
/// (`seek_read` / `seek_write`; they move the file cursor, which nothing here relies on).
#[cfg(windows)]
trait FileExt {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize>;
    fn write_all_at(&self, buf: &[u8], offset: u64) -> io::Result<()>;
}

#[cfg(windows)]
impl FileExt for File {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        std::os::windows::fs::FileExt::seek_read(self, buf, offset)
    }

    fn write_all_at(&self, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
        while !buf.is_empty() {
            match std::os::windows::fs::FileExt::seek_write(self, buf, offset) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => {
                    buf = &buf[n..];
                    offset += n as u64;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

pub const BYTES_PER_ROW: u64 = 16;
/// Rows kept in memory around the view.
pub const BUF_ROWS: u64 = 160;
const DEFAULT_VIEW_ROWS: u64 = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Hex,
    Ascii,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    RowStart,
    RowEnd,
    Start,
    End,
}

#[derive(Clone, Debug)]
pub enum HexAction {
    Click { offset: u64, area: Area },
    Move(Move),
    /// A typed hex digit (0..=15) in the hex area.
    Digit(u8),
    /// A typed byte (printable ASCII) in the ASCII area.
    Byte(u8),
    ToggleArea,
    Undo,
    Redo,
    Scroll { rows: f64 },
    ScrollTo { row: f64 },
    GoTo(u64),
}

#[derive(Clone, Copy)]
struct Step {
    offset: u64,
    before: Option<u8>,
    after: Option<u8>,
    cursor_before: (u64, bool),
    cursor_after: (u64, bool),
}

pub struct HexDoc {
    file: File,
    pub len: u64,
    pub rows: u64,
    top: f64,
    cursor: u64,
    area: Area,
    /// The next typed hex digit is the high nibble.
    high_next: bool,
    edits: BTreeMap<u64, u8>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    window_first: u64,
    window: Vec<u8>,
    /// Rows that fit the widget (written by the widget during layout).
    view_rows: Cell<u64>,
    /// When we last wrote our own patches (to ignore the file-watcher echo).
    saved_at: Option<std::time::Instant>,
}

impl HexDoc {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        let mut doc = Self {
            file,
            len,
            rows: len.div_ceil(BYTES_PER_ROW),
            top: 0.0,
            cursor: 0,
            area: Area::Hex,
            high_next: true,
            edits: BTreeMap::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            window_first: 0,
            window: Vec::new(),
            view_rows: Cell::new(DEFAULT_VIEW_ROWS),
            saved_at: None,
        };
        doc.refill(0);
        Ok(doc)
    }

    // ------------------------------------------------------------- queries

    /// Width (in hex digits) of the offset column.
    pub fn offset_digits(&self) -> usize {
        if self.len > u64::from(u32::MAX) { 16 } else { 8 }
    }

    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    pub fn area(&self) -> Area {
        self.area
    }

    pub fn high_next(&self) -> bool {
        self.high_next
    }

    pub fn dirty(&self) -> bool {
        !self.edits.is_empty()
    }

    pub fn edits(&self) -> &BTreeMap<u64, u8> {
        &self.edits
    }

    pub fn view_rows(&self) -> u64 {
        self.view_rows.get().max(1)
    }

    pub fn set_view_rows(&self, n: u64) {
        self.view_rows.set(n.max(1));
    }

    /// First row shown.
    pub fn top_row(&self) -> u64 {
        self.top as u64
    }

    /// The byte as shown: pending edit, else the file. `None` when out of range
    /// or outside the loaded window.
    pub fn byte(&self, off: u64) -> Option<u8> {
        if off >= self.len {
            return None;
        }
        self.edits.get(&off).copied().or_else(|| self.window_byte(off))
    }

    pub fn is_edited(&self, off: u64) -> bool {
        self.edits.contains_key(&off)
    }

    fn window_byte(&self, off: u64) -> Option<u8> {
        let start = self.window_first * BYTES_PER_ROW;
        off.checked_sub(start).and_then(|i| self.window.get(i as usize).copied())
    }

    /// The byte on disk (window, or a one-byte read).
    fn disk_byte(&self, off: u64) -> Option<u8> {
        self.window_byte(off).or_else(|| {
            let mut b = [0u8];
            (self.file.read_at(&mut b, off).ok()? == 1).then_some(b[0])
        })
    }

    fn current(&self, off: u64) -> u8 {
        self.edits.get(&off).copied().or_else(|| self.disk_byte(off)).unwrap_or(0)
    }

    // -------------------------------------------------------------- editing

    /// Restores edits from a saved session (ignored when the file changed size).
    pub fn set_edits(&mut self, edits: impl IntoIterator<Item = (u64, u8)>) {
        self.edits = edits.into_iter().filter(|(o, _)| *o < self.len).collect();
        self.undo.clear();
        self.redo.clear();
    }

    /// Applies an action. Returns `true` when bytes changed.
    pub fn apply(&mut self, action: HexAction) -> bool {
        let moves_view = matches!(action, HexAction::Scroll { .. } | HexAction::ScrollTo { .. } | HexAction::GoTo(_));
        let mut changed = false;
        match action {
            HexAction::Click { offset, area } => {
                if offset < self.len {
                    self.cursor = offset;
                    self.area = area;
                    self.high_next = true;
                }
            }
            HexAction::Move(m) => self.step(m),
            HexAction::ToggleArea => {
                self.area = if self.area == Area::Hex { Area::Ascii } else { Area::Hex };
                self.high_next = true;
            }
            HexAction::Digit(d) if self.len > 0 && d < 16 => {
                let cur = self.current(self.cursor);
                let before = (self.cursor, self.high_next);
                let (value, advance) = if self.high_next {
                    (d << 4 | (cur & 0x0F), false)
                } else {
                    ((cur & 0xF0) | d, true)
                };
                // Both nibbles of one byte undo together.
                self.write(self.cursor, value, before, advance);
                if advance {
                    self.cursor = (self.cursor + 1).min(self.len - 1);
                }
                self.high_next = advance;
                self.set_last_after();
                changed = true;
            }
            HexAction::Byte(b) if self.len > 0 => {
                let before = (self.cursor, self.high_next);
                self.write(self.cursor, b, before, false);
                self.cursor = (self.cursor + 1).min(self.len - 1);
                self.high_next = true;
                self.set_last_after();
                changed = true;
            }
            HexAction::Digit(_) | HexAction::Byte(_) => {}
            HexAction::Undo => changed = self.undo_step(),
            HexAction::Redo => changed = self.redo_step(),
            HexAction::Scroll { rows } => {
                self.top += rows;
                self.clamp_top();
            }
            HexAction::ScrollTo { row } => {
                self.top = row;
                self.clamp_top();
            }
            HexAction::GoTo(off) => {
                if self.len > 0 {
                    self.cursor = off.min(self.len - 1);
                    self.high_next = true;
                    let row = (self.cursor / BYTES_PER_ROW) as f64;
                    self.top = row - self.view_rows() as f64 / 2.0;
                    self.clamp_top();
                }
            }
        }
        if !moves_view {
            self.reveal_cursor();
        }
        self.ensure_window();
        changed
    }

    fn step(&mut self, m: Move) {
        if self.len == 0 {
            return;
        }
        let last = self.len - 1;
        let page = (self.view_rows().saturating_sub(1).max(1)) * BYTES_PER_ROW;
        self.cursor = match m {
            Move::Left => self.cursor.saturating_sub(1),
            Move::Right => (self.cursor + 1).min(last),
            Move::Up => self.cursor.saturating_sub(BYTES_PER_ROW),
            Move::Down => (self.cursor + BYTES_PER_ROW).min(last),
            Move::PageUp => self.cursor.saturating_sub(page),
            Move::PageDown => (self.cursor + page).min(last),
            Move::RowStart => self.cursor / BYTES_PER_ROW * BYTES_PER_ROW,
            Move::RowEnd => (self.cursor / BYTES_PER_ROW * BYTES_PER_ROW + BYTES_PER_ROW - 1).min(last),
            Move::Start => 0,
            Move::End => last,
        };
        self.high_next = true;
    }

    /// Overwrites one byte; a value equal to the file's drops the edit.
    fn write(&mut self, off: u64, value: u8, cursor_before: (u64, bool), merge_prev: bool) {
        let before = self.edits.get(&off).copied();
        if self.disk_byte(off) == Some(value) {
            self.edits.remove(&off);
        } else {
            self.edits.insert(off, value);
        }
        let after = self.edits.get(&off).copied();
        self.redo.clear();
        if merge_prev {
            if let Some(prev) = self.undo.last_mut().filter(|p| p.offset == off) {
                prev.after = after;
                return;
            }
        }
        self.undo.push(Step { offset: off, before, after, cursor_before, cursor_after: cursor_before });
    }

    fn set_last_after(&mut self) {
        let now = (self.cursor, self.high_next);
        if let Some(s) = self.undo.last_mut() {
            s.cursor_after = now;
        }
    }

    fn set_overlay(&mut self, off: u64, v: Option<u8>) {
        match v {
            Some(b) => self.edits.insert(off, b),
            None => self.edits.remove(&off),
        };
    }

    fn undo_step(&mut self) -> bool {
        let Some(s) = self.undo.pop() else { return false };
        self.set_overlay(s.offset, s.before);
        (self.cursor, self.high_next) = s.cursor_before;
        self.redo.push(s);
        true
    }

    fn redo_step(&mut self) -> bool {
        let Some(s) = self.redo.pop() else { return false };
        self.set_overlay(s.offset, s.after);
        (self.cursor, self.high_next) = s.cursor_after;
        self.undo.push(s);
        true
    }

    /// After a successful save: the written edits are now the file's content.
    pub fn finish_save(&mut self, saved: &[(u64, u8)]) {
        for &(off, b) in saved {
            if self.edits.get(&off) == Some(&b) {
                self.edits.remove(&off);
            }
        }
        self.undo.clear();
        self.redo.clear();
        self.saved_at = Some(std::time::Instant::now());
        self.refill(self.window_first);
    }

    /// Our own save echoes back through the file watcher within moments.
    pub fn recently_saved(&self) -> bool {
        self.saved_at.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(3))
    }

    // ------------------------------------------------------------ scrolling

    fn clamp_top(&mut self) {
        let max = self.rows.saturating_sub(self.view_rows() / 2) as f64;
        self.top = self.top.clamp(0.0, max.max(0.0));
    }

    /// Scrolls so the cursor row is visible.
    pub fn reveal_cursor(&mut self) {
        let row = self.cursor / BYTES_PER_ROW;
        let (top, rows) = (self.top_row(), self.view_rows());
        if row < top {
            self.top = row as f64;
        } else if row + 1 >= top + rows {
            self.top = (row + 2).saturating_sub(rows) as f64;
        }
        self.clamp_top();
    }

    fn ensure_window(&mut self) {
        let want_first = self.top_row();
        let want_rows = self.view_rows() + 2;
        let have_rows = (self.window.len() as u64).div_ceil(BYTES_PER_ROW);
        let covered = want_first >= self.window_first && want_first + want_rows <= self.window_first + have_rows;
        // A window that already reaches the end of the file is complete.
        let at_end = self.window_first + have_rows >= self.rows;
        if !covered && !(at_end && want_first >= self.window_first) {
            self.refill(want_first.saturating_sub(BUF_ROWS / 4));
        }
    }

    fn refill(&mut self, first_row: u64) {
        let mut buf = vec![0u8; (BUF_ROWS * BYTES_PER_ROW) as usize];
        let read = self.file.read_at(&mut buf, first_row * BYTES_PER_ROW).unwrap_or(0);
        buf.truncate(read);
        self.window = buf;
        self.window_first = first_row;
    }
}

/// Writes `edits` into the file in place (contiguous runs in one write each)
/// and syncs. Overwrite-only: the file never changes size.
pub fn write_patches(path: &Path, edits: &BTreeMap<u64, u8>) -> io::Result<()> {
    let file = OpenOptions::new().write(true).open(path)?;
    let mut run: Vec<u8> = Vec::new();
    let mut run_start = 0u64;
    for (&off, &b) in edits {
        if !run.is_empty() && off != run_start + run.len() as u64 {
            file.write_all_at(&run, run_start)?;
            run.clear();
        }
        if run.is_empty() {
            run_start = off;
        }
        run.push(b);
    }
    if !run.is_empty() {
        file.write_all_at(&run, run_start)?;
    }
    file.sync_all()
}

#[cfg(test)]
#[path = "hex_tests.rs"]
mod tests;
