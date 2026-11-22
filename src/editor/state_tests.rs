use std::time::{Duration, Instant};

use super::*;

fn st(text: &str) -> EditorState {
    EditorState::new(text)
}

fn ins(s: &mut EditorState, t: &str, now: Instant) {
    s.apply(Action::Insert(t.into()), now);
}

fn mv(s: &mut EditorState, motion: Motion, extend: bool) {
    s.apply(Action::Move { motion, extend }, Instant::now());
}

fn caret(s: &EditorState) -> (usize, usize) {
    s.caret_line_col()
}

fn select(s: &mut EditorState, a: usize, b: usize) {
    s.set_selection(Selection { anchor: a, head: b });
}

#[test]
fn typing_replaces_selection_and_undo_is_one_step() {
    let now = Instant::now();
    let mut s = st("hello world");
    select(&mut s, 6, 11);
    ins(&mut s, "x", now);
    assert_eq!(s.text(), "hello x");
    assert!(s.is_dirty());
    s.apply(Action::Undo, now);
    assert_eq!(s.text(), "hello world");
    assert_eq!(s.selection(), Selection { anchor: 6, head: 11 }, "selection restored");
    assert!(!s.is_dirty(), "undo back to the loaded state is clean");
    s.apply(Action::Redo, now);
    assert_eq!(s.text(), "hello x");
}

#[test]
fn typing_coalesces_by_word_and_time() {
    let t0 = Instant::now();
    let mut s = st("");
    for (i, c) in "foo bar".chars().enumerate() {
        ins(&mut s, &c.to_string(), t0 + Duration::from_millis(i as u64 * 10));
    }
    // "foo" | " bar"  -> two undo steps
    s.apply(Action::Undo, t0);
    assert_eq!(s.text(), "foo");
    s.apply(Action::Undo, t0);
    assert_eq!(s.text(), "");

    // A pause longer than the window starts a new step.
    ins(&mut s, "a", t0);
    ins(&mut s, "b", t0 + Duration::from_secs(5));
    s.apply(Action::Undo, t0);
    assert_eq!(s.text(), "a");
}

#[test]
fn backspace_and_delete_runs_coalesce() {
    let t0 = Instant::now();
    let mut s = st("abcdef");
    s.set_selection(Selection::caret(6));
    for i in 0..3 {
        s.apply(Action::Backspace, t0 + Duration::from_millis(i));
    }
    assert_eq!(s.text(), "abc");
    s.apply(Action::Undo, t0);
    assert_eq!(s.text(), "abcdef", "three backspaces undo together");

    s.set_selection(Selection::caret(0));
    for i in 0..2 {
        s.apply(Action::Delete, t0 + Duration::from_millis(i));
    }
    assert_eq!(s.text(), "cdef");
    s.apply(Action::Undo, t0);
    assert_eq!(s.text(), "abcdef");
}

#[test]
fn dirty_tracking_follows_save_undo_and_divergence() {
    let now = Instant::now();
    let mut s = st("a");
    s.set_selection(Selection::caret(1));
    ins(&mut s, "b", now);
    s.mark_saved();
    assert!(!s.is_dirty());
    // Typing right after saving must not merge into the saved step.
    ins(&mut s, "c", now);
    assert!(s.is_dirty());
    s.apply(Action::Undo, now);
    assert_eq!(s.text(), "ab");
    assert!(!s.is_dirty(), "undo to the saved point is clean again");
    // Undo past the save point, then edit: the saved state is unreachable.
    s.apply(Action::Undo, now);
    ins(&mut s, "z", now);
    s.apply(Action::Redo, now);
    s.apply(Action::Undo, now);
    assert!(s.is_dirty());
}

#[test]
fn cursor_motions_words_lines_and_pages() {
    let mut s = st("foo.bar baz\n  indented\n\nlast");
    mv(&mut s, Motion::WordRight, false);
    assert_eq!(caret(&s), (0, 3), "word then punctuation are separate stops");
    mv(&mut s, Motion::WordRight, false);
    mv(&mut s, Motion::WordRight, false);
    assert_eq!(caret(&s), (0, 8), "skips the space after the word");
    mv(&mut s, Motion::WordLeft, false);
    assert_eq!(caret(&s), (0, 4));

    mv(&mut s, Motion::Down, false);
    assert_eq!(caret(&s), (1, 4));
    mv(&mut s, Motion::Down, false); // empty line: clamps, remembers column 4
    assert_eq!(caret(&s), (2, 0));
    mv(&mut s, Motion::Down, false);
    assert_eq!(caret(&s), (3, 4), "desired column restored");
    mv(&mut s, Motion::Down, false);
    assert_eq!(caret(&s), (3, 4), "last line: moves to its end");

    mv(&mut s, Motion::DocStart, false);
    mv(&mut s, Motion::Down, false);
    mv(&mut s, Motion::LineStart, false);
    assert_eq!(caret(&s), (1, 2), "smart home: first non-blank");
    mv(&mut s, Motion::LineStart, false);
    assert_eq!(caret(&s), (1, 0), "second press: column 0");
    mv(&mut s, Motion::LineEnd, true);
    assert_eq!(s.selected_text().as_deref(), Some("  indented"));
}

#[test]
fn horizontal_moves_collapse_selection_and_cross_crlf_atomically() {
    let mut s = st("ab\r\ncd");
    assert_eq!(s.eol(), Eol::CrLf);
    select(&mut s, 0, 2);
    mv(&mut s, Motion::Right, false);
    assert_eq!(s.selection(), Selection::caret(2), "Right collapses to the end");
    mv(&mut s, Motion::Right, false);
    assert_eq!(s.selection().head, 4, "steps over \\r\\n as one");
    mv(&mut s, Motion::Left, false);
    assert_eq!(s.selection().head, 2);
    s.set_selection(Selection::caret(4));
    s.apply(Action::Backspace, Instant::now());
    assert_eq!(s.text(), "abcd", "backspace joins lines, removing both bytes");
}

#[test]
fn enter_keeps_indentation_and_opens_blocks() {
    let now = Instant::now();
    let mut s = st("    let a = 1;");
    s.set_selection(Selection::caret(s.len_chars()));
    s.apply(Action::Enter, now);
    assert_eq!(s.text(), "    let a = 1;\n    ");
    let mut s = st("fn main() {");
    s.set_selection(Selection::caret(s.len_chars()));
    s.apply(Action::Enter, now);
    assert_eq!(s.text(), "fn main() {\n    ");
    assert_eq!(caret(&s), (1, 4));
    let mut s = st("a\r\nb");
    s.set_selection(Selection::caret(1));
    s.apply(Action::Enter, now);
    assert_eq!(s.text(), "a\r\n\r\nb", "CRLF documents get CRLF");
}

#[test]
fn paste_normalizes_line_endings_to_the_document_style() {
    let now = Instant::now();
    let mut s = st("x\n");
    s.set_selection(Selection::caret(1));
    ins(&mut s, "1\r\n2\r3", now);
    assert_eq!(s.text(), "x1\n2\n3\n");
    let mut s = st("x\r\n");
    s.set_selection(Selection::caret(1));
    ins(&mut s, "1\n2", now);
    assert_eq!(s.text(), "x1\r\n2\r\n");
}

#[test]
fn tab_indents_lines_never_deletes_and_shift_tab_undoes_it() {
    let now = Instant::now();
    let mut s = st("a\n\nb\nc");
    s.set_selection(Selection::caret(1));
    s.apply(Action::Indent, now);
    assert_eq!(s.text(), "a   \n\nb\nc", "caret: spaces to the next tab stop");

    let mut s = st("a\n\nb\nc");
    select(&mut s, 0, 5); // a, empty, b
    s.apply(Action::Indent, now);
    assert_eq!(s.text(), "    a\n\n    b\nc", "empty lines stay empty");
    assert_eq!(s.selected_text().as_deref(), Some("    a\n\n    b\n"), "selection still covers the same lines");
    s.apply(Action::Unindent, now);
    assert_eq!(s.text(), "a\n\nb\nc");

    // Whole-line selection (triple click) indents instead of replacing.
    let mut s = st("one\ntwo\n");
    select(&mut s, 0, 4);
    s.apply(Action::Indent, now);
    assert_eq!(s.text(), "    one\ntwo\n");

    let mut s = st("\tx\n  y");
    select(&mut s, 0, 6);
    s.apply(Action::Unindent, now);
    assert_eq!(s.text(), "x\ny", "tab or up to tab_size spaces");
}

#[test]
fn mouse_selection_word_line_and_wide_glyphs() {
    let mut s = st("héllo wörld\n\tfoo\n日本 x");
    // Double click inside "wörld".
    s.apply(Action::Click { line: 0, col: 8, extend: false, count: 2, add: false }, Instant::now());
    assert_eq!(s.selected_text().as_deref(), Some("wörld"));
    // Triple click selects the line including its newline.
    s.apply(Action::Click { line: 1, col: 5, extend: false, count: 3, add: false }, Instant::now());
    assert_eq!(s.selected_text().as_deref(), Some("\tfoo\n"));
    // Tab = 4 columns: col 4 is right after it.
    s.apply(Action::Click { line: 1, col: 4, extend: false, count: 1, add: false }, Instant::now());
    assert_eq!(caret(&s), (1, 1));
    // Wide glyphs take two columns; a click in the middle snaps to the nearest edge.
    s.apply(Action::Click { line: 2, col: 2, extend: false, count: 1, add: false }, Instant::now());
    assert_eq!(caret(&s), (2, 1));
    s.apply(Action::Click { line: 2, col: 3, extend: false, count: 1, add: false }, Instant::now());
    assert_eq!(caret(&s), (2, 1), "3 is closer to the boundary at 2 than at 4? tie -> before");
    // Drag extends from the anchor; shift-click too.
    s.apply(Action::Click { line: 0, col: 1, extend: false, count: 1, add: false }, Instant::now());
    s.apply(Action::Drag { line: 0, col: 4 }, Instant::now());
    assert_eq!(s.selected_text().as_deref(), Some("éll"));
    s.apply(Action::Click { line: 0, col: 6, extend: true, count: 1, add: false }, Instant::now());
    assert_eq!(s.selected_text().as_deref(), Some("éllo "));
    // Out-of-range input never panics.
    s.apply(Action::Click { line: 999, col: 999, extend: false, count: 1, add: false }, Instant::now());
    s.apply(Action::Drag { line: 999, col: 0 }, Instant::now());
}

#[test]
fn selection_geometry_per_line_for_rendering() {
    let s = {
        let mut s = st("ab\ncd\nef");
        select(&mut s, 1, 7);
        s
    };
    assert_eq!(s.selections_on_line(0).first().copied(), Some((1, 2, true)), "from col 1, newline included");
    assert_eq!(s.selections_on_line(1).first().copied(), Some((0, 2, true)));
    assert_eq!(s.selections_on_line(2).first().copied(), Some((0, 1, false)));
    let e = {
        let mut e = st("ab\n\ncd");
        select(&mut e, 0, 6);
        e
    };
    assert_eq!(e.selections_on_line(1).first().copied(), Some((0, 0, true)), "empty line in the middle still shows the newline");
    assert!(st("x").selections_on_line(0).is_empty());
}

#[test]
fn delete_word_and_line_endings_at_the_edges() {
    let now = Instant::now();
    let mut s = st("foo bar  baz");
    s.set_selection(Selection::caret(s.len_chars()));
    s.apply(Action::DeleteWordBack, now);
    assert_eq!(s.text(), "foo bar  ");
    s.apply(Action::DeleteWordBack, now);
    assert_eq!(s.text(), "foo ");
    s.set_selection(Selection::caret(0));
    s.apply(Action::DeleteWordForward, now);
    assert_eq!(s.text(), "");
    s.apply(Action::Backspace, now);
    s.apply(Action::Delete, now);
    s.apply(Action::Undo, now);
    s.apply(Action::Undo, now);
    s.apply(Action::Undo, now);
    assert_eq!(s.text(), "foo bar  baz", "everything is undoable, extra undos are harmless");
}

#[test]
fn scrolling_clamps_and_follows_the_caret() {
    let mut s = st(&"line\n".repeat(200));
    s.set_metrics(Metrics { char_w: 8.0, line_h: 20.0, view_h: 200.0, text_w: 400.0 });
    s.apply(Action::Scroll { dx: 0.0, dy: -50.0 }, Instant::now());
    assert_eq!(s.scroll_y, 0.0);
    s.apply(Action::Scroll { dx: 0.0, dy: 1e9 }, Instant::now());
    assert!(s.scroll_y > 3000.0 && s.scroll_y < 4000.0, "clamped near the end: {}", s.scroll_y);

    mv(&mut s, Motion::DocStart, false);
    assert_eq!(s.scroll_y, 0.0, "caret jump scrolls into view");
    for _ in 0..30 {
        mv(&mut s, Motion::Down, false);
    }
    assert_eq!(caret(&s).0, 30);
    assert_eq!(s.scroll_y, 30.0 * 20.0 + 20.0 - 200.0, "caret kept at the bottom edge");
    mv(&mut s, Motion::PageDown, false);
    assert_eq!(caret(&s).0, 30 + 9, "page = view lines - 1");

    // Horizontal: long line pushes the view right, then back.
    let mut s = st(&format!("{}\nshort", "x".repeat(300)));
    s.set_metrics(Metrics { char_w: 8.0, line_h: 20.0, view_h: 200.0, text_w: 400.0 });
    mv(&mut s, Motion::LineEnd, false);
    assert!(s.scroll_x > 1900.0, "{}", s.scroll_x);
    mv(&mut s, Motion::LineStart, false);
    mv(&mut s, Motion::LineStart, false);
    assert_eq!(s.scroll_x, 0.0);
    s.apply(Action::Scroll { dx: 1e9, dy: 0.0 }, Instant::now());
    assert!(s.scroll_x < 2500.0, "clamped to the widest line: {}", s.scroll_x);
}

#[test]
fn reset_keeps_scroll_and_bumps_version() {
    let mut s = st(&"a\n".repeat(100));
    s.set_metrics(Metrics { char_w: 8.0, line_h: 20.0, view_h: 200.0, text_w: 400.0 });
    s.apply(Action::Scroll { dx: 0.0, dy: 300.0 }, Instant::now());
    let v = s.version();
    s.reset("new\n".repeat(100).as_str());
    assert!(s.version() > v);
    assert_eq!(s.scroll_y, 300.0);
    assert!(!s.is_dirty());
    assert_eq!(s.line_string(0), "new");
}

/// Keeps the editing core honest on big inputs. Bounds are generous (debug
/// builds, loaded CI machines); run with `--nocapture` for the real numbers.
#[test]
fn big_documents_stay_interactive() {
    use std::time::Instant as Clock;
    let line = "let value_with_a_long_name = compute(argument_one, argument_two); // comment\n";
    let text = line.repeat(300_000); // ~22 MB, 300k lines
    let t = Clock::now();
    let mut s = st(&text);
    let load = t.elapsed();

    s.set_metrics(Metrics { char_w: 8.0, line_h: 20.0, view_h: 800.0, text_w: 900.0 });
    let t = Clock::now();
    for i in 0..2000usize {
        // Type somewhere in the middle, then somewhere else.
        let line = 150_000 + (i % 50);
        s.apply(Action::Click { line, col: 10, extend: false, count: 1, add: false }, Instant::now());
        s.apply(Action::Insert("x".into()), Instant::now());
    }
    let typing = t.elapsed();

    let t = Clock::now();
    for _ in 0..200 {
        s.apply(Action::Move { motion: Motion::PageDown, extend: false }, Instant::now());
        s.apply(Action::Move { motion: Motion::Down, extend: true }, Instant::now());
    }
    let moving = t.elapsed();

    let t = Clock::now();
    for _ in 0..500 {
        s.apply(Action::Undo, Instant::now());
    }
    let undo = t.elapsed();

    let t = Clock::now();
    let l = s.line_string(299_999);
    let rendering_lines: usize = (0..60).map(|i| s.line_string(100_000 + i).len()).sum();
    let line_access = t.elapsed();
    println!("load {load:?} | 2000 click+type {typing:?} | 400 moves {moving:?} | 500 undos {undo:?} | 60 lines {line_access:?}");
    assert!(!l.is_empty() && rendering_lines > 0);
    assert!(load.as_millis() < 3000, "load {load:?}");
    assert!(typing.as_millis() < 2000, "typing {typing:?}");
    assert!(moving.as_millis() < 1000, "moving {moving:?}");
    assert!(undo.as_millis() < 1500, "undo {undo:?}");
    assert!(line_access.as_millis() < 50, "line access {line_access:?}");
}

// ------------------------------------------------------------ multi-cursor

fn click_add(s: &mut EditorState, line: usize, col: usize) {
    s.apply(Action::Click { line, col, extend: false, count: 1, add: true }, Instant::now());
}

fn sels(s: &EditorState) -> Vec<(usize, usize)> {
    s.selections().map(|x| x.range()).collect()
}

#[test]
fn ctrl_d_selects_word_then_each_next_occurrence_with_wraparound() {
    let now = Instant::now();
    let mut s = st("foo bar foo\nbaz foo");
    s.set_selection(Selection::caret(1)); // inside the first "foo"
    s.apply(Action::SelectNextOccurrence, now);
    assert_eq!(sels(&s), vec![(0, 3)], "first press selects the word");
    s.apply(Action::SelectNextOccurrence, now);
    assert_eq!(sels(&s), vec![(0, 3), (8, 11)]);
    s.apply(Action::SelectNextOccurrence, now);
    assert_eq!(sels(&s), vec![(0, 3), (8, 11), (16, 19)]);
    assert_eq!(s.selection().range(), (16, 19), "the newest cursor is primary");
    s.apply(Action::SelectNextOccurrence, now);
    assert_eq!(s.cursor_count(), 3, "all occurrences taken: nothing more to add");

    // One keystroke edits every cursor; one undo reverts all of them.
    ins(&mut s, "X", now);
    assert_eq!(s.text(), "X bar X\nbaz X");
    assert!(s.is_dirty());
    s.apply(Action::Undo, now);
    assert_eq!(s.text(), "foo bar foo\nbaz foo");
    assert_eq!(sels(&s), vec![(0, 3), (8, 11), (16, 19)], "selections restored too");
    assert!(!s.is_dirty());
    s.apply(Action::Redo, now);
    assert_eq!(s.text(), "X bar X\nbaz X");
    assert_eq!(s.selected_text(), None, "carets after the replacement");
    s.apply(Action::Collapse, now);
    assert_eq!(s.cursor_count(), 1);
}

#[test]
fn typing_and_deleting_at_several_carets_coalesces_per_word() {
    let t0 = Instant::now();
    let mut s = st("a\nb\nc");
    s.set_selection(Selection::caret(1));
    for l in 1..3 {
        click_add(&mut s, l, 1);
    }
    assert_eq!(s.cursor_count(), 3);
    for (i, ch) in "xyz".chars().enumerate() {
        ins(&mut s, &ch.to_string(), t0 + Duration::from_millis(i as u64));
    }
    assert_eq!(s.text(), "axyz\nbxyz\ncxyz");
    s.apply(Action::Undo, t0);
    assert_eq!(s.text(), "a\nb\nc", "three keystrokes at three cursors = one undo step");

    s.apply(Action::Redo, t0);
    for i in 0..2 {
        s.apply(Action::Backspace, t0 + Duration::from_millis(10 + i));
    }
    assert_eq!(s.text(), "ax\nbx\ncx");
    s.apply(Action::Undo, t0);
    assert_eq!(s.text(), "axyz\nbxyz\ncxyz", "two backspaces at three cursors undo together");
    s.apply(Action::Undo, t0);
    assert_eq!(s.text(), "a\nb\nc");
}

#[test]
fn adjacent_cursors_cannot_overlap_and_merge_when_they_collide() {
    let now = Instant::now();
    let mut s = st("abcdef");
    s.set_selection(Selection::caret(2));
    click_add(&mut s, 0, 3);
    assert_eq!(sels(&s), vec![(2, 2), (3, 3)]);
    s.apply(Action::Backspace, now); // removes 'b' and 'c'
    assert_eq!(s.text(), "adef");
    assert_eq!(sels(&s), vec![(1, 1)], "both carets end up at the same spot and merge");
    // Word-delete with neighbours must stay inside its own gap.
    let mut s = st("one two three");
    s.set_selection(Selection::caret(7));
    click_add(&mut s, 0, 13);
    s.apply(Action::DeleteWordBack, now);
    assert_eq!(s.text(), "one  ", "each cursor removes its own word, no double deletion");
    // Moving left into each other merges the cursors.
    let mut s = st("ab");
    s.set_selection(Selection::caret(0));
    click_add(&mut s, 0, 1);
    mv(&mut s, Motion::Left, false);
    assert_eq!(sels(&s), vec![(0, 0)], "colliding carets merge");
    // Click on an existing cursor with Ctrl does not duplicate it.
    click_add(&mut s, 0, 0);
    assert_eq!(s.cursor_count(), 1);
}

#[test]
fn columns_of_cursors_above_and_below_keep_their_column() {
    let mut s = st("abcdef\nab\nabcdef\n");
    s.set_selection(Selection::caret(4)); // line 0, col 4
    s.apply(Action::AddCursorBelow, Instant::now());
    s.apply(Action::AddCursorBelow, Instant::now());
    let at: Vec<_> = s.selections().map(|x| s.line_and_display_col(x.head)).collect();
    assert_eq!(at, vec![(0, 4), (1, 2), (2, 4)], "short line clamps, the column is remembered");
    s.apply(Action::AddCursorBelow, Instant::now());
    assert_eq!(s.cursor_count(), 4, "empty last line gets one too");
    s.apply(Action::AddCursorAbove, Instant::now());
    assert_eq!(s.cursor_count(), 4, "already on the first line: nothing above");
    ins(&mut s, "!", Instant::now());
    assert_eq!(s.text(), "abcd!ef\nab!\nabcd!ef\n!");
}

#[test]
fn copy_joins_and_paste_distributes_one_line_per_cursor() {
    let now = Instant::now();
    let mut s = st("one two three");
    select(&mut s, 0, 3);
    s.apply(Action::Click { line: 0, col: 4, extend: false, count: 2, add: true }, now); // "two"
    s.apply(Action::Click { line: 0, col: 9, extend: false, count: 2, add: true }, now); // "three"
    assert_eq!(s.selected_text().as_deref(), Some("one\ntwo\nthree"));
    // Paste three lines into three cursors: each gets its own line.
    ins(&mut s, "A\nB\nC", now);
    assert_eq!(s.text(), "A B C");
    // A different number of lines is pasted whole at every cursor.
    let mut s = st("x y");
    s.set_selection(Selection::caret(0));
    click_add(&mut s, 0, 2);
    ins(&mut s, "1\n2\n3", now);
    assert_eq!(s.text(), "1\n2\n3x 1\n2\n3y");
}

#[test]
fn indenting_several_selections_touches_each_line_once() {
    let now = Instant::now();
    let mut s = st("a\nb\nc\nd\n");
    select(&mut s, 0, 3); // a, b
    s.apply(Action::Click { line: 1, col: 1, extend: false, count: 1, add: true }, now);
    // The second cursor is a caret on line 1: block indent applies to the
    // selection's lines (a, b); the caret line is already among them.
    s.apply(Action::Indent, now);
    assert_eq!(s.text(), "    a\n    b\nc\nd\n");
    s.apply(Action::Undo, now);
    assert_eq!(s.text(), "a\nb\nc\nd\n", "one undo step");

    // Pure carets on two lines: spaces to the next tab stop at both.
    let mut s = st("a\nbb\n");
    s.set_selection(Selection::caret(1));
    click_add(&mut s, 1, 2);
    s.apply(Action::Indent, now);
    assert_eq!(s.text(), "a   \nbb  \n");
    s.apply(Action::Enter, now);
    assert_eq!(s.text(), "a   \n\nbb  \n\n", "Enter at each caret opens a new line");
    let mut s = st("  x\n  y");
    s.set_selection(Selection::caret(3));
    click_add(&mut s, 1, 3);
    s.apply(Action::Enter, now);
    assert_eq!(s.text(), "  x\n  \n  y\n  ", "indentation is copied at every caret");
}

#[test]
fn bracket_matching_pairs_nested_brackets_and_gives_up_gracefully() {
    let mut s = st("fn f(a: [u8; 3]) { (x) }");
    let at = |s: &mut EditorState, i| s.set_selection(Selection::caret(i));
    at(&mut s, 5); // right after '('
    assert_eq!(s.matching_bracket(), Some((4, 15)));
    at(&mut s, 16); // right after ')'
    assert_eq!(s.matching_bracket(), Some((15, 4)));
    at(&mut s, 7);
    assert_eq!(s.matching_bracket(), None, "not next to a bracket");
    at(&mut s, 18); // right after '{'
    assert_eq!(s.matching_bracket(), Some((17, 23)));
    at(&mut s, 17); // right before '{'
    assert_eq!(s.matching_bracket(), Some((17, 23)));
    let mut s = st("(((");
    s.set_selection(Selection::caret(1));
    assert_eq!(s.matching_bracket(), None, "unbalanced");
    let mut s = st("");
    assert_eq!(s.matching_bracket(), None);
    s.set_selection(Selection::caret(0));
}

#[test]
fn replace_range_is_one_undo_step_and_select_range_reveals() {
    let now = Instant::now();
    let mut s = st(&"line\n".repeat(100));
    s.set_metrics(Metrics { char_w: 8.0, line_h: 20.0, view_h: 200.0, text_w: 400.0 });
    s.select_range(5 * 60, 5 * 60 + 4); // line 60
    assert_eq!(s.selected_text().as_deref(), Some("line"));
    assert!(s.scroll_y > 1000.0 && s.scroll_y < 1300.0, "centered: {}", s.scroll_y);
    s.replace_range(300, 304, "LINE", now);
    assert!(s.is_dirty());
    assert_eq!(s.line_string(60), "LINE");
    s.apply(Action::Undo, now);
    assert_eq!(s.line_string(60), "line");
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Any editing, selection, folding or history action, chosen at random for `s`.
fn random_action(rng: &mut Rng, s: &EditorState) -> Action {
    let (lines, line_len) = (s.line_count(), 12);
    let (line, col) = (rng.below(lines + 1), rng.below(line_len + 2));
    let motion = [Motion::Left, Motion::Right, Motion::Up, Motion::Down, Motion::WordLeft, Motion::WordRight,
        Motion::LineStart, Motion::LineEnd, Motion::DocStart, Motion::DocEnd, Motion::PageUp, Motion::PageDown][rng.below(12)];
    match rng.below(30) {
        0 => Action::Insert("a".into()),
        1 => Action::Insert("é\n".into()),
        2 => Action::Insert("x\r\ny".into()),
        3 => Action::Enter,
        4 | 5 => Action::Backspace,
        6 => Action::Delete,
        7 => Action::DeleteWordBack,
        8 => Action::DeleteWordForward,
        9 => Action::DeleteSelection,
        10 => Action::Indent,
        11 => Action::Unindent,
        12 | 13 => Action::Move { motion, extend: rng.below(2) == 0 },
        14 => Action::SelectAll,
        15 | 16 => Action::Click { line, col, extend: rng.below(3) == 0, count: 1 + rng.below(3) as u8, add: rng.below(2) == 0 },
        17 => Action::Drag { line, col },
        18 => Action::SelectNextOccurrence,
        19 => Action::AddCursorAbove,
        20 => Action::AddCursorBelow,
        21 => Action::Collapse,
        22 => Action::Undo,
        23 => Action::Redo,
        24 | 25 => Action::ToggleFold { line: line.min(lines.saturating_sub(1)) },
        26 => Action::FoldAll,
        27 => Action::Fold,
        28 => Action::Unfold,
        _ => Action::UnfoldAll,
    }
}

/// Random operations on several cursors must never panic, keep the cursor
/// invariants, and undo/redo must be exact inverses.
#[test]
fn random_operations_keep_invariants_and_undo_is_exact() {
    // `VELOEDIT_FUZZ=2000 cargo test random_operations` for a long soak run.
    let seeds: u64 = std::env::var("VELOEDIT_FUZZ").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    for seed in 1..=seeds {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let initial = "fn main() {\n    let ğ = \"日本\";\r\n\tx(1, [2, 3]);\n\n}\n";
        let mut s = st(initial);
        s.set_metrics(Metrics { char_w: 8.0, line_h: 20.0, view_h: 100.0, text_w: 300.0 });
        let mut now = Instant::now();
        for step in 0..250 {
            now += Duration::from_millis(rng.below(1500) as u64);
            // Soft wrap on/off and at different widths must never break the invariants.
            if step % 25 == 0 {
                s.set_wrap(if rng.below(3) == 0 { None } else { Some(8 + rng.below(14)) });
            }
            let action = random_action(&mut rng, &s);
            s.apply(action, now);
            // Folds stay inside the text, sorted, and nobody types into a hidden line.
            let n = s.line_count();
            assert!(s.folds.windows(2).all(|w| w[0].start < w[1].start), "seed {seed}: folds unsorted {:?}", s.folds);
            assert!(s.folds.iter().all(|f| f.start < f.end && f.end < n), "seed {seed}: fold outside the text {:?} ({n} lines)", s.folds);
            for c in sels(&s).iter().flat_map(|&(a, b)| [a, b]) {
                assert!(!s.line_hidden(s.line_of(c)), "seed {seed}: a cursor sits in a folded line");
            }
            let visible = (0..n).filter(|&l| !s.line_hidden(l)).count();
            if s.wrapping() {
                assert!(s.total_rows() >= visible, "seed {seed}: rows {} < visible lines {visible}", s.total_rows());
            } else {
                assert_eq!(s.total_rows(), visible, "seed {seed}: one row per visible line");
            }
            for r in 0..s.total_rows().min(40) {
                assert!(!s.line_hidden(s.line_of_row(r)), "seed {seed}: row {r} maps into a hidden line");
            }
            // Invariants: ascending, disjoint, inside the document.
            let r = sels(&s);
            assert!(!r.is_empty());
            for w in r.windows(2) {
                assert!(w[0].1 <= w[1].0 && w[0] != w[1], "seed {seed}: overlapping cursors {r:?}");
            }
            assert!(r.iter().all(|x| x.1 <= s.len_chars()), "seed {seed}: cursor outside the text");
            assert!(s.selection().range().1 <= s.len_chars());
        }
        // The random run may have ended with undone steps pending: go to the tip first.
        for _ in 0..HISTORY_CAP + 5 {
            s.apply(Action::Redo, now);
        }
        let tip = s.text();
        // Undo everything: back to the start, exactly.
        for _ in 0..HISTORY_CAP + 5 {
            s.apply(Action::Undo, now);
        }
        assert_eq!(s.text(), initial, "seed {seed}: undo-all must restore the original");
        assert!(!s.is_dirty());
        for _ in 0..HISTORY_CAP + 5 {
            s.apply(Action::Redo, now);
        }
        assert_eq!(s.text(), tip, "seed {seed}: redo-all must reproduce the tip");
    }
}

// ------------------------------------------------------------- soft wrap

fn wrapped(text: &str, cols: usize) -> EditorState {
    let s = st(text);
    s.set_wrap(Some(cols));
    s
}

fn rows(s: &EditorState) -> Vec<String> {
    (0..s.total_rows())
        .map(|r| {
            let sp = s.row_span(r);
            s.line_string(sp.line).chars().skip(sp.start).take(sp.end - sp.start).collect()
        })
        .collect()
}

#[test]
fn lines_wrap_after_blanks_and_hard_when_a_word_is_too_long() {
    let s = wrapped("aaa bbb ccc ddd eee\nshort\n\nXXXXXXXXXXXXXXXXXXXXXXXX", 10);
    assert_eq!(
        rows(&s),
        vec!["aaa bbb ", "ccc ddd ", "eee", "short", "", "XXXXXXXXXX", "XXXXXXXXXX", "XXXX"],
        "greedy at blanks; a word longer than the row is cut at the edge"
    );
    assert_eq!(s.total_rows(), 8);
    assert_eq!(s.rows_of_line(0), 0..3);
    assert_eq!(s.rows_of_line(3), 5..8);
    let sp = s.row_span(1);
    assert_eq!((sp.line, sp.start, sp.end, sp.start_col, sp.first, sp.last), (0, 8, 16, 8, false, false));
    assert_eq!(s.row_span(2).end_col, None, "last row of a line has no end column");
    // Without wrapping every line is exactly one row.
    s.set_wrap(None);
    assert_eq!(s.total_rows(), 4);
    assert_eq!(s.row_span(0).end, 19);
}

#[test]
fn rows_and_indices_map_both_ways_including_tabs_and_wide_glyphs() {
    for text in ["aaa bbb ccc ddd eee fff ggg", "\tab\tcd efgh ijkl mnop", "日本語日本語日本語 x 日本語日本語"] {
        let s = wrapped(text, 9);
        for idx in 0..=s.len_chars() {
            let (row, col) = s.row_of(idx);
            assert_eq!(s.idx_at_row_col(row, col), idx, "{text:?}: idx {idx} -> row {row} col {col}");
            assert!(col <= 9 + 2, "{text:?}: column {col} fits the row (blanks may hang)");
        }
    }
    // A caret right after the last character of a row belongs to the next row.
    let s = wrapped("aaaa bbbb cccc", 6);
    assert_eq!(s.row_of(5), (1, 0), "start of the second row");
    assert_eq!(s.row_of(4), (0, 4));
}

#[test]
fn vertical_movement_follows_rows_and_remembers_the_column() {
    let now = Instant::now();
    let mut s = wrapped("aaaa bbbb cccc dddd\nxy", 10);
    // rows: "aaaa bbbb " / "cccc dddd" / "xy"
    s.set_selection(Selection::caret(3)); // row 0, col 3
    s.apply(Action::Move { motion: Motion::Down, extend: false }, now);
    assert_eq!(s.row_of(s.selection().head), (1, 3));
    s.apply(Action::Move { motion: Motion::Down, extend: false }, now);
    assert_eq!(s.row_of(s.selection().head), (2, 2), "clamped to the short line");
    s.apply(Action::Move { motion: Motion::Up, extend: false }, now);
    assert_eq!(s.row_of(s.selection().head), (1, 3), "column remembered");
    s.apply(Action::Move { motion: Motion::Up, extend: false }, now);
    s.apply(Action::Move { motion: Motion::Up, extend: false }, now);
    assert_eq!(s.selection().head, 0, "up from the first row goes to the start");
}

#[test]
fn wrapped_selection_and_carets_are_reported_per_row() {
    let mut s = wrapped("aaaa bbbb cccc dddd\nxy", 10);
    select(&mut s, 7, 13); // "bb bbcc" crosses the first row break (row 0 = 0..10)
    assert_eq!(s.selections_in_row(0), vec![(7, 10, false)]);
    assert_eq!(s.selections_in_row(1), vec![(0, 3, false)]);
    assert!(s.selections_in_row(2).is_empty());
    select(&mut s, 12, 21); // into the next logical line
    assert_eq!(s.selections_in_row(1), vec![(2, 9, true)], "newline marked on the last row of the line");
    assert_eq!(s.selections_in_row(2), vec![(0, 1, false)]);
    s.set_selection(Selection::caret(12));
    assert_eq!(s.carets_in_rows(0..3), vec![(1, 2, true)]);
    assert!(s.carets_in_rows(2..3).is_empty());
}

#[test]
fn scrolling_uses_visual_rows_and_horizontal_scroll_is_disabled() {
    let text = format!("{}\n", "word ".repeat(200)); // one long line -> many rows at 20 cols
    let mut s = wrapped(&text, 20);
    s.set_metrics(Metrics { char_w: 8.0, line_h: 20.0, view_h: 200.0, text_w: 160.0 });
    assert!(s.total_rows() > 40);
    s.apply(Action::Move { motion: Motion::DocEnd, extend: false }, Instant::now());
    // The caret on the last row (the empty line after the newline) is in view.
    let want = (s.total_rows() as f32 - 1.0) * 20.0 + 20.0 - 200.0;
    assert!((s.scroll_y - want).abs() < 1.0, "scroll_y {} vs {want}", s.scroll_y);
    assert_eq!(s.scroll_x, 0.0);
    s.apply(Action::Scroll { dx: 500.0, dy: 0.0 }, Instant::now());
    assert_eq!(s.scroll_x, 0.0, "no sideways scrolling while wrapping");
}

#[test]
fn wrapping_survives_edits_and_gives_up_on_huge_documents() {
    let now = Instant::now();
    let mut s = wrapped("aaaa bbbb cccc", 8);
    assert_eq!(rows(&s), vec!["aaaa ", "bbbb ", "cccc"]);
    s.set_selection(Selection::caret(4));
    ins(&mut s, "XXXXXXXXXXXX", now);
    assert_eq!(rows(&s), vec!["aaaaXXXX", "XXXXXXXX ", "bbbb ", "cccc"], "the grown word is cut at the edge");
    s.apply(Action::Undo, now);
    assert_eq!(rows(&s), vec!["aaaa ", "bbbb ", "cccc"], "rows follow undo");

    let big = "x ".repeat(WRAP_MAX_CHARS);
    let s = wrapped(&big, 40);
    assert!(!s.wrapping(), "too large: wrapping is off, the editor stays responsive");
    assert_eq!(s.total_rows(), 1);
}

#[test]
fn headline_is_the_first_non_blank_line_shortened() {
    assert_eq!(st("").headline(28), None);
    assert_eq!(st("  \n\t\n \r\n").headline(28), None, "only blanks");
    assert_eq!(st("hello\nworld").headline(28).as_deref(), Some("hello"));
    assert_eq!(st("\n\n   indented \t and   spaced  \nnext").headline(28).as_deref(), Some("indented and spaced"));
    assert_eq!(st("crlf line\r\nsecond").headline(28).as_deref(), Some("crlf line"), "no stray \\r");
    assert_eq!(st("bell\u{7}char").headline(28).as_deref(), Some("bellchar"), "control characters are dropped");
    // Cut with an ellipsis, counting characters (not bytes), never splitting one.
    let long = "ğüşiöçĞÜŞİÖÇ abcdefghijklmnopqrstuvwxyz";
    let cut = st(long).headline(12).unwrap();
    assert_eq!(cut.chars().count(), 12);
    assert!(cut.ends_with('\u{2026}') && cut.starts_with("ğüşiöçĞÜŞİÖ"), "{cut}");
    assert_eq!(st("exactly ten").headline(11).as_deref(), Some("exactly ten"), "fits: no ellipsis");
    // A huge single line (a minified paste) is cheap and still bounded.
    let big = "x".repeat(5_000_000);
    assert_eq!(st(&big).headline(28).unwrap().chars().count(), 28);
    // Looks only at the first lines: a long run of blanks gives up instead of scanning everything.
    let blanks = format!("{}late", "\n".repeat(1000));
    assert_eq!(st(&blanks).headline(28), None);
}

/// Two views of one buffer (split right / down): whichever pane the random action runs in,
/// the other one must show exactly the same text, keep sane cursors and folds, and keep
/// its own scroll position.
#[test]
fn a_split_mirror_follows_every_edit_from_either_pane() {
    let seeds: u64 = std::env::var("VELOEDIT_FUZZ").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    for seed in 1..=seeds {
        let mut rng = Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1);
        let initial = "fn main() {\n    let ğ = \"日本\";\r\n\tx(1, [2, 3]);\n\n}\n".repeat(6);
        let mut doc = st(&initial);
        doc.set_metrics(Metrics { char_w: 8.0, line_h: 20.0, view_h: 100.0, text_w: 300.0 });
        let mut other = doc.mirror();
        other.set_metrics(Metrics { char_w: 8.0, line_h: 20.0, view_h: 140.0, text_w: 200.0 });
        doc.track_changes(true);
        let mut focus = 0;
        let mut now = Instant::now();
        for step in 0..250 {
            now += Duration::from_millis(rng.below(1500) as u64);
            // Click into the other pane: the views trade places, like the app does.
            if rng.below(3) == 0 {
                doc.swap_view(&mut other);
                focus = 1 - focus;
            }
            if step % 40 == 39 {
                doc.reset("replaced\nfrom disk\n");
            } else {
                let action = random_action(&mut rng, &doc);
                let untouched = (sels(&other), other.scroll_y, other.folds.clone());
                let edited = doc.apply(action, now);
                doc.feed(&mut other);
                if !edited {
                    // Moving, selecting, scrolling or folding in one pane never disturbs the other.
                    assert_eq!((sels(&other), other.scroll_y, other.folds.clone()), untouched, "seed {seed} step {step}");
                }
            }
            if step % 40 == 39 {
                doc.feed(&mut other);
            }
            assert_eq!(other.text(), doc.text(), "seed {seed} step {step}: the panes show different text");
            assert_eq!(other.version(), doc.version(), "seed {seed}");
            for (name, v) in [("focused", &doc), ("other", &other)] {
                let n = v.line_count();
                assert!(v.folds.windows(2).all(|w| w[0].start < w[1].start), "seed {seed} {name}: folds unsorted");
                assert!(v.folds.iter().all(|f| f.start < f.end && f.end < n), "seed {seed} {name}: fold outside the text");
                let r = sels(v);
                for w in r.windows(2) {
                    assert!(w[0].1 <= w[1].0 && w[0] != w[1], "seed {seed} {name}: overlapping cursors {r:?}");
                }
                assert!(r.iter().all(|x| x.1 <= v.len_chars()), "seed {seed} {name}: cursor outside the text");
                for c in r.iter().flat_map(|&(a, b)| [a, b]) {
                    assert!(!v.line_hidden(v.line_of(c)), "seed {seed} {name}: a cursor sits in a folded line");
                }
            }
        }
        // The document's own undo history reaches back through both panes' edits.
        for _ in 0..HISTORY_CAP + 5 {
            doc.apply(Action::Undo, now);
            doc.feed(&mut other);
        }
        assert_eq!(other.text(), doc.text(), "seed {seed}: after undoing everything");
    }
}

#[test]
fn panes_keep_their_own_scroll_cursor_and_folds() {
    let text: String = (0..300).map(|i| format!("line {i}\n")).collect();
    let now = Instant::now();
    let mut doc = st(&text);
    doc.set_metrics(Metrics { char_w: 8.0, line_h: 20.0, view_h: 200.0, text_w: 400.0 });
    let mut other = doc.mirror();
    doc.track_changes(true);
    // This pane: caret on line 5, looking at line 200.
    doc.apply(Action::Click { line: 5, col: 2, extend: false, count: 1, add: false }, now);
    doc.apply(Action::ScrollTo { y: 20.0 * 200.0 }, now);
    assert_eq!(other.scroll_y, 0.0, "the new pane keeps its own place");
    // Click into the other pane (views trade places) and type a new first line.
    doc.swap_view(&mut other);
    doc.apply(Action::Insert("new first line\n".into()), now);
    doc.feed(&mut other);
    doc.swap_view(&mut other);
    assert_eq!(doc.scroll_y, 20.0 * 200.0, "the pane that did not type did not scroll");
    assert_eq!(doc.line_of(doc.selection().head), 6, "its caret moved down with the text");
    assert_eq!(other.caret_line_col(), (1, 0), "the typing pane's caret sits behind the new text");
    assert_eq!(other.scroll_y, 0.0);
    assert_eq!(other.text(), doc.text());
    // Undo from either pane takes the edit back in both.
    doc.apply(Action::Undo, now);
    doc.feed(&mut other);
    assert_eq!(other.text(), text);
    assert_eq!(doc.text(), text);
}

#[test]
fn trimming_cuts_blanks_off_every_line_end_in_one_undo_step() {
    let now = Instant::now();
    let text = "a  \n\tb\t \n   \n c  d  \nlast  ";
    let mut s = st(text);
    s.mark_saved();
    assert_eq!(s.trim_trailing_whitespace(now, false), 5, "five lines end in blanks");
    assert_eq!(s.text(), "a\n\tb\n\n c  d\nlast", "inner blanks and indentation stay, line breaks stay");
    assert!(s.is_dirty());
    // One step back brings every blank back.
    s.apply(Action::Undo, now);
    assert_eq!(s.text(), text);
    assert!(!s.is_dirty(), "undo back to the saved text is clean");
    s.apply(Action::Redo, now);
    assert_eq!(s.text(), "a\n\tb\n\n c  d\nlast");

    // Nothing left to trim: the text, the version and the history are left alone.
    s.mark_saved();
    let version = s.version();
    assert_eq!(s.trim_trailing_whitespace(now, false), 0);
    assert_eq!(s.version(), version);
    assert!(!s.is_dirty(), "a no-op trim does not make the buffer dirty");
}

#[test]
fn trimming_keeps_crlf_line_endings_and_only_cuts_horizontal_blanks() {
    let now = Instant::now();
    let mut s = st("a  \r\nb\t\r\n\r\nc \r\n");
    assert_eq!(s.trim_trailing_whitespace(now, false), 3);
    assert_eq!(s.text(), "a\r\nb\r\n\r\nc\r\n");
    assert_eq!(s.eol(), Eol::CrLf);

    // No-break and ideographic spaces are blanks; a form feed ends a line, it is not one.
    let mut s = st("x\u{00A0} \u{3000}\ny\u{0C}z");
    assert_eq!(s.trim_trailing_whitespace(now, false), 1);
    assert_eq!(s.text(), "x\ny\u{0C}z");
}

#[test]
fn trimming_moves_carets_and_selections_with_the_text() {
    let now = Instant::now();
    // A caret in the blanks lands at the end of the text left of them.
    let mut s = st("ab    \ncd  \nef");
    s.set_selection(Selection::caret(4));
    s.trim_trailing_whitespace(now, false);
    assert_eq!(s.text(), "ab\ncd\nef");
    assert_eq!(s.selection(), Selection::caret(2));

    // A selection from line 1 into line 3 keeps covering the same words.
    let mut s = st("one  \ntwo  \nthree  ");
    select(&mut s, 1, 15); // "ne  \ntwo  \nthr"
    s.trim_trailing_whitespace(now, false);
    assert_eq!(s.text(), "one\ntwo\nthree");
    assert_eq!(s.selected_text().as_deref(), Some("ne\ntwo\nthr"));

    // Every cursor is handled: three carets, each behind the blanks of its line.
    let mut s = st("a \nb \nc ");
    s.set_selection(Selection::caret(0));
    s.apply(Action::AddCursorBelow, now);
    s.apply(Action::AddCursorBelow, now);
    mv(&mut s, Motion::LineEnd, false);
    assert_eq!(sels(&s), vec![(2, 2), (5, 5), (8, 8)]);
    s.trim_trailing_whitespace(now, false);
    assert_eq!(s.text(), "a\nb\nc");
    assert_eq!(sels(&s), vec![(1, 1), (3, 3), (5, 5)], "each caret stays at the end of its own line");
}

#[test]
fn trimming_is_replayed_on_a_split_mirror() {
    let now = Instant::now();
    let mut doc = st("fn a() {  \n\tx();\t\n}  \n".repeat(4).as_str());
    let mut other = doc.mirror();
    doc.track_changes(true);
    other.set_selection(Selection::caret(9)); // in the blanks of line 1, in the other pane
    assert_eq!(doc.trim_trailing_whitespace(now, false), 12);
    doc.feed(&mut other);
    assert_eq!(other.text(), doc.text());
    assert_eq!(other.version(), doc.version());
    assert_eq!(other.selection(), Selection::caret(8), "the other pane's caret moved to the end of its line");
}

#[test]
fn trimming_markdown_keeps_hard_line_breaks_and_cuts_everything_else() {
    let now = Instant::now();
    let text = "para one  \nsecond line\n\nend of para  \n\nlast   \n\ttab\t\nx \nmixed\t  \nnext\n  \nfoo\ny  ";
    let mut s = st(text);
    let n = s.trim_trailing_whitespace(now, true);
    assert_eq!(
        s.text(),
        // kept: "para one  " (text follows) and "last   " (text follows, three spaces is still a break)
        "para one  \nsecond line\n\nend of para\n\nlast   \n\ttab\nx\nmixed\nnext\n\nfoo\ny",
        "kept: spaces before more text; cut: before a blank line or the end, a single space, tabs, a blank line's own blanks"
    );
    assert_eq!(n, 6);
    // The same text without the exception is cut everywhere.
    let mut s = st(text);
    assert_eq!(s.trim_trailing_whitespace(now, false), 8);
    assert_eq!(s.text(), "para one\nsecond line\n\nend of para\n\nlast\n\ttab\nx\nmixed\nnext\n\nfoo\ny");

    // CRLF files break the same way.
    let mut s = st("a  \r\nb  \r\n\r\nc \r\n");
    assert_eq!(s.trim_trailing_whitespace(now, true), 2);
    assert_eq!(s.text(), "a  \r\nb\r\n\r\nc\r\n");
}

#[test]
fn replacing_the_text_is_an_undoable_edit_of_the_part_that_differs() {
    let now = Instant::now();
    let mut s = st("fn main() {\n    old();\n    keep();\n}\n");
    s.mark_saved();
    s.set_selection(Selection::caret(s.text().find("keep").unwrap() + 2)); // inside "keep"
    assert!(s.replace_text("fn main() {\n    brand_new(1);\n    keep();\n}\n", now));
    assert_eq!(s.text(), "fn main() {\n    brand_new(1);\n    keep();\n}\n");
    assert!(!s.is_dirty(), "the reloaded text is the saved state");
    assert_eq!(s.selected_text(), None);
    let caret = s.selection().head;
    assert_eq!(&s.text()[caret - 2..caret + 2], "keep", "the caret stays at the same place of the unchanged tail");

    // One Ctrl+Z gives the old text back - and the buffer is unsaved again.
    s.apply(Action::Undo, now);
    assert_eq!(s.text(), "fn main() {\n    old();\n    keep();\n}\n");
    assert!(s.is_dirty());
    s.apply(Action::Redo, now);
    assert_eq!(s.text(), "fn main() {\n    brand_new(1);\n    keep();\n}\n");
    assert!(!s.is_dirty());
}

#[test]
fn replacing_with_the_same_text_changes_nothing_and_other_texts_work_too() {
    let now = Instant::now();
    let mut s = st("same\ntext\n");
    let v = s.version();
    assert!(s.replace_text("same\ntext\n", now));
    assert_eq!(s.version(), v, "identical text: no edit, no history");
    assert!(!s.is_dirty());

    // Emptying and filling, appending, a different line ending, multi-byte text.
    assert!(s.replace_text("", now));
    assert_eq!(s.text(), "");
    s.apply(Action::Undo, now);
    assert_eq!(s.text(), "same\ntext\n", "even an emptied buffer comes back");
    assert!(s.replace_text("same\ntext\nmore ğü日本\n", now));
    assert_eq!(s.text(), "same\ntext\nmore ğü日本\n");
    assert!(s.replace_text("same\r\ntext\r\nmore ğü日本\r\n", now));
    assert_eq!(s.eol(), Eol::CrLf);
    s.apply(Action::Undo, now);
    assert_eq!(s.text(), "same\ntext\nmore ğü日本\n");
}

#[test]
fn a_difference_too_big_for_the_history_is_left_to_the_caller() {
    let now = Instant::now();
    let big = "x".repeat(900_000);
    let mut s = st(&big);
    assert!(!s.replace_text(&"y".repeat(900_000), now), "1.8M characters of difference");
    assert_eq!(s.text(), big, "nothing was touched");
    assert!(s.replace_text(&format!("{big}tail"), now), "a small difference in a big text is cheap");
}

#[test]
fn replacing_the_text_is_followed_by_a_split_mirror() {
    let now = Instant::now();
    let mut doc = st("one\ntwo\nthree\n");
    let mut other = doc.mirror();
    doc.track_changes(true);
    assert!(doc.replace_text("one\n2\nthree\nfour\n", now));
    doc.feed(&mut other);
    assert_eq!(other.text(), doc.text());
}

#[test]
fn converting_indentation_keeps_each_lines_width_and_is_one_undo_step() {
    let now = Instant::now();
    // Tab size 4 (the default): a tab is up to the next multiple of 4 columns.
    let mut s = st("\tone\n  \ttwo\n        three\n      four\n   five\nsix\n  \n");
    let original = s.text();
    // To spaces: every tab becomes the spaces that reach the same column.
    assert_eq!(s.convert_indentation(true, now), 2);
    assert_eq!(s.text(), "    one\n    two\n        three\n      four\n   five\nsix\n  \n");
    s.apply(Action::Undo, now);
    assert_eq!(s.text(), original, "one undo brings every line back");
    assert!(!s.is_dirty());

    // To tabs: a tab per 4 columns, the rest stays spaces; nothing inside a line is touched.
    let mut s = st("    one\n      two\n   three\n\t  four\nspaces    inside\n");
    assert_eq!(s.convert_indentation(false, now), 2);
    assert_eq!(s.text(), "\tone\n\t  two\n   three\n\t  four\nspaces    inside\n");
    assert_eq!(s.convert_indentation(false, now), 0, "nothing left to convert");

    // Another tab size changes what a tab is worth.
    let mut s = st("\tone\n    two\n");
    s.set_tab_size(2);
    assert_eq!(s.convert_indentation(true, now), 1);
    assert_eq!(s.text(), "  one\n    two\n");
    s.set_tab_size(8);
    assert_eq!(s.convert_indentation(false, now), 0, "two and four columns are not a tab at width 8");
    assert_eq!(s.text(), "  one\n    two\n");
    s.set_tab_size(2);
    assert_eq!(s.convert_indentation(false, now), 2, "at width 2 they are one and two tabs");
    assert_eq!(s.text(), "\tone\n\t\ttwo\n");
}

#[test]
fn converting_indentation_moves_carets_with_the_text() {
    let now = Instant::now();
    let mut s = st("\t\tx\nplain\n");
    s.set_selection(Selection::caret(3)); // right behind the x
    s.convert_indentation(true, now);
    assert_eq!(s.text(), "        x\nplain\n");
    assert_eq!(s.selection(), Selection::caret(9), "still right behind the x");
}

#[test]
fn line_endings_can_be_converted_both_ways_and_undo_restores_the_style_too() {
    let now = Instant::now();
    let mut s = st("a\nb\n\nc");
    assert_eq!(s.eol(), Eol::Lf);
    assert_eq!(s.set_eol(Eol::CrLf, now), 3, "the last line has no ending");
    assert_eq!(s.text(), "a\r\nb\r\n\r\nc");
    assert_eq!(s.eol(), Eol::CrLf);
    // New lines follow the new style.
    s.set_selection(Selection::caret(s.len_chars()));
    ins(&mut s, "\nd", now + Duration::from_secs(5));
    assert_eq!(s.text(), "a\r\nb\r\n\r\nc\r\nd");
    s.apply(Action::Undo, now);
    s.apply(Action::Undo, now);
    assert_eq!(s.text(), "a\nb\n\nc", "undo restores the text...");
    assert_eq!(s.eol(), Eol::Lf, "...and the style");
    s.apply(Action::Redo, now);
    assert_eq!((s.text().as_str(), s.eol()), ("a\r\nb\r\n\r\nc", Eol::CrLf));

    // Back to LF; a mixed file becomes uniform either way.
    assert_eq!(s.set_eol(Eol::Lf, now), 3);
    assert_eq!(s.text(), "a\nb\n\nc");
    let mut mixed = st("a\r\nb\nc\r\nd\n");
    assert_eq!(mixed.set_eol(Eol::Lf, now), 2);
    assert_eq!(mixed.text(), "a\nb\nc\nd\n");
    let mut mixed = st("a\r\nb\nc\r\nd\n");
    assert_eq!(mixed.set_eol(Eol::CrLf, now), 2);
    assert_eq!(mixed.text(), "a\r\nb\r\nc\r\nd\r\n");
}

#[test]
fn a_line_ending_that_is_already_right_changes_nothing() {
    let now = Instant::now();
    let mut s = st("a\r\nb\r\n");
    let version = s.version();
    assert_eq!(s.set_eol(Eol::CrLf, now), 0);
    assert_eq!((s.version(), s.is_dirty()), (version, false), "no edit, nothing to save");
    // A text without any line break only takes the style (the next Enter uses it).
    let mut one = st("single line");
    assert_eq!(one.set_eol(Eol::CrLf, now), 0);
    assert_eq!(one.eol(), Eol::CrLf);
    assert!(!one.is_dirty());
    one.set_selection(Selection::caret(11));
    ins(&mut one, "\nnext", now);
    assert_eq!(one.text(), "single line\r\nnext");
}

#[test]
fn converting_line_endings_keeps_carets_and_a_split_mirror_in_step() {
    let now = Instant::now();
    let mut s = st("one\ntwo\nthree");
    s.set_selection(Selection { anchor: 4, head: 7 }); // "two"
    s.set_eol(Eol::CrLf, now);
    assert_eq!(s.text(), "one\r\ntwo\r\nthree");
    assert_eq!(s.selected_text().as_deref(), Some("two"), "the selection still covers the same word");
    // A caret at a line's end stays in front of the whole line break, never between \r and \n.
    let mut c = st("ab\ncd");
    c.set_selection(Selection::caret(2));
    c.set_eol(Eol::CrLf, now);
    assert_eq!(c.selection(), Selection::caret(2));
    ins(&mut c, "X", now);
    assert_eq!(c.text(), "abX\r\ncd");
    // ...and the way back.
    c.set_eol(Eol::Lf, now);
    assert_eq!((c.text().as_str(), c.selection()), ("abX\ncd", Selection::caret(3)));

    // The second pane of a split follows the text and the style.
    let mut mirror = st("one\ntwo\nthree");
    let changes = s.take_changes();
    mirror.follow(&s, changes);
    assert_eq!(mirror.text(), s.text());
    assert_eq!(mirror.eol(), Eol::CrLf);
}
