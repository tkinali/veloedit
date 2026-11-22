//! Syntax highlighting with tree-sitter.
//!
//! Pipeline (all heavy work off the UI thread):
//!   text --(incremental parse, reusing the previous tree)--> tree
//!        --(grammar's highlights.scm captures, painted per byte)--> `LineSpans`
//!        --(Arc held by the document)--> `ui::editor_view` (draws colored runs)
//!
//! `Group` is the theme-facing vocabulary; `ui::theme` maps it to colors.

mod engine;
pub mod lexer;

use std::path::Path;

pub use engine::{LineSpans, ParseState, Span, highlight};

/// Files above this size are not highlighted (keeps the worker snappy).
pub const MAX_BYTES: usize = 2 * 1024 * 1024;

/// Semantic highlight class. Themes map each class to a color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Group {
    Keyword,
    Function,
    Type,
    String,
    Comment,
    Number,
    Constant,
    Operator,
    Property,
    Punctuation,
    Attribute,
    Tag,
    Escape,
}

impl Group {
    pub const COUNT: usize = 13;

    /// Maps a tree-sitter capture name (`function.builtin`, `string.escape`, ...)
    /// to a class. Unknown / uninteresting names (plain variables, ...) are not
    /// highlighted at all.
    pub fn from_capture(name: &str) -> Option<Group> {
        let is = |p: &str| name == p || name.strip_prefix(p).is_some_and(|r| r.starts_with('.'));
        // Order matters: most specific first.
        Some(if is("string.escape") || is("escape") {
            Group::Escape
        } else if is("tag.attribute") || is("attribute") || is("annotation") || is("decorator") {
            Group::Attribute
        } else if is("tag") || is("diff.minus") {
            Group::Tag
        } else if is("keyword.operator") || is("operator") {
            Group::Operator
        } else if is("keyword")
            || is("conditional")
            || is("repeat")
            || is("include")
            || is("exception")
            || is("storage")
            || is("define")
        {
            Group::Keyword
        } else if is("function")
            || is("method")
            || is("text.title")
            || is("markup.heading")
            || is("diff.delta")
            || is("title")
        {
            Group::Function
        } else if is("type") || is("constructor") || is("class") || is("module") || is("namespace") || is("label") {
            Group::Type
        } else if is("string")
            || is("character")
            || is("text.literal")
            || is("markup.raw")
            || is("diff.plus")
        {
            Group::String
        } else if is("comment") {
            Group::Comment
        } else if is("number") || is("float") {
            Group::Number
        } else if is("constant")
            || is("boolean")
            || is("text.emphasis")
            || is("text.strong")
            || is("markup.italic")
            || is("markup.bold")
        {
            Group::Constant
        } else if is("property") || is("field") || is("variable.member") {
            Group::Property
        } else if is("text.uri") || is("markup.link.url") || is("text.reference") || is("markup.link") {
            Group::Attribute
        } else if is("punctuation") || is("delimiter") {
            Group::Punctuation
        } else {
            return None;
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Json,
    Toml,
    Bash,
    C,
    Java,
    Php,
    Ruby,
    Lua,
    Html,
    /// HTML with Twig / Jinja / Nunjucks / Liquid tags (`{{ }}`, `{% %}`, `{# #}`).
    Twig,
    Css,
    Scss,
    Xml,
    Yaml,
    Go,
    Markdown,
    /// Inline part of Markdown; only ever injected into `Markdown`, never detected.
    MarkdownInline,
    Make,
    CMake,
    Ini,
    Diff,
    /// Apache configuration (`.htaccess`, `httpd.conf`, virtual hosts): painted by `lexer::paint_apache`.
    Apache,
    /// No grammar: the table-driven lexer (`lexer::SPECS[i]`).
    Lex(u8),
}

impl Lang {
    /// Every language backed by a grammar (the lexer ones are listed by `lexer::SPECS`).
    #[cfg(test)]
    pub const GRAMMARS: [Lang; 26] = [
        Lang::Rust, Lang::Python, Lang::JavaScript, Lang::TypeScript, Lang::Tsx, Lang::Json,
        Lang::Toml, Lang::Bash, Lang::C, Lang::Java, Lang::Php,
        Lang::Ruby, Lang::Lua, Lang::Html, Lang::Twig, Lang::Css, Lang::Scss, Lang::Xml,
        Lang::Yaml, Lang::Go, Lang::Markdown, Lang::MarkdownInline, Lang::Make, Lang::CMake,
        Lang::Ini, Lang::Diff,
    ];

    /// Language of a file by name/extension.
    pub fn detect(path: &Path) -> Option<Lang> {
        let name = path.file_name()?.to_str()?;
        match name {
            "Cargo.lock" | "Pipfile" | "poetry.lock" => return Some(Lang::Toml),
            ".htaccess" | "httpd.conf" | "apache2.conf" | "apache.conf" | "httpd-vhosts.conf" => return Some(Lang::Apache),
            "composer.lock" | ".babelrc" | ".eslintrc" | ".prettierrc" | ".stylelintrc" => return Some(Lang::Json),
            ".bashrc" | ".bash_profile" | ".bash_aliases" | ".bash_logout" | ".profile" | ".zshrc" | ".zprofile"
            | ".zshenv" | "PKGBUILD" => return Some(Lang::Bash),
            "Makefile" | "makefile" | "GNUmakefile" => return Some(Lang::Make),
            "CMakeLists.txt" => return Some(Lang::CMake),
            "Gemfile" | "Rakefile" | "Vagrantfile" | "Podfile" | "Brewfile" | "Guardfile" | "Capfile" => {
                return Some(Lang::Ruby);
            }
            ".gitignore" | ".gitattributes" | ".gitmodules" | ".dockerignore" | ".editorconfig" | ".npmrc"
            | ".gitconfig" => return Some(Lang::Ini),
            _ => {}
        }
        if name.starts_with(".env") {
            return Some(Lang::Ini);
        }
        // `sitemap.xml.old`, `phpunit.xml.dist`, `.htaccess.bak`: the language of what it is a copy of.
        if let Some(stem) = strip_variant(name) {
            return Lang::detect(&path.with_file_name(stem));
        }
        let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        if let Some(i) = lexer::lookup(ext.as_deref(), name) {
            return Some(Lang::Lex(i));
        }
        Some(match ext?.as_str() {
            "rs" => Lang::Rust,
            "py" | "pyi" | "pyw" | "pyx" => Lang::Python,
            "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "json" | "jsonc" | "geojson" | "webmanifest" | "har" => Lang::Json,
            "toml" => Lang::Toml,
            "sh" | "bash" | "zsh" | "ksh" | "fish" => Lang::Bash,
            "c" | "h" => Lang::C,
            "java" => Lang::Java,
            "php" | "phtml" | "php3" | "php4" | "php5" | "php7" | "php8" | "phps" | "inc" | "module" => Lang::Php,
            "rb" | "rake" | "gemspec" | "ru" | "rbw" => Lang::Ruby,
            "lua" => Lang::Lua,
            // Template languages are mostly markup: HTML colors the markup and is
            // right far more often than plain text.
            "html" | "htm" | "xhtml" | "vue" | "svelte" | "tpl" | "erb" | "ejs" | "hbs" | "mustache" | "astro" => {
                Lang::Html
            }
            "twig" | "j2" | "jinja" | "jinja2" | "njk" | "nunjucks" | "liquid" => Lang::Twig,
            "css" => Lang::Css,
            "scss" | "sass" => Lang::Scss,
            "xml" | "svg" | "xsd" | "xsl" | "xslt" | "plist" | "csproj" | "vbproj" | "fsproj" | "vcxproj" | "props"
            | "targets" | "xaml" | "wsdl" | "rss" | "atom" | "kml" | "gpx" | "resx" | "storyboard" | "xlf"
            | "xliff" | "dtd" | "nuspec" | "opml" | "tmx" | "xib" | "fxml" | "mxml" | "rng" | "xmp" | "jrxml"
            // .NET: web.config, app.config, nuget.config ...
            | "config" => Lang::Xml,
            "htaccess" | "vhost" | "apache" => Lang::Apache,
            "yml" | "yaml" => Lang::Yaml,
            "go" => Lang::Go,
            "md" | "markdown" | "mdown" | "mkd" | "mdx" => Lang::Markdown,
            "mk" | "mak" => Lang::Make,
            "cmake" => Lang::CMake,
            "ini" | "cfg" | "conf" | "properties" | "env" | "desktop" | "service" | "socket" | "timer"
            | "editorconfig" | "gitconfig" | "lock" => Lang::Ini,
            "diff" | "patch" => Lang::Diff,
            _ => return None,
        })
    }

    /// Like [`detect`](Lang::detect), but may also look at the first bytes of the file: a
    /// `.conf` is Apache or nginx depending on what is in it, and a file with no known name
    /// that starts with `<?xml` or a `#!` line is what that says.
    pub fn detect_with(path: &Path, head: &str) -> Option<Lang> {
        match Lang::detect(path) {
            Some(Lang::Ini) if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("conf")) => {
                Some(sniff_conf(head).unwrap_or(Lang::Ini))
            }
            Some(lang) => Some(lang),
            // Plain-text names stay plain; anything else with no known language may say what it is.
            None if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "txt" | "text" | "log" | "csv" | "tsv" | "rst" | "adoc" | "rtf")) =>
            {
                None
            }
            None => sniff(head),
        }
    }

    /// [`detect_with`](Lang::detect_with) for a whole text (only its beginning is looked at).
    pub fn detect_text(path: &Path, text: &str) -> Option<Lang> {
        let mut n = text.len().min(8192);
        while !text.is_char_boundary(n) {
            n -= 1;
        }
        Lang::detect_with(path, &text[..n])
    }

    pub fn name(self) -> &'static str {
        match self {
            Lang::Rust => "Rust",
            Lang::Python => "Python",
            Lang::JavaScript => "JavaScript",
            Lang::TypeScript => "TypeScript",
            Lang::Tsx => "TSX",
            Lang::Json => "JSON",
            Lang::Toml => "TOML",
            Lang::Bash => "Shell",
            Lang::C => "C",
            Lang::Java => "Java",
            Lang::Php => "PHP",
            Lang::Ruby => "Ruby",
            Lang::Lua => "Lua",
            Lang::Html => "HTML",
            Lang::Twig => "Twig / Jinja",
            Lang::Css => "CSS",
            Lang::Scss => "SCSS",
            Lang::Xml => "XML",
            Lang::Yaml => "YAML",
            Lang::Go => "Go",
            Lang::Markdown | Lang::MarkdownInline => "Markdown",
            Lang::Make => "Makefile",
            Lang::CMake => "CMake",
            Lang::Ini => "INI / Config",
            Lang::Diff => "Diff",
            Lang::Apache => "Apache Config",
            Lang::Lex(i) => lexer::spec(i).map_or("Plain Text", |s| s.name),
        }
    }
}

impl Lang {
    /// Every language a buffer can be set to by hand, by name (inline Markdown is only ever
    /// part of Markdown).
    pub fn pickable() -> Vec<Lang> {
        let mut all = vec![
            Lang::Rust, Lang::Python, Lang::JavaScript, Lang::TypeScript, Lang::Tsx, Lang::Json, Lang::Toml,
            Lang::Bash, Lang::C, Lang::Java, Lang::Php, Lang::Ruby, Lang::Lua, Lang::Html, Lang::Twig, Lang::Css,
            Lang::Scss, Lang::Xml, Lang::Yaml, Lang::Go, Lang::Markdown, Lang::Make, Lang::CMake, Lang::Ini,
            Lang::Diff, Lang::Apache,
        ];
        all.extend((0..lexer::SPECS.len()).map(|i| Lang::Lex(i as u8)));
        all.sort_by_key(|l| l.name().to_lowercase());
        all
    }
}

/// What the user picks for a buffer's highlighting ("View > Syntax").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// Go by the file name (and, for a text that was never saved, by what it starts with).
    Auto,
    /// No highlighting, whatever the name or the content says.
    Plain,
    Lang(Lang),
}

impl Syntax {
    pub fn title(self) -> &'static str {
        match self {
            Syntax::Auto => "Auto Detect",
            Syntax::Plain => "Plain Text",
            Syntax::Lang(l) => l.name(),
        }
    }

    /// Every choice, in list order: the two special ones, then the languages by name.
    pub fn all() -> Vec<Syntax> {
        [Syntax::Auto, Syntax::Plain].into_iter().chain(Lang::pickable().into_iter().map(Syntax::Lang)).collect()
    }
}

/// Suffixes of a copy / template of another file (`x.xml.dist`, `index.php.bak`).
const VARIANT_SUFFIXES: [&str; 12] =
    [".old", ".bak", ".orig", ".dist", ".tmp", ".sample", ".example", ".template", ".default", ".backup", ".save", ".new"];

/// `name` without a copy suffix (`None` when it has none, or nothing would be left).
fn strip_variant(name: &str) -> Option<&str> {
    let lower = name.to_ascii_lowercase();
    VARIANT_SUFFIXES
        .iter()
        .find(|s| lower.ends_with(*s) && name.len() > s.len())
        .map(|s| &name[..name.len() - s.len()])
        .or_else(|| name.strip_suffix('~').filter(|n| !n.is_empty()))
}

/// Apache or nginx? Looks for what only one of them writes in the first few KiB.
fn sniff_conf(head: &str) -> Option<Lang> {
    let head = &head[..head.len().min(8192)];
    let apache = ["<VirtualHost", "<Directory", "<IfModule", "<Location", "<Files", "DocumentRoot", "RewriteEngine", "LoadModule", "ServerName "];
    let nginx = ["server {", "server{", "location /", "location ~", "proxy_pass", "fastcgi_pass", "upstream "];
    let count = |words: &[&str]| words.iter().filter(|w| head.contains(**w)).count();
    let (a, n) = (count(&apache), count(&nginx));
    if a > n {
        Some(Lang::Apache)
    } else if n > a {
        lexer::lookup(None, "nginx.conf").map(Lang::Lex)
    } else {
        None
    }
}

/// `s` cut to at most `n` bytes, never in the middle of a character.
fn prefix(s: &str, n: usize) -> &str {
    let mut n = n.min(s.len());
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    &s[..n]
}

/// `s` without the HTML comments it starts with (a page saved from a browser begins with a
/// `<!-- saved from url=... -->` line). An unterminated comment is left alone.
fn skip_comments(mut s: &str) -> &str {
    for _ in 0..8 {
        let Some(rest) = s.strip_prefix("<!--") else { break };
        let Some(end) = rest.find("-->") else { break };
        s = rest[end + 3..].trim_start();
    }
    s
}

/// The language of a text that has no file name (a new tab the user pasted into): what its
/// first bytes say it is, or `None`.
pub fn detect_content(head: &str) -> Option<Lang> {
    sniff(head)
}

/// The language a file with no recognizable name says it is in.
fn sniff(head: &str) -> Option<Lang> {
    let head = head.trim_start_matches('\u{feff}').trim_start();
    if head.starts_with("<?xml") {
        return Some(Lang::Xml);
    }
    if head.starts_with("<?php") {
        return Some(Lang::Php);
    }
    let lower = prefix(skip_comments(head), 64).to_ascii_lowercase();
    if lower.starts_with("<!doctype html") || lower.starts_with("<html") || lower.starts_with("<head") || lower.starts_with("<body") {
        return Some(Lang::Html);
    }
    // Any other document type (SVG, DocBook, ...) is XML.
    if lower.starts_with("<!doctype") || lower.starts_with("<svg") {
        return Some(Lang::Xml);
    }
    let first = head.lines().next()?;
    let shebang = first.strip_prefix("#!")?;
    let word = |w: &str| shebang.split(|c: char| !c.is_ascii_alphanumeric()).any(|t| t == w || (t.starts_with(w) && t[w.len()..].chars().all(|c| c.is_ascii_digit())));
    Some(if word("python") {
        Lang::Python
    } else if word("node") || word("deno") || word("bun") {
        Lang::JavaScript
    } else if word("ruby") {
        Lang::Ruby
    } else if word("php") {
        Lang::Php
    } else if word("lua") {
        Lang::Lua
    } else if word("sh") || word("bash") || word("zsh") || word("ksh") || word("dash") {
        Lang::Bash
    } else if word("perl") {
        return lexer::lookup(Some("pl"), "").map(Lang::Lex);
    } else {
        return None;
    })
}

#[cfg(test)]
mod tests;
