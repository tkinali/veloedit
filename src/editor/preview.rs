//! Parsed Markdown for the preview pane.
//!
//! Two jobs:
//!
//! * **Tables are parsed here**, not by iced: iced's table widget lays every
//!   cell out on one unwrapped line inside a horizontal scrollable, which is
//!   unreadable for real-world tables (long prose in cells). We split the
//!   source into *blocks* - ordinary Markdown (parsed by iced) and tables
//!   (cells parsed by iced as inline Markdown, laid out by `ui::markdown_view`
//!   with wrapping columns).
//! * **Big documents are windowed.** Laying a rendered document out costs time
//!   proportional to its block count on *every* UI update, and the parser is
//!   super-linear on big inputs (measured: 400 KiB = 40 ms parse + ~250 ms
//!   layout; 4 MiB = 2.8 s parse). So small documents are rendered whole;
//!   bigger ones are cut into chunks at block boundaries (inside long tables:
//!   between rows, repeating the header) and only the few chunks around the
//!   editor's scroll position are shown. The preview follows the editor.

use iced::widget::markdown;

use super::TextDoc;

/// Documents up to this size are parsed and shown whole.
pub const FULL_MAX_BYTES: usize = 48 * 1024;
/// Chunks of larger documents start at the first block boundary after this much text.
const CHUNK_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// One table cell: its inline Markdown, parsed.
#[derive(Debug, Clone)]
pub struct Cell {
    pub items: Vec<markdown::Item>,
    /// Length of the source text (a hint for column widths).
    pub chars: usize,
}

#[derive(Debug, Clone)]
pub struct Table {
    pub aligns: Vec<Align>,
    pub header: Vec<Cell>,
    pub rows: Vec<Vec<Cell>>,
}

#[derive(Debug, Clone)]
pub enum Block {
    Markdown(Vec<markdown::Item>),
    Table(Table),
}

/// What part of a large document is currently shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// 1-based, inclusive.
    pub first_line: usize,
    pub last_line: usize,
    pub total_lines: usize,
    start: usize,
    end: usize,
}

/// Where a chunk of a large document starts; inside a table, also where that
/// table's header (+ delimiter row) is, so the chunk can repeat it.
#[derive(Debug, Clone, Copy)]
struct Chunk {
    start: usize,
    header: Option<(usize, usize)>,
}

pub struct Preview {
    pub version: u64,
    pub blocks: Vec<Block>,
    /// `None`: the whole document.
    pub window: Option<Window>,
    chunks: Vec<Chunk>,
    /// The chunk the window is centered on.
    center: usize,
}

impl Preview {
    /// Builds the preview for the document's current text and scroll position,
    /// re-using `prev`'s chunk table when the text did not change.
    pub fn build(doc: &TextDoc, prev: Option<Preview>) -> Self {
        let state = &doc.state;
        if state.len_bytes() <= FULL_MAX_BYTES {
            let blocks = parse_blocks(&doc.text());
            return Self { version: doc.version(), blocks, window: None, chunks: Vec::new(), center: 0 };
        }
        let chunks = match prev {
            Some(p) if p.version == doc.version() && !p.chunks.is_empty() => p.chunks,
            _ => chunk_starts(doc),
        };
        let center = chunk_at(&chunks, state.top_byte());
        let lo = center.saturating_sub(1);
        let hi = (center + 1).min(chunks.len() - 1);
        let start = chunks[lo].start;
        let end = chunks.get(hi + 1).map_or(state.len_bytes(), |c| c.start);
        let mut text = String::new();
        if let Some((a, b)) = chunks[lo].header {
            text = state.byte_slice(a, b); // the window opens inside a table: repeat its header
        }
        text.push_str(&state.byte_slice(start, end));
        let window = Window {
            first_line: state.line_of_byte(start) + 1,
            last_line: state.line_of_byte(end.saturating_sub(1)) + 1,
            total_lines: state.line_count(),
            start,
            end,
        };
        Self { version: doc.version(), blocks: parse_blocks(&text), window: Some(window), chunks, center }
    }

    /// Does this preview still show what the document and its scroll position call for?
    pub fn is_current(&self, doc: &TextDoc) -> bool {
        self.version == doc.version()
            && (self.window.is_none() || chunk_at(&self.chunks, doc.state.top_byte()) == self.center)
    }

    /// Where in the rendered part the editor's top line is, 0..=1.
    pub fn scroll_fraction(&self, doc: &TextDoc) -> f32 {
        match self.window {
            Some(w) => {
                let top = doc.state.top_byte().clamp(w.start, w.end);
                ((top - w.start) as f32 / (w.end - w.start).max(1) as f32).clamp(0.0, 1.0)
            }
            None => {
                let max = doc.state.max_scroll_y();
                if max > 0.0 { (doc.state.scroll_y / max).clamp(0.0, 1.0) } else { 0.0 }
            }
        }
    }

    /// Rough amount of rendered content (blocks + table rows), for tests.
    #[cfg(test)]
    pub fn weight(&self) -> usize {
        self.blocks
            .iter()
            .map(|b| match b {
                Block::Markdown(items) => items.len(),
                Block::Table(t) => t.rows.len() + 1,
            })
            .sum()
    }
}

/// Index of the chunk containing byte `at`.
fn chunk_at(chunks: &[Chunk], at: usize) -> usize {
    chunks.partition_point(|c| c.start <= at).saturating_sub(1)
}

// ---------------------------------------------------------------- line helpers

/// Tracks fenced code blocks line by line (their content is never a table).
#[derive(Default)]
struct Fence(Option<(char, usize)>);

impl Fence {
    /// Feeds one line; returns whether the line is part of a fenced block (fences included).
    fn step(&mut self, line: &str) -> bool {
        let trimmed = line.trim_start();
        let run = |c: char| trimmed.chars().take_while(|&x| x == c).count();
        match self.0 {
            Some((c, n)) => {
                if run(c) >= n && trimmed.trim_start_matches(c).trim().is_empty() {
                    self.0 = None;
                }
                true
            }
            None => {
                for c in ['`', '~'] {
                    if run(c) >= 3 {
                        self.0 = Some((c, run(c)));
                        return true;
                    }
                }
                false
            }
        }
    }
}

/// `|---|:--:|` -> alignments (None when the line is not a delimiter row).
fn delimiter(line: &str) -> Option<Vec<Align>> {
    let t = line.trim();
    if !t.contains('-') || !t.contains('|') && !t.contains(':') {
        return None;
    }
    let cells = split_row(t);
    let mut aligns = Vec::with_capacity(cells.len());
    for c in &cells {
        let core = c.trim_matches(':');
        if core.is_empty() || !core.chars().all(|x| x == '-') {
            return None;
        }
        aligns.push(match (c.starts_with(':'), c.ends_with(':')) {
            (true, true) => Align::Center,
            (false, true) => Align::Right,
            _ => Align::Left,
        });
    }
    (!aligns.is_empty()).then_some(aligns)
}

/// Splits a table row at unescaped pipes; the outer pipes are optional.
fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cur.push('|');
                chars.next();
            }
            '|' => cells.push(std::mem::take(&mut cur).trim().to_owned()),
            _ => cur.push(c),
        }
    }
    // A trailing pipe closes the last cell; text after the last pipe is one more cell.
    if !cur.trim().is_empty() || !t.ends_with('|') {
        cells.push(cur.trim().to_owned());
    }
    cells
}

fn could_be_header(line: &str) -> bool {
    let t = line.trim_start();
    line.contains('|') && !t.starts_with('>') && line.len() - t.len() < 4
}

// ------------------------------------------------------------------- blocks

/// Splits `text` into Markdown and table blocks.
pub fn parse_blocks(text: &str) -> Vec<Block> {
    let mut lines: Vec<(usize, &str)> = Vec::new();
    let mut offset = 0;
    for l in text.split_inclusive('\n') {
        lines.push((offset, l.trim_end_matches(['\n', '\r'])));
        offset += l.len();
    }

    // Pass 1: table regions as (first line, delimiter line, one past the last row).
    let mut tables = Vec::new();
    let mut fence = Fence::default();
    let mut i = 0;
    while i < lines.len() {
        if fence.step(lines[i].1) {
            i += 1;
            continue;
        }
        if i + 1 < lines.len() && could_be_header(lines[i].1) {
            if let Some(aligns) = delimiter(lines[i + 1].1) {
                if split_row(lines[i].1).len() == aligns.len() {
                    let mut end = i + 2;
                    while end < lines.len() && !lines[end].1.trim().is_empty() && lines[end].1.contains('|') {
                        end += 1;
                    }
                    tables.push((i, end, aligns));
                    i = end;
                    continue;
                }
            }
        }
        i += 1;
    }
    if tables.is_empty() {
        return vec![Block::Markdown(markdown::parse(text).collect())];
    }

    // Link reference definitions live anywhere in the document; every Markdown
    // segment gets a copy so `[text][ref]` keeps working across a table.
    let refs: String = {
        let mut fence = Fence::default();
        lines
            .iter()
            .filter(|(_, l)| !fence.step(l) && is_reference_definition(l))
            .map(|(_, l)| format!("{l}\n"))
            .collect()
    };

    let segment = |from: usize, to: usize| -> Option<Block> {
        let body = &text[from..to];
        (!body.trim().is_empty()).then(|| Block::Markdown(markdown::parse(&format!("{body}\n{refs}")).collect()))
    };
    let mut blocks = Vec::new();
    let mut cursor = 0;
    for (first, end, aligns) in tables {
        let start = lines[first].0;
        blocks.extend(segment(cursor, start));
        let cell = |s: &str| Cell { items: markdown::parse(s).collect(), chars: s.chars().count() };
        let n = aligns.len();
        let row = |line: &str| -> Vec<Cell> {
            let mut cells: Vec<Cell> = split_row(line).iter().take(n).map(|s| cell(s)).collect();
            cells.resize_with(n, || cell(""));
            cells
        };
        blocks.push(Block::Table(Table {
            header: row(lines[first].1),
            rows: lines[first + 2..end].iter().map(|(_, l)| row(l)).collect(),
            aligns,
        }));
        cursor = lines.get(end).map_or(text.len(), |(o, _)| *o);
    }
    blocks.extend(segment(cursor, text.len()));
    blocks
}

fn is_reference_definition(line: &str) -> bool {
    let t = line.trim_start();
    line.len() - t.len() < 4
        && t.starts_with('[')
        && t.find("]:").is_some_and(|i| i > 1 && !t[1..i].contains(']') && t[i + 2..].trim().len() > 0)
}

/// Chunk start offsets: a cut is made at a line start that follows a blank
/// line (or, inside a table, between two rows), outside fenced code blocks,
/// once the chunk has grown past `CHUNK_BYTES`.
fn chunk_starts(doc: &TextDoc) -> Vec<Chunk> {
    let mut starts = vec![Chunk { start: 0, header: None }];
    let mut fence = Fence::default();
    let mut prev_blank = false;
    // Previous line, and the table we are inside of: (header + delimiter byte range).
    let mut prev: Option<(usize, String)> = None;
    let mut table: Option<(usize, usize)> = None;
    doc.state.for_each_line(|offset, line| {
        let in_fence = fence.step(line);
        if !in_fence {
            let blank = line.trim().is_empty();
            let grown = offset - starts[starts.len() - 1].start >= CHUNK_BYTES;
            if let Some(header) = table {
                if blank || !line.contains('|') {
                    table = None;
                } else if grown {
                    starts.push(Chunk { start: offset, header: Some(header) });
                }
            }
            if table.is_none() {
                if let Some((po, pl)) = &prev {
                    // This line is the delimiter row of a table whose header was the previous line.
                    if could_be_header(pl) && delimiter(line).is_some_and(|a| a.len() == split_row(pl).len()) {
                        table = Some((*po, offset + line.len() + 1));
                    }
                }
                if table.is_none() && prev_blank && grown {
                    starts.push(Chunk { start: offset, header: None });
                }
            }
            prev_blank = blank;
        } else {
            prev_blank = false;
            table = None;
        }
        prev = Some((offset, line.to_owned()));
    });
    starts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> TextDoc {
        TextDoc::new(text, false, 0)
    }

    fn big(blocks: usize) -> String {
        (0..blocks)
            .map(|i| format!("# Heading {i}\n\nParagraph {i} with `code` and *emphasis* and some more words to take space.\n\n```rust\nfn f{i}() {{\n\n    // blank line inside a fence\n}}\n```\n\n- a\n- b\n\n"))
            .collect()
    }

    fn table_doc(rows: usize) -> String {
        let mut s = String::from("# T\n\n| ID | Karar | Gerekçe |\n|---|---|---|\n");
        for i in 0..rows {
            s += &format!("| T-{i:03} | **bold {i}** decision | {} |\n", "long reasoning with words ".repeat(40));
        }
        s + "\nAfter the table.\n"
    }

    #[test]
    fn small_documents_are_rendered_whole() {
        let d = doc("# Hi\n\ntext\n");
        let p = Preview::build(&d, None);
        assert!(p.window.is_none() && p.weight() > 0 && p.is_current(&d));
    }

    #[test]
    fn tables_become_blocks_with_cells_alignment_and_escaped_pipes() {
        let src = "intro\n\n| A | B \\| pipe | C |\n|:--|:-:|--:|\n| 1 | **two** | `3` |\n| only one |\n\nafter\n";
        let blocks = parse_blocks(src);
        assert_eq!(blocks.len(), 3, "text, table, text");
        assert!(matches!(&blocks[0], Block::Markdown(i) if !i.is_empty()));
        let Block::Table(t) = &blocks[1] else { panic!("table") };
        assert_eq!(t.aligns, vec![Align::Left, Align::Center, Align::Right]);
        assert_eq!(t.header.len(), 3);
        assert_eq!(t.header[1].chars, "B | pipe".chars().count(), "escaped pipe stays in the cell");
        assert_eq!(t.rows.len(), 2);
        assert!(t.rows[1].len() == 3 && t.rows[1][2].chars == 0, "short rows are padded");
        assert!(matches!(&blocks[2], Block::Markdown(i) if !i.is_empty()));
    }

    #[test]
    fn not_everything_with_a_pipe_is_a_table() {
        // No delimiter row, a fenced "table", a quote, and a mismatched column count.
        for src in [
            "a | b\nplain text\n",
            "```\n| a | b |\n|---|---|\n| 1 | 2 |\n```\n",
            "> | a | b |\n> |---|---|\n",
            "| a | b | c |\n|---|---|\n| 1 | 2 | 3 |\n",
            "just --- rules\n\n---\n",
        ] {
            assert!(parse_blocks(src).iter().all(|b| matches!(b, Block::Markdown(_))), "{src:?}");
        }
        // Reference links keep working on both sides of a table.
        let src = "see [ref]\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nand [ref] again\n\n[ref]: https://example.org\n";
        let blocks = parse_blocks(src);
        assert_eq!(blocks.len(), 3);
        let has_link = |b: &Block| format!("{b:?}").contains("https://example.org");
        assert!(has_link(&blocks[0]) && has_link(&blocks[2]));
    }

    #[test]
    fn chunks_never_cut_inside_a_fenced_block_and_cover_everything() {
        let d = doc(&big(1500));
        let starts = chunk_starts(&d);
        assert!(starts.len() > 10 && starts[0].start == 0);
        assert!(starts.windows(2).all(|w| w[0].start < w[1].start));
        let text = d.text();
        for c in &starts[1..] {
            assert!(c.header.is_none());
            assert!(text[..c.start].ends_with("\n\n"), "cut at {}", c.start);
            let fences = text[..c.start].lines().filter(|l| l.trim_start().starts_with("```")).count();
            assert_eq!(fences % 2, 0, "cut at {} is inside a fence", c.start);
        }
    }

    #[test]
    fn a_giant_table_is_cut_between_rows_and_every_cut_repeats_the_header() {
        let d = doc(&table_doc(400)); // ~ 400 KB in one table without a blank line
        assert!(d.state.len_bytes() > 300_000);
        let text = d.text();
        let starts = chunk_starts(&d);
        assert!(starts.len() > 10, "{} chunks", starts.len());
        let mut inside = 0;
        for c in &starts[1..] {
            if text[c.start..].starts_with("| T-") {
                let (a, b) = c.header.expect("cut inside a table carries its header");
                assert_eq!(&text[a..b], "| ID | Karar | Gerekçe |\n|---|---|---|\n");
                assert!(text[..c.start].ends_with("|\n"), "cut between rows");
                inside += 1;
            } else {
                // The only other cut: the text after the table.
                assert!(c.header.is_none() && text[c.start..].starts_with("After the table"));
            }
        }
        assert!(inside > 10, "{inside} cuts inside the table");
        // A window in the middle shows a table that still has its header, and few rows.
        let mut d = d;
        d.state.set_metrics(crate::editor::state::Metrics { char_w: 8.0, line_h: 20.0, view_h: 600.0, text_w: 800.0 });
        d.state.apply(crate::editor::state::Action::ScrollTo { y: 20.0 * 200.0 }, std::time::Instant::now());
        let p = Preview::build(&d, None);
        let Some(Block::Table(t)) = p.blocks.iter().find(|b| matches!(b, Block::Table(_))) else { panic!("a table") };
        assert_eq!(t.header.len(), 3);
        assert!(t.rows.len() < 60 && t.rows.len() > 3, "{} rows", t.rows.len());
        assert!(p.weight() < 80);
    }

    #[test]
    fn a_big_document_shows_only_a_window_around_the_scroll_position() {
        let d = doc(&big(4000)); // ~ 1 MiB
        assert!(d.state.len_bytes() > 600_000);
        let p = Preview::build(&d, None);
        let w = p.window.expect("windowed");
        assert_eq!(w.first_line, 1, "top of the document");
        assert!(w.last_line < w.total_lines / 5 && p.weight() < 2500, "{w:?} {}", p.weight());
        assert!(p.is_current(&d));

        // Moving the editor's view far down moves the window; the old one is stale.
        d.state.set_metrics(crate::editor::state::Metrics { char_w: 8.0, line_h: 20.0, view_h: 600.0, text_w: 800.0 });
        let mut d = d;
        d.state.apply(crate::editor::state::Action::ScrollTo { y: 20.0 * (w.total_lines as f32 / 2.0) }, std::time::Instant::now());
        assert!(!p.is_current(&d), "scrolled away from the shown part");
        let q = Preview::build(&d, Some(p));
        let w2 = q.window.unwrap();
        let top_line = d.state.line_of_byte(d.state.top_byte()) + 1;
        assert!((w2.first_line..=w2.last_line).contains(&top_line), "{w2:?} top {top_line}");
        assert!(w2.first_line > w.last_line && q.is_current(&d));
        let f = q.scroll_fraction(&d);
        assert!((0.0..=1.0).contains(&f));
    }
}
