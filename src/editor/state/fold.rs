//! Code folding, for every language: a block is *the lines indented deeper than
//! the line that opens it* (blank lines don't count). That is what Sublime Text
//! does for languages without a fold grammar, and for brace languages it is the
//! same thing in practice. When the line after the block is only a closer (`}`,
//! `)`, `</div>`, `end`, ...) it folds together with the block and is shown after
//! the `…` marker, like `function f() { … }`.
//!
//! Collapsed folds are stored as line ranges and hidden through the same row
//! table soft wrap uses (a hidden line simply has zero rows), so scrolling,
//! hit-testing and vertical movement need no special cases. Edits move the
//! ranges along and unfold what they touch.

use super::*;

/// Never scan further than this many lines for one block / one "fold all".
const MAX_SCAN: usize = 400_000;
/// How many blank lines in a row are skipped when looking for the block's first line.
const MAX_BLANK_RUN: usize = 2_000;

/// A collapsed block: lines `start + 1 ..= end` are hidden; `start` stays visible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold {
    pub start: usize,
    pub end: usize,
    /// The last hidden line is the block's closer (`}` ...), drawn after the `…`.
    pub closer: bool,
}

fn is_closer_text(line: &str) -> bool {
    let t = line.trim().trim_end_matches([',', ';']).trim_end();
    !t.is_empty()
        && (t.chars().all(|c| matches!(c, '}' | ')' | ']'))
            || t.starts_with("</")
            || matches!(
                t,
                "end" | "endif" | "endfor" | "endforeach" | "endwhile" | "endswitch" | "endfunction" | "endclass" | "fi" | "done" | "esac"
            ))
}

impl EditorState {
    // ------------------------------------------------------------- queries

    /// Indentation of a line in display columns; `None` for a blank line.
    fn indent_of(&self, line: usize) -> Option<usize> {
        let mut col = 0;
        for ch in self.rope.line(line).chars() {
            match ch {
                '\n' | '\r' => return None,
                c if c.is_whitespace() => col += char_width(c, col, self.tab_size),
                _ => return Some(col),
            }
        }
        None
    }

    /// Does a block start at `line`? (Cheap: only looks at the next non-blank line.)
    pub fn is_foldable(&self, line: usize) -> bool {
        let count = self.line_count();
        let Some(ind) = (line < count).then(|| self.indent_of(line)).flatten() else { return false };
        let limit = (line + 1 + MAX_BLANK_RUN).min(count);
        (line + 1..limit).find_map(|n| self.indent_of(n)).is_some_and(|d| d > ind)
    }

    /// The block that starts at `line`, if there is one.
    pub fn fold_range(&self, line: usize) -> Option<Fold> {
        let count = self.line_count().min(MAX_SCAN);
        let ind = self.indent_of(line)?;
        let mut n = line + 1;
        while n < count && n < line + 1 + MAX_BLANK_RUN && self.indent_of(n).is_none() {
            n += 1;
        }
        if n >= count || self.indent_of(n)? <= ind {
            return None;
        }
        let mut last = n;
        let mut m = n + 1;
        while m < count {
            match self.indent_of(m) {
                None => {}
                Some(d) if d > ind => last = m,
                Some(_) => break,
            }
            m += 1;
        }
        let closer = m < count && self.indent_of(m) == Some(ind) && is_closer_text(&self.line_string(m));
        Some(Fold { start: line, end: if closer { m } else { last }, closer })
    }

    pub fn has_folds(&self) -> bool {
        !self.folds.is_empty()
    }

    pub fn fold_gen(&self) -> u64 {
        self.fold_gen
    }

    /// The collapsed fold that starts at `line`.
    pub fn fold_at(&self, line: usize) -> Option<Fold> {
        let i = self.folds.partition_point(|f| f.start < line);
        self.folds.get(i).filter(|f| f.start == line).copied()
    }

    /// Is `line` hidden inside a collapsed fold?
    pub fn line_hidden(&self, line: usize) -> bool {
        self.folds.iter().any(|f| f.start < line && line <= f.end)
    }

    /// Can the fold arrow of `line` be clicked (collapsed, or a block starts there)?
    pub fn fold_toggleable(&self, line: usize) -> bool {
        self.fold_at(line).is_some() || self.is_foldable(line)
    }

    // ----------------------------------------------------------- commands

    fn set_folds(&mut self, folds: Vec<Fold>) {
        self.folds = folds;
        self.fold_gen += 1;
        self.park_cursors_above_folds();
    }

    /// Collapses the block that starts at `line`. Returns whether anything changed.
    pub fn fold_line(&mut self, line: usize) -> bool {
        if self.fold_at(line).is_some() {
            return false;
        }
        let Some(f) = self.fold_range(line) else { return false };
        let mut folds = self.folds.clone();
        let at = folds.partition_point(|x| x.start < f.start);
        folds.insert(at, f);
        self.set_folds(folds);
        true
    }

    pub fn unfold_line(&mut self, line: usize) -> bool {
        let before = self.folds.len();
        let folds: Vec<Fold> = self.folds.iter().copied().filter(|f| f.start != line).collect();
        if folds.len() == before {
            return false;
        }
        self.set_folds(folds);
        true
    }

    pub fn toggle_fold(&mut self, line: usize) {
        if !self.unfold_line(line) {
            self.fold_line(line);
        }
    }

    /// Ctrl+Shift+[ : collapses the innermost block around the caret.
    pub fn fold_at_caret(&mut self) {
        let (line, _) = self.caret_line_col();
        let lo = line.saturating_sub(5_000);
        for l in (lo..=line).rev() {
            if self.fold_at(l).is_some() {
                // Already collapsed: fold the enclosing block instead.
                continue;
            }
            if self.is_foldable(l) && self.fold_range(l).is_some_and(|f| f.end >= line) {
                self.fold_line(l);
                return;
            }
        }
    }

    /// Ctrl+Shift+] : expands the fold on the caret's line.
    pub fn unfold_at_caret(&mut self) {
        let line = self.caret_line_col().0;
        self.unfold_line(line);
    }

    /// Every block of the document with its nesting depth (1 = outermost).
    fn all_blocks(&self) -> Vec<(Fold, usize)> {
        let n = self.line_count().min(MAX_SCAN);
        let mut out = Vec::new();
        let mut open_ends: Vec<usize> = Vec::new();
        for line in 0..n {
            while open_ends.last().is_some_and(|&e| e < line) {
                open_ends.pop();
            }
            if self.is_foldable(line) {
                if let Some(f) = self.fold_range(line) {
                    out.push((f, open_ends.len() + 1));
                    open_ends.push(f.end);
                }
            }
        }
        out
    }

    /// Collapses every block (`level: None`) or only those at nesting depth `level`.
    pub fn fold_all(&mut self, level: Option<usize>) {
        let folds = self
            .all_blocks()
            .into_iter()
            .filter(|(_, depth)| level.is_none_or(|l| l == *depth))
            .map(|(f, _)| f)
            .collect();
        self.set_folds(folds);
    }

    pub fn unfold_all(&mut self) {
        if !self.folds.is_empty() {
            self.set_folds(Vec::new());
        }
    }

    // ------------------------------------------------------------- upkeep

    /// A caret hidden by a *new* fold moves to the end of the line that stays visible.
    fn park_cursors_above_folds(&mut self) {
        let hidden_top = |me: &Self, line: usize| me.folds.iter().filter(|f| f.start < line && line <= f.end).map(|f| f.start).min();
        let mut moved = false;
        for i in 0..self.cursors.len() {
            let (a, b) = (self.cursors[i].sel.anchor, self.cursors[i].sel.head);
            let (la, lb) = (self.line_of(a), self.line_of(b));
            let top = [hidden_top(self, la), hidden_top(self, lb)].into_iter().flatten().min();
            if let Some(start) = top {
                let at = self.line_start(start) + self.line_len(start);
                self.cursors[i] = Cursor::new(Selection::caret(at));
                moved = true;
            }
        }
        if moved {
            self.normalize();
        }
    }

    /// Anything the user can put a caret in must be visible: unfold what hides a cursor.
    pub(super) fn reveal_cursors(&mut self) {
        if self.folds.is_empty() {
            return;
        }
        let lines: Vec<usize> = self
            .cursors
            .iter()
            .flat_map(|c| [self.line_of(c.sel.anchor), self.line_of(c.sel.head)])
            .collect();
        let keep: Vec<Fold> = self
            .folds
            .iter()
            .copied()
            .filter(|f| !lines.iter().any(|&l| f.start < l && l <= f.end))
            .collect();
        if keep.len() != self.folds.len() {
            self.folds = keep;
            self.fold_gen += 1;
        }
    }

    /// Per edit piece (in the text's coordinates *before* the edit): first line touched,
    /// newlines removed, newlines inserted. Empty when nothing is folded.
    pub(super) fn fold_plan<'a>(&self, pieces: impl Iterator<Item = (usize, &'a str, &'a str)>) -> Vec<(usize, usize, usize)> {
        if self.folds.is_empty() {
            return Vec::new();
        }
        pieces
            .map(|(pos, removed, inserted)| {
                (self.line_of(pos), removed.matches('\n').count(), inserted.matches('\n').count())
            })
            .collect()
    }

    /// Moves the folds along with the edit described by `plan` (made by [`fold_plan`]
    /// before the text changed); a fold the edit touches is dropped.
    pub(super) fn fold_apply(&mut self, plan: Vec<(usize, usize, usize)>) {
        if plan.is_empty() || self.folds.is_empty() {
            return;
        }
        let lines = self.line_count();
        for (sl, rl, al) in plan.into_iter().rev() {
            let edit_last = sl + rl;
            self.folds.retain_mut(|f| {
                if f.end < sl {
                    return true; // before the edit
                }
                if f.start > edit_last {
                    // after the edit: the line numbers move
                    f.start = (f.start + al).saturating_sub(rl);
                    f.end = (f.end + al).saturating_sub(rl);
                    return true;
                }
                // Typing inside the header line keeps the fold; anything else that touches it unfolds.
                sl == f.start && rl == 0 && al == 0
            });
        }
        self.folds.retain(|f| f.end < lines && f.start < f.end);
        self.folds.sort_by_key(|f| f.start);
        self.fold_gen += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHP: &str = "<?php\nclass A\n{\n    public function f($x)\n    {\n        if ($x) {\n            return 1;\n        }\n\n        return 2;\n    }\n\n    public function g()\n    {\n        return 3;\n    }\n}\n";

    fn state(text: &str) -> EditorState {
        EditorState::new(text)
    }

    #[test]
    fn blocks_are_the_lines_indented_deeper_and_a_lone_closer_folds_with_them() {
        let s = state(PHP);
        // `{` of the class (line 2): body + closing brace.
        assert_eq!(s.fold_range(2), Some(Fold { start: 2, end: 16, closer: true }));
        // Allman-style method: the `{` line is the header.
        assert_eq!(s.fold_range(4), Some(Fold { start: 4, end: 10, closer: true }));
        // `if ($x) {` ... `}`; blank lines inside do not end or break a block.
        assert_eq!(s.fold_range(5), Some(Fold { start: 5, end: 7, closer: true }));
        // Lines without a deeper next line are not foldable.
        for l in [0, 1, 6, 9, 12] {
            assert!(!s.is_foldable(l), "line {l}");
            assert_eq!(s.fold_range(l), None);
        }
        assert!(s.is_foldable(2) && s.is_foldable(4) && s.is_foldable(5));
        // Python: no closer, the block ends before the dedent.
        let py = state("def f():\n    a = 1\n\n    b = 2\n\nx = 3\n");
        assert_eq!(py.fold_range(0), Some(Fold { start: 0, end: 3, closer: false }));
        // Tabs count as tab_size columns.
        let tabs = state("a\n\tb\n\t\tc\n\td\n");
        assert_eq!(tabs.fold_range(0).map(|f| f.end), Some(3));
        assert_eq!(tabs.fold_range(1).map(|f| f.end), Some(2));
    }

    #[test]
    fn folding_hides_lines_from_the_row_model_and_unfolding_restores_them() {
        let mut s = state(PHP);
        let rows = s.total_rows();
        assert_eq!(rows, s.line_count());
        assert!(s.fold_line(4));
        assert!(s.has_folds() && s.fold_at(4).is_some() && s.line_hidden(7) && !s.line_hidden(4) && !s.line_hidden(11));
        assert_eq!(s.total_rows(), rows - 6, "lines 5..=10 hide, the header stays: {}", s.total_rows());
        // Row <-> line mapping skips the hidden lines.
        assert_eq!(s.line_of_row(4), 4);
        assert_eq!(s.line_of_row(5), 11, "the row after the folded header is the blank line 11");
        assert_eq!(s.rows_of_line(7), 5..5, "hidden lines own no rows");
        // Folding twice is a no-op; toggling expands.
        assert!(!s.fold_line(4));
        s.toggle_fold(4);
        assert!(!s.has_folds() && s.total_rows() == rows);
        // Fold all / by level / unfold all.
        s.fold_all(None);
        assert!(s.fold_at(2).is_some() && s.fold_at(4).is_some() && s.fold_at(5).is_some());
        s.fold_all(Some(2));
        assert!(s.fold_at(2).is_none() && s.fold_at(4).is_some() && s.fold_at(13).is_some() && s.fold_at(5).is_none());
        s.unfold_all();
        assert!(!s.has_folds());
    }

    #[test]
    fn the_caret_never_stays_inside_a_fold_and_movement_skips_hidden_lines() {
        let mut s = state(PHP);
        // Caret on line 6 (inside the method), then fold the method: caret goes to the header.
        let at = s.line_start(6) + 3;
        s.set_selection(Selection::caret(at));
        s.fold_at_caret();
        assert!(s.fold_at(5).is_some() || s.fold_at(4).is_some(), "the innermost block around the caret");
        let (line, _) = s.caret_line_col();
        assert!(!s.line_hidden(line), "caret is on a visible line");
        // Down from the folded header lands on the first line after the fold, not inside it.
        let mut s = state(PHP);
        s.fold_line(4);
        s.set_selection(Selection::caret(s.line_start(4)));
        s.apply(Action::Move { motion: Motion::Down, extend: false }, Instant::now());
        assert_eq!(s.caret_line_col().0, 11);
        // Jumping into hidden text (find, go to line) unfolds exactly what hides it.
        s.fold_line(2);
        assert!(s.fold_at(2).is_some());
        s.select_range(s.line_start(6), s.line_start(6) + 2);
        assert!(s.fold_at(2).is_none(), "outer fold opened");
        assert!(s.fold_at(4).is_none() || !s.line_hidden(6), "the line is visible now");
    }

    #[test]
    fn edits_move_folds_along_and_touching_one_unfolds_it() {
        let now = Instant::now();
        let mut s = state(PHP);
        s.fold_line(4);
        // Insert a line far above: the fold moves down with its block.
        s.set_selection(Selection::caret(0));
        s.apply(Action::Insert("// header\n".into()), now);
        assert_eq!(s.fold_at(5).map(|f| (f.start, f.end)), Some((5, 11)));
        assert!(s.fold_at(4).is_none());
        // Typing on the header line keeps it folded...
        let end_of_header = s.line_start(5) + s.line_len(5);
        s.set_selection(Selection::caret(end_of_header));
        s.apply(Action::Insert(" // note".into()), now);
        assert!(s.fold_at(5).is_some());
        // ...an edit below it does not touch it...
        let tail = s.len_chars();
        s.set_selection(Selection::caret(tail));
        s.apply(Action::Insert("// tail\n".into()), now);
        assert!(s.fold_at(5).is_some());
        // ...and undo/redo move it along as well.
        s.apply(Action::Undo, now);
        s.apply(Action::Undo, now);
        s.apply(Action::Undo, now);
        assert_eq!(s.fold_at(4).map(|f| f.end), Some(10), "back where it was after undoing the header line");
        s.apply(Action::Redo, now);
        assert_eq!(s.fold_at(5).map(|f| f.end), Some(11));
        // Replacing text inside the hidden block (replace-all) drops the fold.
        let inside = s.line_start(8);
        s.replace_range(inside, inside + 2, "zz", now);
        assert!(!s.has_folds());
    }
}
