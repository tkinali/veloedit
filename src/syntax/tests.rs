use std::{path::Path, sync::Arc};

use super::*;

fn spans_of(lang: Lang, src: &str) -> LineSpans {
    highlight(lang, None, Arc::new(src.to_owned())).expect("parses").1
}

fn groups_on(spans: &LineSpans, line: usize, src: &str) -> Vec<(String, Group)> {
    let text = src.split('\n').nth(line).unwrap();
    spans
        .line(line)
        .iter()
        .map(|s| (text[s.start as usize..s.end as usize].to_owned(), s.group))
        .collect()
}

#[test]
fn language_detection() {
    assert_eq!(Lang::detect(Path::new("src/main.rs")), Some(Lang::Rust));
    assert_eq!(Lang::detect(Path::new("A.TSX")), Some(Lang::Tsx));
    assert_eq!(Lang::detect(Path::new("Cargo.lock")), Some(Lang::Toml));
    assert_eq!(Lang::detect(Path::new("notes.txt")), None);
    assert_eq!(Lang::detect(Path::new("Makefile")), Some(Lang::Make));
    assert_eq!(Lang::detect(Path::new("app/Http/index.php")), Some(Lang::Php));
    assert_eq!(Lang::detect(Path::new("resources/views/a.blade.php")), Some(Lang::Php));
    assert_eq!(Lang::detect(Path::new("x.PHTML")), Some(Lang::Php));
    assert_eq!(Lang::detect(Path::new(".env.local")), Some(Lang::Ini));
    assert_eq!(Lang::detect(Path::new("Gemfile")), Some(Lang::Ruby));
    assert_eq!(Lang::detect(Path::new("main.cpp")).map(Lang::name), Some("C++"));
    assert_eq!(Lang::detect(Path::new("Program.cs")).map(Lang::name), Some("C#"));
    assert_eq!(Lang::detect(Path::new("schema.SQL")).map(Lang::name), Some("SQL"));
    assert!(matches!(Lang::detect(Path::new("Main.kt")), Some(Lang::Lex(_))));
    assert_eq!(Lang::detect(Path::new("Dockerfile")).map(Lang::name), Some("Dockerfile"));
    assert_eq!(Lang::detect(Path::new("a.php")).map(Lang::name), Some("PHP"));
}

#[test]
fn rust_tokens_get_the_right_classes() {
    let src = "// hi\nfn main() {\n    let s = \"a\\n\"; // c\n}\n";
    let spans = spans_of(Lang::Rust, src);
    assert_eq!(spans.line_count(), 5, "one entry per line incl. the empty last one");

    let l0 = groups_on(&spans, 0, src);
    assert_eq!(l0, vec![("// hi".into(), Group::Comment)]);

    let l1 = groups_on(&spans, 1, src);
    assert!(l1.contains(&("fn".into(), Group::Keyword)), "{l1:?}");
    assert!(l1.contains(&("main".into(), Group::Function)), "{l1:?}");

    let l2 = groups_on(&spans, 2, src);
    assert!(l2.contains(&("let".into(), Group::Keyword)), "{l2:?}");
    assert!(l2.iter().any(|(t, g)| *g == Group::String && t.starts_with('"')), "{l2:?}");
    assert!(l2.iter().any(|(t, g)| *g == Group::Escape && t == "\\n"), "{l2:?}");
    assert!(l2.contains(&("// c".into(), Group::Comment)), "{l2:?}");
}

#[test]
fn multi_line_constructs_span_every_line() {
    let src = "/* a\n b\n c */ fn x() {}";
    let spans = spans_of(Lang::Rust, src);
    for l in 0..3 {
        assert!(
            groups_on(&spans, l, src).iter().any(|(_, g)| *g == Group::Comment),
            "line {l} must be comment"
        );
    }
}

#[test]
fn utf8_and_crlf_offsets_stay_inside_the_line() {
    let src = "let s = \"ğüşiöç\";\r\n// ünlü\r\nfn f() {}";
    let spans = spans_of(Lang::Rust, src);
    for (i, line) in src.split('\n').enumerate() {
        let content = line.trim_end_matches('\r');
        for s in spans.line(i) {
            assert!((s.end as usize) <= content.len(), "line {i}: {s:?} exceeds {content:?}");
            assert!(content.is_char_boundary(s.start as usize) && content.is_char_boundary(s.end as usize));
        }
    }
    assert!(groups_on(&spans, 1, src).iter().any(|(_, g)| *g == Group::Comment));
}

#[test]
fn incremental_parse_equals_full_parse_for_typical_edits() {
    let base = "fn main() {\n    let x = 1;\n    println!(\"hi ğ\");\n}\n";
    let (state, _) = highlight(Lang::Rust, None, Arc::new(base.to_owned())).unwrap();
    let edits = [
        base.replacen("1", "12345", 1),                      // insert
        base.replacen("    let x = 1;\n", "", 1),            // delete a line
        base.replacen("main", "mäin", 1),                    // multibyte insert
        base.replacen("\"hi ğ\"", "\"hi", 1),                // break a string
        format!("{base}\nstruct S;\n"),                      // append
        format!("// header\n{base}"),                        // prepend
        base.to_owned(),                                     // no change
    ];
    for new in edits {
        let new = Arc::new(new);
        let (_, inc) = highlight(Lang::Rust, Some(&state), new.clone()).unwrap();
        let (_, full) = highlight(Lang::Rust, None, new.clone()).unwrap();
        assert_eq!(inc, full, "incremental != full for {new:?}");
    }
}

#[test]
fn diff_edit_is_minimal_and_positions_are_right() {
    let e = engine::diff_edit("ab\ncd\nef", "ab\ncXd\nef");
    assert_eq!((e.start_byte, e.old_end_byte, e.new_end_byte), (4, 4, 5));
    assert_eq!((e.start_position.row, e.start_position.column), (1, 1));
    assert_eq!((e.new_end_position.row, e.new_end_position.column), (1, 2));
    let e = engine::diff_edit("same", "same");
    assert_eq!((e.start_byte, e.old_end_byte, e.new_end_byte), (4, 4, 4));
}

#[test]
fn every_bundled_grammar_compiles_its_query_and_highlights() {
    let sample = |lang: Lang| -> &'static str {
        match lang {
            Lang::Rust => "fn main() { let a = 1; }",
            Lang::Python => "def f(x):\n    return 'a' + str(1)  # c\n",
            Lang::JavaScript => "function f() { return \"a\" + 1; } // c",
            Lang::TypeScript => "let a: number = 1; // c",
            Lang::Tsx => "const a = <div className=\"x\">hi</div>;",
            Lang::Json => "{\"a\": [1, true, null, \"s\"]}",
            Lang::Toml => "[package]\nname = \"x\"\nn = 1 # c\n",
            Lang::Bash => "echo \"hi\" # c\nif true; then ls; fi",
            Lang::C => "int main(void) { return 0; } // c",
            Lang::Java => "public class A { void f() { String s = \"x\"; } } // c",
            Lang::Php => "<?php\nfunction f($a) { return \"x\" . $a; } // c\n",
            Lang::Ruby => "def f(a)\n  puts \"x\" # c\nend\n",
            Lang::Lua => "local function f() return \"x\" end -- c",
            Lang::Html => "<p class=\"x\">hi</p><!-- c -->",
            Lang::Twig => "<p>{{ a|upper }}</p>{# c #}",
            Lang::Css => "a { color: red; } /* c */",
            Lang::Scss => "$c: red; a { color: $c; } // c",
            Lang::Xml => "<a b=\"c\">text</a><!-- c -->",
            Lang::Yaml => "a: 1\nb: \"s\" # c\n",
            Lang::Go => "package main\nfunc main() { x := 1 } // c",
            Lang::Markdown => "# Title\n\n```rust\nlet a = 1;\n```\n\n- item\n",
            Lang::MarkdownInline => "some `code` and *emphasis* and [link](http://x)",
            Lang::Make => "all: main.o\n\t$(CC) -o app main.o # c\n",
            Lang::CMake => "cmake_minimum_required(VERSION 3.10) # c\nproject(x)\n",
            Lang::Ini => "[section]\nkey = value ; c\n",
            Lang::Diff => "--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new\n",
            Lang::Lex(_) | Lang::Apache => unreachable!(),
        }
    };
    for lang in Lang::GRAMMARS {
        let spans = spans_of(lang, sample(lang));
        let n: usize = (0..spans.line_count()).map(|i| spans.line(i).len()).sum();
        assert!(n > 0, "{} produced no highlights", lang.name());
    }
}

#[test]
fn every_lexer_language_highlights_its_sample() {
    for (i, spec) in lexer::SPECS.iter().enumerate() {
        let kw = spec.keywords.first().expect("has keywords");
        let comment = spec.line_comments.first().copied().unwrap_or("");
        let src = format!("{kw} value 12 \"str\" {comment} note\n");
        let spans = spans_of(Lang::Lex(i as u8), &src);
        let found: Vec<Group> = spans.line(0).iter().map(|s| s.group).collect();
        assert!(found.contains(&Group::Keyword) || found.contains(&Group::Constant), "{}: {found:?}", spec.name);
        assert!(found.contains(&Group::Number), "{}: {found:?}", spec.name);
        if !comment.is_empty() {
            assert!(found.contains(&Group::Comment), "{}: {found:?}", spec.name);
        }
    }
}

#[test]
fn lexer_handles_blocks_strings_and_utf8() {
    let kotlin = Lang::detect(Path::new("A.kt")).unwrap();
    let src = "/* ğ\n ü */ val s = \"a\\\"b\" // ç\nval t = \"\"\"x\ny\"\"\"\nfun f() = 1\n";
    let spans = spans_of(kotlin, src);
    let l0 = groups_on(&spans, 0, src);
    assert!(l0.iter().any(|(t, g)| *g == Group::Comment && t.starts_with("/*")), "{l0:?}");
    let l1 = groups_on(&spans, 1, src);
    assert!(l1.iter().any(|(t, g)| *g == Group::Comment && t.ends_with("ü */")), "{l1:?}");
    assert!(l1.contains(&("val".into(), Group::Keyword)), "{l1:?}");
    assert!(l1.iter().any(|(t, g)| *g == Group::String && t == "\"a\\\"b\""), "{l1:?}");
    assert!(l1.iter().any(|(t, g)| *g == Group::Comment && t == "// ç"), "{l1:?}");
    // Triple-quoted strings span lines.
    let l2 = groups_on(&spans, 2, src);
    let l3 = groups_on(&spans, 3, src);
    assert!(l2.iter().any(|(_, g)| *g == Group::String), "{l2:?}");
    assert!(l3.iter().any(|(t, g)| *g == Group::String && t.starts_with('y')), "{l3:?}");
    let l4 = groups_on(&spans, 4, src);
    assert!(l4.contains(&("fun".into(), Group::Keyword)) && l4.contains(&("f".into(), Group::Function)), "{l4:?}");
}

#[test]
fn sql_keywords_match_in_any_case() {
    let src = "SELECT name, COUNT(*) FROM users where id = 12 -- note\nselect 'x';\n";
    let spans = spans_of(Lang::detect(Path::new("q.sql")).unwrap(), src);
    let l0 = groups_on(&spans, 0, src);
    for kw in ["SELECT", "FROM", "where"] {
        assert!(l0.contains(&(kw.into(), Group::Keyword)), "{kw}: {l0:?}");
    }
    assert!(l0.contains(&("COUNT".into(), Group::Function)), "{l0:?}");
    assert!(l0.contains(&("12".into(), Group::Number)) && l0.iter().any(|(_, g)| *g == Group::Comment), "{l0:?}");
    assert!(groups_on(&spans, 1, src).contains(&("select".into(), Group::Keyword)));
    // C++ directives.
    let cpp = Lang::detect(Path::new("a.cpp")).unwrap();
    let src = "#include <vector>\nclass A : public B { int x = 1; };\n";
    let spans = spans_of(cpp, src);
    assert!(groups_on(&spans, 0, src).contains(&("#include".into(), Group::Keyword)));
    assert!(groups_on(&spans, 1, src).contains(&("class".into(), Group::Keyword)));
}

#[test]
fn twig_tags_are_colored_over_the_html() {
    let lang = Lang::detect(Path::new("templates/base.html.twig")).unwrap();
    assert_eq!(lang, Lang::Twig);
    for ext in ["a.j2", "a.jinja", "a.njk", "a.liquid"] {
        assert_eq!(Lang::detect(Path::new(ext)), Some(Lang::Twig), "{ext}");
    }
    let src = "<p class=\"{{ cls }}\">{{ user.name|upper }} {% for u in users if u.id > 10 %}{{- u.id + 1 -}}{% endfor %}</p>\n{# a {{ note }} #}\n{{ \"}}\" ~ x }} {% if x %}\n";
    let spans = spans_of(lang, src);
    let l0 = groups_on(&spans, 0, src);
    assert!(l0.contains(&("p".into(), Group::Tag)), "html still colored: {l0:?}");
    assert!(l0.contains(&("{{".into(), Group::Operator)) && l0.contains(&("{%".into(), Group::Operator)), "{l0:?}");
    assert!(l0.contains(&("for".into(), Group::Keyword)) && l0.contains(&("in".into(), Group::Keyword)), "{l0:?}");
    assert!(l0.contains(&("name".into(), Group::Property)), "{l0:?}");
    assert!(l0.contains(&("upper".into(), Group::Function)), "{l0:?}");
    assert!(l0.contains(&("10".into(), Group::Number)) && l0.contains(&("1".into(), Group::Number)), "{l0:?}");
    assert!(l0.iter().any(|(t, g)| *g == Group::Operator && t.contains("{{-")), "trim marker: {l0:?}");
    assert!(l0.iter().any(|(t, g)| *g == Group::Operator && t.contains("-}}")), "trim marker: {l0:?}");
    // The attribute value is HTML's string, but the tag inside it is a tag.
    assert!(l0.iter().any(|(t, g)| *g == Group::Operator && t == "{{"), "{l0:?}");
    // A comment swallows tags inside it.
    let l1 = groups_on(&spans, 1, src);
    assert_eq!(l1, vec![("{# a {{ note }} #}".to_owned(), Group::Comment)], "{l1:?}");
    // `}}` inside a string does not end the tag.
    let l2 = groups_on(&spans, 2, src);
    assert!(l2.iter().any(|(t, g)| *g == Group::String && t == "\"}}\""), "{l2:?}");
    assert!(l2.contains(&("if".into(), Group::Keyword)), "{l2:?}");

    // An unclosed `{{` must not color the rest of the file.
    let src = "<p>{{ oops</p>\n<b>fine</b>\n";
    let spans = spans_of(lang, src);
    assert!(groups_on(&spans, 1, src).contains(&("b".into(), Group::Tag)));
    assert!(!groups_on(&spans, 0, src).iter().any(|(_, g)| *g == Group::Operator));
}

#[test]
fn php_files_color_php_and_the_html_around_it() {
    let src = "<?php\n$a = \"x\"; // note\nfunction f() { return 1; }\n?>\n<div class=\"box\">hi</div>\n<style>a { color: red; }</style>\n<script>var n = 12;</script>\n";
    let spans = spans_of(Lang::Php, src);
    let l1 = groups_on(&spans, 1, src);
    assert!(l1.iter().any(|(t, g)| *g == Group::String && t.contains('x')), "php string: {l1:?}");
    assert!(l1.iter().any(|(t, g)| *g == Group::Comment && t.contains("note")), "php comment: {l1:?}");
    let l2 = groups_on(&spans, 2, src);
    assert!(l2.iter().any(|(t, g)| *g == Group::Keyword && t == "function"), "php keyword: {l2:?}");
    let html = groups_on(&spans, 4, src);
    assert!(html.iter().any(|(_, g)| *g == Group::Tag), "html tag: {html:?}");
    assert!(html.iter().any(|(t, g)| *g == Group::Attribute && t == "class"), "html attribute: {html:?}");
    let css = groups_on(&spans, 5, src);
    assert!(css.iter().any(|(t, _)| t == "color"), "css inside <style>: {css:?}");
    let js = groups_on(&spans, 6, src);
    assert!(js.iter().any(|(t, g)| *g == Group::Number && t == "12"), "js inside <script>: {js:?}");
}

#[test]
fn markdown_colors_blocks_and_inline_code() {
    let src = "# Title\n\nsome `code` here\n";
    let spans = spans_of(Lang::Markdown, src);
    let l0 = groups_on(&spans, 0, src);
    assert!(!l0.is_empty(), "heading must be colored: {l0:?}");
    let l2 = groups_on(&spans, 2, src);
    assert!(l2.iter().any(|(t, _)| t.contains("code")), "inline code: {l2:?}");
}

#[test]
fn apache_config_is_painted_by_directive_not_as_an_ini_file() {
    // The user's file: the sections and `Options -MultiViews` used to be painted as INI errors.
    let src = "<IfModule mod_negotiation.c>\n    Options -MultiViews\n</IfModule>\n# comment\nRedirectMatch 301 ^/en/ https://www.acmagile.com/\nRewriteCond %{HTTP_HOST} ^old\\.acmagile\\.com$ [NC]\nRewriteRule ^(.*) https://www.acmagile.com/$1 [L,R=301]\nHeader set X-Frame-Options \"DENY\"\nRewriteRule a b \\\n  [L]\n";
    let spans = spans_of(Lang::Apache, src);
    let line = |n: usize| groups_on(&spans, n, src);
    assert_eq!(
        line(0),
        vec![("<".into(), Group::Punctuation), ("IfModule".into(), Group::Tag), ("mod_negotiation.c".into(), Group::Type), (">".into(), Group::Punctuation)]
    );
    assert_eq!(
        line(1),
        vec![("Options".into(), Group::Keyword), ("-".into(), Group::Operator), ("MultiViews".into(), Group::Constant)]
    );
    assert_eq!(line(2), vec![("</".into(), Group::Punctuation), ("IfModule".into(), Group::Tag), (">".into(), Group::Punctuation)]);
    assert_eq!(line(3), vec![("# comment".into(), Group::Comment)]);
    // Every `RedirectMatch` line is built the same way, whatever the pattern looks like.
    assert_eq!(
        line(4),
        vec![
            ("RedirectMatch".into(), Group::Keyword),
            ("301".into(), Group::Number),
            ("^/en/".into(), Group::String),
            ("https://www.acmagile.com/".into(), Group::Attribute),
        ]
    );
    let l5 = line(5);
    assert!(l5.contains(&("%{HTTP_HOST}".into(), Group::Property)) && l5.contains(&("NC".into(), Group::Type)), "{l5:?}");
    let l6 = line(6);
    assert!(l6.contains(&("$1".into(), Group::Property)) && l6.contains(&("301".into(), Group::Number)), "{l6:?}");
    let l7 = line(7);
    assert!(l7.contains(&("set".into(), Group::Constant)) && l7.contains(&("\"DENY\"".into(), Group::String)), "{l7:?}");
    // A continued line has no directive name of its own.
    let l9 = line(9);
    assert!(l9.iter().all(|(_, g)| *g != Group::Keyword), "{l9:?}");
}

#[test]
fn detection_knows_copies_config_files_and_what_files_say_about_themselves() {
    let d = |p: &str| Lang::detect(Path::new(p));
    assert_eq!(d("/var/www/agile.loc/public/.htaccess"), Some(Lang::Apache));
    assert_eq!(d("sitemap.xml.old"), Some(Lang::Xml), "a copy is what it is a copy of");
    assert_eq!(d("phpunit.xml.dist"), Some(Lang::Xml));
    assert_eq!(d("web.config"), Some(Lang::Xml));
    assert_eq!(d("translations/messages.tr.xlf"), Some(Lang::Xml));
    assert_eq!(d(".htaccess.bak"), Some(Lang::Apache));
    assert_eq!(d("index.php~"), Some(Lang::Php));
    assert_eq!(d(".env.dist"), Some(Lang::Ini));
    assert_eq!(d("notes.old"), None);
    assert_eq!(d(".old"), None);
    // Names that say nothing: the first bytes do.
    let w = |p: &str, head: &str| Lang::detect_with(Path::new(p), head);
    assert_eq!(w("data", "<?xml version=\"1.0\"?><a/>"), Some(Lang::Xml));
    assert_eq!(w("deploy", "#!/usr/bin/env python3\nprint(1)"), Some(Lang::Python));
    assert_eq!(w("run", "#!/bin/bash\nls"), Some(Lang::Bash));
    assert_eq!(w("README", "just words"), None);
    assert_eq!(w("a.txt", "<?xml version=\"1.0\"?>"), None, "a known-unknown name is left alone");
    // `.conf`: Apache or nginx, by content; anything else stays INI-style.
    assert_eq!(w("site.conf", "<VirtualHost *:80>\n  ServerName a.loc\n  DocumentRoot /var/www\n</VirtualHost>\n"), Some(Lang::Apache));
    assert_eq!(w("site.conf", "server {\n  listen 80;\n  location / { proxy_pass http://x; }\n}\n").map(Lang::name), Some("Nginx"));
    assert_eq!(w("my.conf", "[main]\nkey = value\n"), Some(Lang::Ini));
}

#[test]
fn a_text_without_a_name_is_recognized_by_what_it_starts_with() {
    use super::detect_content as d;
    // A page copied from a browser: doctype, or a comment first, or no doctype at all.
    assert_eq!(d("<!DOCTYPE html>\n<html lang=\"tr\">\n<head>"), Some(Lang::Html));
    assert_eq!(d("\n\n  <!doctype HTML PUBLIC \"-//W3C//DTD HTML 4.01//EN\">"), Some(Lang::Html));
    assert_eq!(d("<html>\n<body>"), Some(Lang::Html));
    assert_eq!(d("<head>\n<title>x</title>"), Some(Lang::Html));
    assert_eq!(d("<!-- saved from url=(0022)https://example.com/ -->\n<!DOCTYPE html>\n<html>"), Some(Lang::Html));
    assert_eq!(d("<!-- a -->\n<!-- b -->\n<html>"), Some(Lang::Html));
    assert_eq!(d("<!-- never closed\n<html>"), None, "an unterminated comment says nothing");
    // Other documents.
    assert_eq!(d("<?xml version=\"1.0\"?>\n<a/>"), Some(Lang::Xml));
    assert_eq!(d("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\"/>"), Some(Lang::Xml));
    assert_eq!(d("<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x\">"), Some(Lang::Xml));
    assert_eq!(d("<?php\necho 1;"), Some(Lang::Php));
    assert_eq!(d("#!/usr/bin/env python3\nprint(1)"), Some(Lang::Python));
    // Ordinary words and markup that is not a page say nothing.
    assert_eq!(d("just some words"), None);
    assert_eq!(d(""), None);
    assert_eq!(d("<div>a fragment</div>"), None);
    assert_eq!(d("a < b and c > d"), None);
}

#[test]
fn looking_at_the_start_of_a_text_never_cuts_a_character_in_two() {
    // Every offset: the 64-byte window must land on a character boundary whatever precedes it.
    let word = "şğüıöç İĞÜŞÖÇ 日本語のテキスト ";
    for pad in 0..16 {
        let text = format!("{}{}", "a".repeat(pad), word.repeat(12));
        assert_eq!(super::detect_content(&text), None);
        assert_eq!(Lang::detect_with(Path::new("README"), &text), None);
    }
    let page = format!("<!DOCTYPE html>{}", word.repeat(30));
    assert_eq!(super::detect_content(&page), Some(Lang::Html));
}

#[test]
fn the_syntax_list_has_every_language_once_and_starts_with_the_two_special_choices() {
    let all = super::Syntax::all();
    assert_eq!(&all[..2], &[super::Syntax::Auto, super::Syntax::Plain]);
    let langs = Lang::pickable();
    assert!(!langs.contains(&Lang::MarkdownInline), "inline Markdown is not a language of its own");
    assert!(langs.contains(&Lang::Html) && langs.contains(&Lang::Apache), "grammars and painters");
    assert!(langs.len() >= Lang::GRAMMARS.len() - 1 + lexer::SPECS.len() - 1, "{}", langs.len());
    // The table-driven languages are in it too, by name, in alphabetical order, none twice.
    assert!(langs.iter().any(|l| l.name() == "Kotlin"));
    let names: Vec<String> = langs.iter().map(|l| l.name().to_lowercase()).collect();
    assert!(names.windows(2).all(|w| w[0] <= w[1]), "sorted: {names:?}");
    let mut unique = langs.clone();
    unique.dedup();
    assert_eq!(unique.len(), langs.len());
    assert_eq!(all.len(), langs.len() + 2);
}
