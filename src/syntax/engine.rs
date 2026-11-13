//! Incremental parsing + highlight-query evaluation. Blocking; runs on workers.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Language, Parser, Point, Query, QueryCursor, Range, Tree};

use super::{Group, Lang, lexer};

/// A parsed buffer, kept between edits so the next parse is incremental.
#[derive(Clone)]
pub struct ParseState {
    pub lang: Lang,
    pub text: Arc<String>,
    /// `None` for the grammar-less lexer languages.
    tree: Option<Tree>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// Byte offsets within the line.
    pub start: u32,
    pub end: u32,
    pub group: Group,
}

/// Highlights of a whole buffer: non-overlapping spans per line.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LineSpans {
    spans: Vec<Span>,
    /// `spans[offsets[i]..offsets[i + 1]]` belong to line `i`.
    offsets: Vec<u32>,
}

impl LineSpans {
    pub fn line(&self, i: usize) -> &[Span] {
        match (self.offsets.get(i), self.offsets.get(i + 1)) {
            (Some(&a), Some(&b)) => &self.spans[a as usize..b as usize],
            _ => &[],
        }
    }

    #[cfg(test)]
    pub fn line_count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }
}

/// Compiled grammar + query + capture-index -> class table (built once per language).
struct Config {
    language: Language,
    query: Query,
    groups: Vec<Option<Group>>,
    /// Regions that are written in another language (PHP -> HTML, HTML -> JS/CSS, ...).
    inject: Option<Inject>,
}

/// One query whose pattern `i` captures the regions written in `langs[i]`.
struct Inject {
    query: Query,
    langs: Vec<Lang>,
}

fn config(lang: Lang) -> Option<Arc<Config>> {
    static CACHE: OnceLock<Mutex<HashMap<Lang, Option<Arc<Config>>>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    cache
        .entry(lang)
        .or_insert_with(|| match build_config(lang) {
            Ok(c) => Some(Arc::new(c)),
            Err(e) => {
                eprintln!("veloedit: highlighting disabled for {}: {e}", lang.name());
                None
            }
        })
        .clone()
}

fn build_config(lang: Lang) -> Result<Config, String> {
    let (language, query): (Language, String) = match lang {
        Lang::Rust => (tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY.into()),
        Lang::Python => (tree_sitter_python::LANGUAGE.into(), tree_sitter_python::HIGHLIGHTS_QUERY.into()),
        Lang::JavaScript => (
            tree_sitter_javascript::LANGUAGE.into(),
            format!("{}\n{}", tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY),
        ),
        // TypeScript's query only adds to JavaScript's (see its README).
        Lang::TypeScript => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            format!("{}\n{}", tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_typescript::HIGHLIGHTS_QUERY),
        ),
        Lang::Tsx => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            format!(
                "{}\n{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
        ),
        Lang::Json => (tree_sitter_json::LANGUAGE.into(), tree_sitter_json::HIGHLIGHTS_QUERY.into()),
        Lang::Toml => (tree_sitter_toml_ng::LANGUAGE.into(), tree_sitter_toml_ng::HIGHLIGHTS_QUERY.into()),
        Lang::Bash => (tree_sitter_bash::LANGUAGE.into(), tree_sitter_bash::HIGHLIGHT_QUERY.into()),
        Lang::C => (tree_sitter_c::LANGUAGE.into(), tree_sitter_c::HIGHLIGHT_QUERY.into()),
        Lang::Java => (tree_sitter_java::LANGUAGE.into(), tree_sitter_java::HIGHLIGHTS_QUERY.into()),
        // `LANGUAGE_PHP` understands `<?php ... ?>` inside HTML (the usual PHP file).
        Lang::Php => (tree_sitter_php::LANGUAGE_PHP.into(), tree_sitter_php::HIGHLIGHTS_QUERY.into()),
        Lang::Ruby => (tree_sitter_ruby::LANGUAGE.into(), tree_sitter_ruby::HIGHLIGHTS_QUERY.into()),
        Lang::Lua => (tree_sitter_lua::LANGUAGE.into(), tree_sitter_lua::HIGHLIGHTS_QUERY.into()),
        Lang::Html => (tree_sitter_html::LANGUAGE.into(), tree_sitter_html::HIGHLIGHTS_QUERY.into()),
        Lang::Css => (tree_sitter_css::LANGUAGE.into(), tree_sitter_css::HIGHLIGHTS_QUERY.into()),
        Lang::Scss => (tree_sitter_scss::language(), tree_sitter_scss::HIGHLIGHTS_QUERY.into()),
        Lang::Xml => (tree_sitter_xml::LANGUAGE_XML.into(), tree_sitter_xml::XML_HIGHLIGHT_QUERY.into()),
        Lang::Yaml => (tree_sitter_yaml::LANGUAGE.into(), tree_sitter_yaml::HIGHLIGHTS_QUERY.into()),
        Lang::Go => (tree_sitter_go::LANGUAGE.into(), tree_sitter_go::HIGHLIGHTS_QUERY.into()),
        Lang::Markdown => (tree_sitter_md::LANGUAGE.into(), tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.into()),
        Lang::MarkdownInline => {
            (tree_sitter_md::INLINE_LANGUAGE.into(), tree_sitter_md::HIGHLIGHT_QUERY_INLINE.into())
        }
        Lang::Make => (tree_sitter_make::LANGUAGE.into(), tree_sitter_make::HIGHLIGHTS_QUERY.into()),
        Lang::CMake => (tree_sitter_cmake::LANGUAGE.into(), tree_sitter_cmake::HIGHLIGHTS_QUERY.into()),
        Lang::Ini => (tree_sitter_ini::LANGUAGE.into(), tree_sitter_ini::HIGHLIGHTS_QUERY.into()),
        Lang::Diff => (tree_sitter_diff::LANGUAGE.into(), tree_sitter_diff::HIGHLIGHTS_QUERY.into()),
        Lang::Twig | Lang::Lex(_) | Lang::Apache => return Err("no grammar of its own".into()),
    };
    let query = Query::new(&language, &query).map_err(|e| e.to_string())?;
    let groups = query.capture_names().iter().map(|n| Group::from_capture(n)).collect();
    let inject = match injection_rules(lang) {
        Some((src, langs)) => Some(Inject { query: Query::new(&language, src).map_err(|e| e.to_string())?, langs }),
        None => None,
    };
    Ok(Config { language, query, groups, inject })
}

/// Which node kinds hold code of another language (pattern `i` -> `langs[i]`).
fn injection_rules(lang: Lang) -> Option<(&'static str, Vec<Lang>)> {
    Some(match lang {
        Lang::Php => ("(text) @c", vec![Lang::Html]),
        Lang::Html => (
            "(script_element (raw_text) @c) (style_element (raw_text) @c)",
            vec![Lang::JavaScript, Lang::Css],
        ),
        Lang::Markdown => ("(inline) @c", vec![Lang::MarkdownInline]),
        _ => return None,
    })
}

const NONE: u8 = u8::MAX;

/// Parses `text` (incrementally when `prev` is for the same language) and
/// returns the new parse state plus per-line highlight spans. Blocking.
pub fn highlight(lang: Lang, prev: Option<&ParseState>, text: Arc<String>) -> Option<(ParseState, LineSpans)> {
    if lang == Lang::Apache {
        let paint = lexer::paint_apache(&text);
        let spans = spans_from_paint(&paint, &text);
        return Some((ParseState { lang, text, tree: None }, spans));
    }
    if let Lang::Lex(i) = lang {
        let paint = lexer::paint(lexer::spec(i)?, &text);
        let spans = spans_from_paint(&paint, &text);
        return Some((ParseState { lang, text, tree: None }, spans));
    }
    // Template languages are their HTML grammar plus an overlay for the tags.
    let cfg = config(if lang == Lang::Twig { Lang::Html } else { lang })?;
    let mut parser = Parser::new();
    parser.set_language(&cfg.language).ok()?;

    let old_tree = prev.filter(|p| p.lang == lang).and_then(|p| {
        let mut tree = p.tree.clone()?;
        tree.edit(&diff_edit(&p.text, &text));
        Some(tree)
    });
    let tree = parser.parse(text.as_bytes(), old_tree.as_ref())?;

    // Injected languages first, the host's own captures on top.
    let mut paint = vec![NONE; text.len()];
    paint_injections(&cfg, &tree, text.as_bytes(), &mut paint, 0);
    paint_captures(&cfg, &tree, text.as_bytes(), &mut paint);
    if lang == Lang::Twig {
        lexer::paint_template(&text, &mut paint);
    }
    let spans = spans_from_paint(&paint, &text);
    Some((ParseState { lang, text, tree: Some(tree) }, spans))
}

/// Paints every captured range onto a per-byte class map (later captures win,
/// matching tree-sitter's "later pattern overrides" convention).
fn paint_captures(cfg: &Config, tree: &Tree, bytes: &[u8], paint: &mut [u8]) {
    let mut cursor = QueryCursor::new();
    let mut captures = cursor.captures(&cfg.query, tree.root_node(), bytes);
    while let Some((m, idx)) = captures.next() {
        let cap = m.captures()[*idx];
        if let Some(Some(group)) = cfg.groups.get(cap.index as usize) {
            let r = cap.node.byte_range();
            paint[r.start.min(bytes.len())..r.end.min(bytes.len())].fill(*group as u8);
        }
    }
}

/// Highlights the regions written in another language by parsing just those
/// ranges with the other grammar (tree-sitter "included ranges").
fn paint_injections(cfg: &Config, tree: &Tree, bytes: &[u8], paint: &mut [u8], depth: u8) {
    const MAX_DEPTH: u8 = 2;
    let Some(inject) = &cfg.inject else { return };
    if depth >= MAX_DEPTH {
        return;
    }
    let mut by_lang: Vec<(Lang, Vec<Range>)> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&inject.query, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        let Some(&lang) = inject.langs.get(m.pattern_index) else { continue };
        for cap in m.captures() {
            let r = cap.node.range();
            if r.end_byte <= r.start_byte {
                continue;
            }
            match by_lang.iter_mut().find(|(l, _)| *l == lang) {
                Some((_, ranges)) => ranges.push(r),
                None => by_lang.push((lang, vec![r])),
            }
        }
    }
    for (lang, ranges) in by_lang {
        let Some(sub) = config(lang) else { continue };
        let mut parser = Parser::new();
        if parser.set_language(&sub.language).is_err() || parser.set_included_ranges(&ranges).is_err() {
            continue;
        }
        let Some(subtree) = parser.parse(bytes, None) else { continue };
        paint_injections(&sub, &subtree, bytes, paint, depth + 1);
        paint_captures(&sub, &subtree, bytes, paint);
    }
}

/// Run-length encodes the per-byte class map into per-line spans.
fn spans_from_paint(paint: &[u8], text: &str) -> LineSpans {
    let mut out = LineSpans::default();
    let mut line_start = 0;
    for line in text.split('\n') {
        out.offsets.push(out.spans.len() as u32);
        // `\r` belongs to the line ending, not to the text editor's line.
        let content_len = line.strip_suffix('\r').map_or(line.len(), str::len);
        let cells = &paint[line_start..line_start + content_len];
        let mut i = 0;
        while i < cells.len() {
            let g = cells[i];
            let mut j = i + 1;
            while j < cells.len() && cells[j] == g {
                j += 1;
            }
            if g != NONE {
                out.spans.push(Span { start: i as u32, end: j as u32, group: GROUPS[g as usize] });
            }
            i = j;
        }
        line_start += line.len() + 1;
    }
    out.offsets.push(out.spans.len() as u32);
    out
}

const GROUPS: [Group; Group::COUNT] = [
    Group::Keyword, Group::Function, Group::Type, Group::String, Group::Comment, Group::Number,
    Group::Constant, Group::Operator, Group::Property, Group::Punctuation, Group::Attribute,
    Group::Tag, Group::Escape,
];

/// The single edit that turns `old` into `new` (common prefix/suffix trimmed).
/// Typing, pasting and deleting are all one such edit.
pub(super) fn diff_edit(old: &str, new: &str) -> InputEdit {
    let (o, n) = (old.as_bytes(), new.as_bytes());
    let prefix = o.iter().zip(n).take_while(|(a, b)| a == b).count();
    let max_suffix = o.len().min(n.len()) - prefix;
    let suffix = o[prefix..]
        .iter()
        .rev()
        .zip(n[prefix..].iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let (old_end, new_end) = (o.len() - suffix, n.len() - suffix);
    InputEdit {
        start_byte: prefix,
        old_end_byte: old_end,
        new_end_byte: new_end,
        start_position: point_at(o, prefix),
        old_end_position: point_at(o, old_end),
        new_end_position: point_at(n, new_end),
    }
}

fn point_at(text: &[u8], byte: usize) -> Point {
    let upto = &text[..byte];
    let row = upto.iter().filter(|&&b| b == b'\n').count();
    let column = upto.iter().rev().take_while(|&&b| b != b'\n').count();
    Point { row, column }
}
