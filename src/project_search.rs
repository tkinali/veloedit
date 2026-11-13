//! Search across the workspace (Ctrl+Shift+F): a `.gitignore`-aware walk, text
//! files only, regex per file, optionally narrowed by a file filter such as
//! `*.php, !vendor`. Blocking; the app runs it on a worker and aborts it (via
//! `cancelled`) as soon as the query changes.

use std::{fs, path::{Path, PathBuf}};

use regex::Regex;

use crate::detect::{self, FileKind, SNIFF_LEN};

/// Stop after this many hits (the list is for navigating, not for reporting).
pub const MAX_HITS: usize = 2000;
/// Larger files are skipped (generated bundles, dumps).
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Preview text shown per hit.
const PREVIEW_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    /// Shown label: path relative to its root (root name prefixed with several roots).
    pub rel: String,
    /// 0-based line and character column of the match.
    pub line: usize,
    pub col: usize,
    /// Match length in characters, clipped to the end of its line.
    pub len: usize,
    pub preview: String,
    /// Where the match sits inside `preview`: `(start, len)` in characters.
    pub mark: (usize, usize),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub hits: Vec<Hit>,
    pub files: usize,
    pub truncated: bool,
}

/// `*.php, *.js` includes only those files; `!vendor` / `-vendor` leaves a
/// folder or file out. Empty = everything. Unparsable globs are dropped.
fn overrides(root: &Path, filter: &str) -> Option<ignore::overrides::Override> {
    let mut b = ignore::overrides::OverrideBuilder::new(root);
    let mut any = false;
    for glob in filter.split([',', ' ']).map(str::trim).filter(|g| !g.is_empty()) {
        let glob = match glob.strip_prefix('-') {
            Some(rest) if !rest.is_empty() => format!("!{rest}"),
            _ => glob.to_owned(),
        };
        // `src` should also match `src/...` like a gitignore line does.
        if b.add(&glob).is_ok() {
            any = true;
        }
    }
    if any { b.build().ok() } else { None }
}

pub fn run(roots: &[PathBuf], filter: &str, re: &Regex, cancelled: impl Fn() -> bool) -> Outcome {
    let mut out = Outcome::default();
    let prefix = roots.len() > 1;
    for root in roots {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut walker = ignore::WalkBuilder::new(root);
        walker
            .hidden(false)
            .require_git(false)
            .follow_links(false)
            .sort_by_file_path(|a, b| a.cmp(b)) // stable, alphabetical results
            .filter_entry(|e| e.file_name() != ".git");
        if let Some(o) = overrides(root, filter) {
            walker.overrides(o);
        }
        let walker = walker.build();
        for entry in walker.filter_map(Result::ok) {
            if cancelled() {
                return out;
            }
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let rel = entry.path().strip_prefix(root).unwrap_or(entry.path()).to_string_lossy().replace('\\', "/");
            let label = if prefix { format!("{name}/{rel}") } else { rel };
            if search_file(entry.path(), &label, re, &mut out) {
                out.truncated = true;
                return out;
            }
        }
    }
    out
}

/// Returns `true` when the hit limit was reached.
fn search_file(path: &Path, label: &str, re: &Regex, out: &mut Outcome) -> bool {
    let Ok(meta) = fs::metadata(path) else { return false };
    if meta.len() > MAX_FILE_BYTES {
        return false;
    }
    let Ok(bytes) = fs::read(path) else { return false };
    let ext = path.extension().and_then(|e| e.to_str());
    if detect::classify(&bytes[..bytes.len().min(SNIFF_LEN)], ext) != FileKind::Text {
        return false;
    }
    let Ok(text) = String::from_utf8(bytes) else { return false };
    out.files += 1;

    // Incremental line bookkeeping: matches arrive in ascending order.
    let (mut line, mut line_start) = (0usize, 0usize);
    for m in re.find_iter(&text) {
        line += text[line_start..m.start()].matches('\n').count();
        if let Some(nl) = text[line_start..m.start()].rfind('\n') {
            line_start += nl + 1;
        }
        let line_end = text[line_start..].find('\n').map_or(text.len(), |i| line_start + i);
        let line_text = text[line_start..line_end].trim_end_matches('\r');
        let col = text[line_start..m.start()].chars().count();
        let len = text[m.start()..m.end().min(line_end)].chars().count();
        let (preview, mark) = preview(line_text, col, len.max(1));
        out.hits.push(Hit { path: path.to_path_buf(), rel: label.to_owned(), line, col, len: len.max(1), preview, mark });
        if out.hits.len() >= MAX_HITS {
            return true;
        }
    }
    false
}

/// The line, trimmed of indentation, centered on the match when it is long.
/// Also returns where the match sits in the returned text (chars).
fn preview(line: &str, col: usize, len: usize) -> (String, (usize, usize)) {
    let trimmed = line.trim_start();
    let lost = line.chars().count() - trimmed.chars().count();
    let col = col.saturating_sub(lost);
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= PREVIEW_CHARS {
        let len = len.min(chars.len().saturating_sub(col));
        return (trimmed.to_owned(), (col.min(chars.len()), len));
    }
    let start = col.saturating_sub(PREVIEW_CHARS / 3).min(chars.len() - PREVIEW_CHARS);
    let body: String = chars[start..start + PREVIEW_CHARS].iter().collect();
    let lead = usize::from(start > 0);
    let trail = start + PREVIEW_CHARS < chars.len();
    let mark_start = (col - start.min(col)) + lead;
    let mark_len = len.min(PREVIEW_CHARS.saturating_sub(col - start.min(col)));
    (
        format!("{}{body}{}", if lead == 1 { "\u{2026}" } else { "" }, if trail { "\u{2026}" } else { "" }),
        (mark_start, mark_len),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::Query;

    fn tree(tag: &str) -> PathBuf {
        // One directory per test: tests run in parallel.
        let dir = std::env::temp_dir().join(format!("veloedit-psearch-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("target")).unwrap();
        fs::write(dir.join(".gitignore"), "target/\n").unwrap();
        fs::write(dir.join("src/a.rs"), "fn alpha() {}\n    let needle = 1; // needle again\r\nfn omega() {}\n").unwrap();
        fs::write(dir.join("src/b.txt"), "no match here\nNEEDLE shouting\n").unwrap();
        fs::write(dir.join("target/junk.rs"), "needle in ignored dir\n").unwrap();
        fs::write(dir.join("blob.bin"), b"needle\0\0binary").unwrap();
        fs::write(dir.join("big.txt"), "needle ".repeat(400_000)).unwrap(); // > 2 MB
        dir
    }

    fn re(text: &str, case: bool) -> Regex {
        Query { text: text.into(), case_sensitive: case, ..Query::default() }.compile().unwrap().unwrap()
    }

    #[test]
    fn finds_text_files_only_with_positions_and_previews() {
        let dir = tree("find");
        let out = run(std::slice::from_ref(&dir), "", &re("needle", false), || false);
        let mut got: Vec<(String, usize, usize, usize, String)> = out
            .hits
            .iter()
            .map(|h| (h.rel.clone(), h.line, h.col, h.len, h.preview.clone()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                ("src/a.rs".into(), 1, 8, 6, "let needle = 1; // needle again".into()),
                ("src/a.rs".into(), 1, 23, 6, "let needle = 1; // needle again".into()),
                ("src/b.txt".into(), 1, 0, 6, "NEEDLE shouting".into()),
            ],
            "ignored dir, binary and oversized files are skipped; CRLF is trimmed"
        );
        assert!(!out.truncated);
        assert_eq!(out.files, 3 + 0, "a.rs, b.txt and .gitignore were read ({} files)", out.files);
        // Case-sensitive drops the shouting one.
        assert_eq!(run(&[dir.clone()], "", &re("needle", true), || false).hits.len(), 2);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn can_be_cancelled_and_caps_the_hit_count() {
        let dir = tree("cancel");
        assert!(run(&[dir.clone()], "", &re("needle", false), || true).hits.is_empty());
        let many = dir.join("many.txt");
        fs::write(&many, "x\n".repeat(MAX_HITS + 500)).unwrap();
        let out = run(&[dir.clone()], "", &re("x", false), || false);
        assert_eq!(out.hits.len(), MAX_HITS);
        assert!(out.truncated);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn previews_are_centered_on_long_lines() {
        let line = format!("{}FOUND{}", "a".repeat(300), "b".repeat(300));
        let (p, (ms, ml)) = preview(&line, 300, 5);
        assert!(p.contains("FOUND") && p.starts_with('\u{2026}') && p.ends_with('\u{2026}'), "{p}");
        assert!(p.chars().count() <= PREVIEW_CHARS + 2);
        assert_eq!(p.chars().skip(ms).take(ml).collect::<String>(), "FOUND", "the mark follows the match");
        let (p, mark) = preview("   indented word", 12, 4);
        assert_eq!((p.as_str(), mark), ("indented word", (9, 4)));
        // A match near the end of a long line is still marked correctly.
        let tail = format!("{}END", "x".repeat(400));
        let (p, (ms, ml)) = preview(&tail, 400, 3);
        assert_eq!(p.chars().skip(ms).take(ml).collect::<String>(), "END");
    }

    #[test]
    fn file_filters_include_and_exclude() {
        let dir = tree("filter");
        fs::create_dir_all(dir.join("vendor")).unwrap();
        fs::write(dir.join("vendor/v.rs"), "needle in vendor\n").unwrap();
        let rels = |filter: &str| {
            let mut v: Vec<String> =
                run(std::slice::from_ref(&dir), filter, &re("needle", false), || false).hits.iter().map(|h| h.rel.clone()).collect();
            v.sort();
            v.dedup();
            v
        };
        assert_eq!(rels("*.txt"), vec!["src/b.txt"]);
        assert_eq!(rels("*.rs, *.txt"), vec!["src/a.rs", "src/b.txt", "vendor/v.rs"]);
        assert_eq!(rels("!vendor"), vec!["src/a.rs", "src/b.txt"]);
        assert_eq!(rels("*.rs -vendor"), vec!["src/a.rs"]);
        assert_eq!(rels("   "), vec!["src/a.rs", "src/b.txt", "vendor/v.rs"], "blank = everything");
        fs::remove_dir_all(dir).ok();
    }
}
