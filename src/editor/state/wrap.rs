//! Soft wrapping: maps *visual rows* to (line, segment) and back.
//!
//! The editor is monospace, so wrapping is a pure function of the text and the
//! number of columns: a line is cut greedily at the last blank before the edge
//! (or hard at the edge when a word is longer than a row). Display columns stay
//! measured from the start of the *line*, so tab stops, hit-testing and the
//! non-wrapped code paths keep working unchanged; a segment only remembers the
//! column it starts at. Without wrapping every line is exactly one row, so the
//! widget uses this one row API for both modes.

use super::*;

/// Beyond this many characters wrapping switches itself off (the row table is
/// rebuilt after every edit).
pub const WRAP_MAX_CHARS: usize = 600_000;
const MIN_COLS: usize = 8;

#[derive(Default)]
pub(super) struct WrapCache {
    key: Option<(u64, usize, usize, u64)>,
    /// `first_row[i]` = visual row of line `i`'s first segment (+ one sentinel).
    first_row: Vec<usize>,
    /// Per line, the segments after the first: `(char offset, display column)`.
    breaks: Vec<Vec<(u32, u32)>>,
}

/// One visual row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowSpan {
    pub line: usize,
    /// Char offsets within the line, `end` exclusive (without the line ending).
    pub start: usize,
    pub end: usize,
    /// Display column (from the line start) where this row begins / ends;
    /// `end_col` is `None` for the last row of a line.
    pub start_col: usize,
    pub end_col: Option<usize>,
    pub first: bool,
    pub last: bool,
}

impl EditorState {
    /// Wrap at `cols` display columns (`None` = no wrapping). Cheap; call it
    /// from `layout` every frame.
    pub fn set_wrap(&self, cols: Option<usize>) {
        self.wrap.set(cols.map(|c| c.max(MIN_COLS)));
    }

    fn wrap_cols(&self) -> Option<usize> {
        self.wrap.get().filter(|_| self.len_chars() <= WRAP_MAX_CHARS)
    }

    pub fn wrapping(&self) -> bool {
        self.wrap_cols().is_some()
    }

    /// The row table exists while lines wrap *or* some are folded away (a hidden
    /// line has zero rows); otherwise a row is a line and no table is needed.
    fn with_wrap<R>(&self, f: impl FnOnce(&WrapCache) -> R) -> Option<R> {
        let cols = self.wrap_cols();
        if cols.is_none() && self.folds.is_empty() {
            return None;
        }
        let key = (self.version, cols.unwrap_or(0), self.tab_size, self.fold_gen);
        {
            let mut cache = self.wrap_cache.borrow_mut();
            if cache.key != Some(key) {
                *cache = self.build_wrap(cols);
                cache.key = Some(key);
            }
        }
        Some(f(&self.wrap_cache.borrow()))
    }

    fn build_wrap(&self, cols: Option<usize>) -> WrapCache {
        let tab = self.tab_size;
        let n = self.line_count();
        let mut first_row = Vec::with_capacity(n + 1);
        let mut breaks = Vec::with_capacity(n);
        let mut row = 0usize;
        // Folds are sorted by start: `hide_end` is one past the last line hidden by any fold seen so far.
        let (mut next_fold, mut hide_end) = (0usize, 0usize);
        for i in 0..n {
            first_row.push(row);
            while self.folds.get(next_fold).is_some_and(|f| f.start < i) {
                hide_end = hide_end.max(self.folds[next_fold].end + 1);
                next_fold += 1;
            }
            if i < hide_end {
                breaks.push(Vec::new()); // hidden: no rows at all
                continue;
            }
            let Some(cols) = cols else {
                row += 1;
                breaks.push(Vec::new());
                continue;
            };
            let chars: Vec<char> = self.rope.line(i).chars().take(self.line_len(i).min(MAX_COLS)).collect();
            let mut b: Vec<(u32, u32)> = Vec::new();
            let (mut idx, mut abs) = (0usize, 0usize);
            let (mut seg_start, mut seg_col) = (0usize, 0usize);
            let mut last_blank: Option<(usize, usize)> = None;
            while idx < chars.len() {
                let ch = chars[idx];
                let w = char_width(ch, abs, tab);
                // Blanks may hang over the edge; any other character that does
                // not fit starts a new row.
                if abs + w - seg_col > cols && abs > seg_col && !ch.is_whitespace() {
                    let (at, col) = match last_blank {
                        Some((i, c)) if i > seg_start => (i, c),
                        _ => (idx, abs),
                    };
                    b.push((at as u32, col as u32));
                    (seg_start, seg_col, last_blank) = (at, col, None);
                    (idx, abs) = (at, col);
                    continue;
                }
                abs += w;
                idx += 1;
                if ch == ' ' || ch == '\t' {
                    last_blank = Some((idx, abs));
                }
            }
            row += 1 + b.len();
            breaks.push(b);
        }
        first_row.push(row);
        WrapCache { key: None, first_row, breaks }
    }

    /// Number of visual rows.
    pub fn total_rows(&self) -> usize {
        self.with_wrap(|c| c.first_row.last().copied().unwrap_or(0)).unwrap_or_else(|| self.line_count())
    }

    /// The logical line a visual row belongs to.
    pub fn line_of_row(&self, row: usize) -> usize {
        self.row_span(row).line
    }

    /// Visual rows of one line.
    pub fn rows_of_line(&self, line: usize) -> std::ops::Range<usize> {
        self.with_wrap(|c| {
            let line = line.min(c.first_row.len().saturating_sub(2));
            c.first_row[line]..c.first_row[line + 1]
        })
        .unwrap_or(line..line + 1)
    }

    pub fn row_span(&self, row: usize) -> RowSpan {
        let non_wrapped = |me: &Self| {
            let line = row.min(me.line_count() - 1);
            RowSpan { line, start: 0, end: me.line_len(line), start_col: 0, end_col: None, first: true, last: true }
        };
        let wrapped = self.with_wrap(|c| {
            let line = (c.first_row.partition_point(|&r| r <= row).saturating_sub(1)).min(c.breaks.len() - 1);
            let seg = (row - c.first_row[line]).min(c.breaks[line].len());
            let b = &c.breaks[line];
            let (start, start_col) = if seg == 0 { (0, 0) } else { (b[seg - 1].0 as usize, b[seg - 1].1 as usize) };
            let next = b.get(seg).map(|&(o, col)| (o as usize, col as usize));
            (line, seg, start, start_col, next, b.len())
        });
        let Some((line, seg, start, start_col, next, nbreaks)) = wrapped else { return non_wrapped(self) };
        RowSpan {
            line,
            start,
            end: next.map_or_else(|| self.line_len(line), |(o, _)| o),
            start_col,
            end_col: next.map(|(_, c)| c),
            first: seg == 0,
            last: seg == nbreaks,
        }
    }

    /// `(row, display column inside the row)` of a char index.
    pub fn row_of(&self, idx: usize) -> (usize, usize) {
        let idx = idx.min(self.len_chars());
        let line = self.line_of(idx);
        let off = idx - self.line_start(line);
        let col = display_width(self.rope.line(line).chars().take(off), self.tab_size);
        let seg = self
            .with_wrap(|c| {
                let b = &c.breaks[line.min(c.breaks.len() - 1)];
                let s = b.partition_point(|&(o, _)| o as usize <= off);
                (c.first_row[line] + s, s.checked_sub(1).map_or(0, |k| b[k].1 as usize))
            })
            .unwrap_or((line, 0));
        (seg.0, col - seg.1)
    }

    /// Char index for a display column *inside* a visual row (nearest boundary).
    pub fn idx_at_row_col(&self, row: usize, col: usize) -> usize {
        let span = self.row_span(row);
        let ls = self.line_start(span.line);
        let target = span.start_col + col;
        let (mut acc, mut n) = (span.start_col, span.start);
        for ch in self.rope.line(span.line).chars().skip(span.start).take(span.end - span.start) {
            let w = char_width(ch, acc, self.tab_size);
            if target < acc + w {
                return ls + if target - acc > w / 2 { n + 1 } else { n };
            }
            acc += w;
            n += 1;
        }
        ls + n
    }

    /// Selected columns inside one visual row: `(start, end, includes_newline)`.
    pub fn selections_in_row(&self, row: usize) -> Vec<(usize, usize, bool)> {
        let sp = self.row_span(row);
        let ls = self.line_start(sp.line);
        let (rs, re) = (ls + sp.start, ls + sp.end);
        let next_line = ls + self.rope.line(sp.line).len_chars();
        let first = self.cursors.partition_point(|c| c.sel.range().1 <= rs);
        let mut out = Vec::new();
        for c in &self.cursors[first..] {
            let (a, b) = c.sel.range();
            if a >= next_line.max(re + usize::from(!sp.last)) && a >= re {
                break;
            }
            if a == b || b <= rs {
                continue;
            }
            let (from, to) = (a.max(rs).min(re), b.min(re));
            let w = |upto: usize| {
                display_width(self.rope.line(sp.line).chars().take(upto - ls), self.tab_size) - sp.start_col
            };
            let (c0, c1) = (w(from), w(to));
            let nl = sp.last && b > re && sp.line + 1 < self.line_count();
            if c1 > c0 || nl {
                out.push((c0, c1, nl));
            }
        }
        out
    }

    /// Carets inside a range of visual rows: `(row, column in row, primary)`.
    pub fn carets_in_rows(&self, rows: std::ops::Range<usize>) -> Vec<(usize, usize, bool)> {
        self.cursors
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let (row, col) = self.row_of(c.sel.head);
                rows.contains(&row).then_some((row, col, i == self.primary))
            })
            .collect()
    }
}
