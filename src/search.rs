//! Find / replace engine. Pure functions over `&str`; the app runs them on a
//! worker. Every query - literal or regex - goes through the `regex` crate
//! (literals are escaped), so case folding, `\b` word boundaries and
//! replacement templates behave identically in both modes.
//!
//! Positions are **character** offsets (what the editor's rope uses).

use regex::{NoExpand, Regex, RegexBuilder};

/// Highlighting millions of hits helps nobody and costs memory.
pub const MAX_MATCHES: usize = 100_000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

/// A replacement in character coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

impl Query {
    /// `Ok(None)` for an empty query; `Err` carries a short message for the UI.
    pub fn compile(&self) -> Result<Option<Regex>, String> {
        if self.text.is_empty() {
            return Ok(None);
        }
        let mut pattern = if self.regex { self.text.clone() } else { regex::escape(&self.text) };
        if self.whole_word {
            pattern = format!(r"\b(?:{pattern})\b");
        }
        let re = RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .size_limit(10 << 20)
            .build()
            .map_err(|e| {
                // The regex crate's message spans many lines; keep the last one.
                e.to_string().lines().last().unwrap_or("invalid pattern").trim().to_owned()
            })?;
        if re.is_match("") {
            return Err("pattern matches empty text".to_owned());
        }
        Ok(Some(re))
    }
}

/// All matches as `(start, end)` char ranges, ascending, at most [`MAX_MATCHES`].
pub fn find_all(text: &str, re: &Regex) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut byte, mut chars) = (0usize, 0usize);
    for m in re.find_iter(text) {
        chars += text[byte..m.start()].chars().count();
        let len = m.as_str().chars().count();
        out.push((chars, chars + len));
        chars += len;
        byte = m.end();
        if out.len() >= MAX_MATCHES {
            break;
        }
    }
    out
}

/// Replaces every match. Returns the *smallest* single edit that turns `text`
/// into the result (so it is one undo step and cheap to apply) and the number
/// of replaced matches; `None` when nothing changes.
pub fn replace_all(text: &str, re: &Regex, replacement: &str, literal: bool) -> Option<(Edit, usize)> {
    let count = re.find_iter(text).count();
    let out = if literal {
        re.replace_all(text, NoExpand(replacement))
    } else {
        re.replace_all(text, replacement)
    };
    let new: &str = &out;
    if new == text {
        return None;
    }
    // Minimal differing window, on char boundaries of both strings.
    let (a, b) = (text.as_bytes(), new.as_bytes());
    let mut prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    while !(text.is_char_boundary(prefix) && new.is_char_boundary(prefix)) {
        prefix -= 1;
    }
    let max_suffix = a.len().min(b.len()) - prefix;
    let mut suffix = a.iter().rev().zip(b.iter().rev()).take(max_suffix).take_while(|(x, y)| x == y).count();
    while !(text.is_char_boundary(a.len() - suffix) && new.is_char_boundary(b.len() - suffix)) {
        suffix -= 1;
    }
    let start = text[..prefix].chars().count();
    let end = start + text[prefix..a.len() - suffix].chars().count();
    Some((Edit { start, end, text: new[prefix..b.len() - suffix].to_owned() }, count))
}

/// The replacement for the single match that starts at char offset `at`.
pub fn replace_at(text: &str, re: &Regex, at: usize, replacement: &str, literal: bool) -> Option<Edit> {
    let byte = text.char_indices().nth(at).map_or(text.len(), |(i, _)| i);
    let caps = re.captures_at(text, byte)?;
    let m = caps.get(0)?;
    if m.start() != byte {
        return None;
    }
    let mut out = String::new();
    if literal {
        out.push_str(replacement);
    } else {
        caps.expand(replacement, &mut out);
    }
    Some(Edit { start: at, end: at + m.as_str().chars().count(), text: out })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(text: &str) -> Query {
        Query { text: text.into(), ..Query::default() }
    }

    fn re(query: &Query) -> Regex {
        query.compile().unwrap().unwrap()
    }

    #[test]
    fn literal_case_and_whole_word() {
        let t = "Cat cat concat cat.";
        assert_eq!(find_all(t, &re(&q("cat"))), vec![(0, 3), (4, 7), (11, 14), (15, 18)], "case-insensitive by default");
        let sens = Query { case_sensitive: true, ..q("cat") };
        assert_eq!(find_all(t, &re(&sens)).len(), 3);
        let word = Query { whole_word: true, ..q("cat") };
        assert_eq!(find_all(t, &re(&word)), vec![(0, 3), (4, 7), (15, 18)]);
        // Regex metacharacters are literal unless regex mode is on.
        assert_eq!(find_all("a.b axb", &re(&q("a.b"))), vec![(0, 3)]);
        let rx = Query { regex: true, ..q("a.b") };
        assert_eq!(find_all("a.b axb", &re(&rx)).len(), 2);
    }

    #[test]
    fn offsets_are_characters_not_bytes() {
        let t = "ğüş ğ\n日本 ğ";
        assert_eq!(find_all(t, &re(&q("ğ"))), vec![(0, 1), (4, 5), (9, 10)]);
        assert_eq!(find_all(t, &re(&q("日本"))), vec![(6, 8)]);
    }

    #[test]
    fn multiline_anchors_and_errors() {
        let rx = Query { regex: true, ..q("^fn \\w+") };
        assert_eq!(find_all("fn a\nlet x\nfn b", &re(&rx)), vec![(0, 4), (11, 15)], "^ matches at every line start");
        assert!(Query { regex: true, ..q("(") }.compile().is_err());
        assert!(Query { regex: true, ..q("x*") }.compile().unwrap_err().contains("empty"));
        assert!(matches!(q("").compile(), Ok(None)));
    }

    #[test]
    fn replace_all_is_one_minimal_edit() {
        let t = "keep foo and foo, keep";
        let (e, n) = replace_all(t, &re(&q("foo")), "bar", true).unwrap();
        assert_eq!(n, 2);
        assert_eq!(e, Edit { start: 5, end: 16, text: "bar and bar".into() });
        // Applying the edit reproduces the replace_all output.
        let mut chars: Vec<char> = t.chars().collect();
        chars.splice(e.start..e.end, e.text.chars());
        assert_eq!(chars.into_iter().collect::<String>(), "keep bar and bar, keep");
        assert!(replace_all("nothing", &re(&q("zzz")), "y", true).is_none());
        assert!(replace_all("aaa", &re(&q("a")), "a", true).is_none(), "no-op replacement changes nothing");
    }

    #[test]
    fn regex_templates_and_literal_dollar() {
        let rx = Query { regex: true, ..q(r"(\w+)@(\w+)") };
        let t = "ali@home, veli@work";
        let (e, _) = replace_all(t, &re(&rx), "$2:$1", false).unwrap();
        assert_eq!(e.text, "home:ali, work:veli");
        // A literal query never expands `$1`.
        let (e, _) = replace_all("a b", &re(&q("a")), "$1", true).unwrap();
        assert_eq!(e.text, "$1");
        // Multibyte edge: the edit window must not split a character.
        let (e, _) = replace_all("çç çç", &re(&q("ç")), "c", true).unwrap();
        assert_eq!(e, Edit { start: 0, end: 5, text: "cc cc".into() });
    }

    #[test]
    fn replace_at_targets_one_match() {
        let t = "x-1 x-2 x-3";
        let rx = Query { regex: true, ..q(r"x-(\d)") };
        let e = replace_at(t, &re(&rx), 4, "n$1", false).unwrap();
        assert_eq!(e, Edit { start: 4, end: 7, text: "n2".into() });
        assert!(replace_at(t, &re(&rx), 5, "n$1", false).is_none(), "no match starts exactly there");
    }

    #[test]
    fn match_count_is_capped() {
        let t = "a".repeat(MAX_MATCHES + 50);
        assert_eq!(find_all(&t, &re(&q("a"))).len(), MAX_MATCHES);
    }
}
