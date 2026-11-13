//! Fallback highlighter for languages without a bundled tree-sitter grammar.
//!
//! A tiny hand-written lexer: comments, strings, numbers, keywords, types
//! (`Capitalized`), constants (`ALL_CAPS`) and calls (`name(`). It is driven by
//! a per-language table, so supporting one more language is one table row and
//! costs no binary size (unlike a grammar).

use super::Group;

pub struct Spec {
    pub name: &'static str,
    /// File extensions (lowercase, no dot).
    pub exts: &'static [&'static str],
    /// Exact file names.
    pub files: &'static [&'static str],
    pub line_comments: &'static [&'static str],
    pub block_comment: Option<(&'static str, &'static str)>,
    /// Characters that open a string (a matching one closes it).
    pub quotes: &'static str,
    /// `"""` / `'''` strings may span lines.
    pub triple_quotes: bool,
    pub keywords: &'static [&'static str],
    /// Keywords match in any case (SQL).
    pub ignore_case: bool,
}

const NONE: Option<(&str, &str)> = None;
const C_LINE: &[&str] = &["//"];
const C_BLOCK: Option<(&str, &str)> = Some(("/*", "*/"));
const HASH: &[&str] = &["#"];

pub static SPECS: &[Spec] = &[
    Spec {
        name: "Kotlin",
        exts: &["kt", "kts"],
        files: &[],
        line_comments: C_LINE,
        block_comment: C_BLOCK,
        quotes: "\"'",
        triple_quotes: true,
        keywords: &[
            "as", "break", "class", "continue", "do", "else", "for", "fun", "if", "in", "interface", "is",
            "object", "package", "return", "super", "this", "throw", "try", "typealias", "val", "var",
            "when", "while", "import", "override", "open", "abstract", "final", "private", "protected",
            "public", "internal", "data", "sealed", "enum", "companion", "suspend", "inline", "lateinit",
            "const", "by", "init", "constructor", "get", "set", "where", "out", "vararg", "operator",
            "true", "false", "null",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Swift",
        exts: &["swift"],
        files: &[],
        line_comments: C_LINE,
        block_comment: C_BLOCK,
        quotes: "\"",
        triple_quotes: true,
        keywords: &[
            "associatedtype", "class", "deinit", "enum", "extension", "fileprivate", "func", "import", "init",
            "inout", "internal", "let", "open", "operator", "private", "protocol", "public", "static",
            "struct", "subscript", "typealias", "var", "break", "case", "continue", "default", "defer",
            "do", "else", "fallthrough", "for", "guard", "if", "in", "repeat", "return", "switch", "where",
            "while", "as", "catch", "is", "nil", "rethrows", "super", "self", "throw", "throws", "try",
            "async", "await", "actor", "some", "any", "true", "false", "final", "override", "lazy",
            "mutating", "weak", "unowned",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Dart",
        exts: &["dart"],
        files: &[],
        line_comments: C_LINE,
        block_comment: C_BLOCK,
        quotes: "\"'",
        triple_quotes: true,
        keywords: &[
            "abstract", "as", "assert", "async", "await", "break", "case", "catch", "class", "const",
            "continue", "covariant", "default", "deferred", "do", "dynamic", "else", "enum", "export",
            "extends", "extension", "external", "factory", "false", "final", "finally", "for", "get",
            "if", "implements", "import", "in", "interface", "is", "late", "library", "mixin", "new",
            "null", "on", "operator", "part", "required", "rethrow", "return", "set", "static", "super",
            "switch", "this", "throw", "true", "try", "typedef", "var", "void", "while", "with", "yield",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Scala",
        exts: &["scala", "sc", "sbt"],
        files: &[],
        line_comments: C_LINE,
        block_comment: C_BLOCK,
        quotes: "\"'",
        triple_quotes: true,
        keywords: &[
            "abstract", "case", "catch", "class", "def", "do", "else", "extends", "false", "final",
            "finally", "for", "forSome", "if", "implicit", "import", "lazy", "match", "new", "null",
            "object", "override", "package", "private", "protected", "return", "sealed", "super", "this",
            "throw", "trait", "try", "true", "type", "val", "var", "while", "with", "yield", "given",
            "using", "enum", "then",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Perl",
        exts: &["pl", "pm", "pod", "t"],
        files: &[],
        line_comments: HASH,
        block_comment: NONE,
        quotes: "\"'`",
        triple_quotes: false,
        keywords: &[
            "my", "our", "local", "sub", "package", "use", "no", "require", "if", "elsif", "else", "unless",
            "while", "until", "for", "foreach", "do", "last", "next", "redo", "return", "and", "or", "not",
            "eq", "ne", "lt", "gt", "le", "ge", "cmp", "print", "say", "die", "warn", "defined", "undef",
            "ref", "bless", "qw", "BEGIN", "END",
        ],
        ignore_case: false,
    },
    Spec {
        name: "R",
        exts: &["r", "rmd"],
        files: &[],
        line_comments: HASH,
        block_comment: NONE,
        quotes: "\"'`",
        triple_quotes: false,
        keywords: &[
            "if", "else", "repeat", "while", "function", "for", "in", "next", "break", "return", "TRUE",
            "FALSE", "NULL", "NA", "Inf", "NaN", "library", "require", "source",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Haskell",
        exts: &["hs", "lhs"],
        files: &[],
        line_comments: &["--"],
        block_comment: Some(("{-", "-}")),
        quotes: "\"",
        triple_quotes: false,
        keywords: &[
            "case", "class", "data", "default", "deriving", "do", "else", "forall", "foreign", "if", "import",
            "in", "infix", "infixl", "infixr", "instance", "let", "module", "newtype", "of", "then", "type",
            "where", "qualified", "as", "hiding",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Zig",
        exts: &["zig", "zon"],
        files: &[],
        line_comments: C_LINE,
        block_comment: NONE,
        quotes: "\"'",
        triple_quotes: false,
        keywords: &[
            "addrspace", "align", "allowzero", "and", "anyframe", "anytype", "asm", "async", "await", "break",
            "callconv", "catch", "comptime", "const", "continue", "defer", "else", "enum", "errdefer",
            "error", "export", "extern", "fn", "for", "if", "inline", "linksection", "noalias", "nosuspend",
            "opaque", "or", "orelse", "packed", "pub", "resume", "return", "struct", "suspend", "switch",
            "test", "threadlocal", "try", "union", "unreachable", "usingnamespace", "var", "volatile",
            "while", "true", "false", "null", "undefined",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Elixir",
        exts: &["ex", "exs", "heex", "eex"],
        files: &[],
        line_comments: HASH,
        block_comment: NONE,
        quotes: "\"'",
        triple_quotes: true,
        keywords: &[
            "def", "defp", "defmodule", "defmacro", "defmacrop", "defstruct", "defprotocol", "defimpl",
            "defdelegate", "do", "end", "fn", "if", "else", "unless", "case", "cond", "with", "for", "when",
            "in", "and", "or", "not", "import", "alias", "require", "use", "quote", "unquote", "receive",
            "after", "try", "rescue", "catch", "raise", "true", "false", "nil",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Julia",
        exts: &["jl"],
        files: &[],
        line_comments: HASH,
        block_comment: Some(("#=", "=#")),
        quotes: "\"'",
        triple_quotes: true,
        keywords: &[
            "baremodule", "begin", "break", "catch", "const", "continue", "do", "else", "elseif", "end",
            "export", "finally", "for", "function", "global", "if", "import", "let", "local", "macro",
            "module", "quote", "return", "struct", "try", "using", "while", "mutable", "abstract", "type",
            "true", "false", "nothing",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Groovy",
        exts: &["groovy", "gradle", "gvy"],
        files: &[],
        line_comments: C_LINE,
        block_comment: C_BLOCK,
        quotes: "\"'",
        triple_quotes: true,
        keywords: &[
            "abstract", "as", "assert", "break", "case", "catch", "class", "const", "continue", "def",
            "default", "do", "else", "enum", "extends", "final", "finally", "for", "goto", "if",
            "implements", "import", "in", "instanceof", "interface", "new", "package", "private",
            "protected", "public", "return", "static", "super", "switch", "this", "throw", "throws", "trait",
            "try", "while", "true", "false", "null",
        ],
        ignore_case: false,
    },
    Spec {
        name: "PowerShell",
        exts: &["ps1", "psm1", "psd1"],
        files: &[],
        line_comments: HASH,
        block_comment: Some(("<#", "#>")),
        quotes: "\"'",
        triple_quotes: false,
        keywords: &[
            "begin", "break", "catch", "class", "continue", "data", "do", "dynamicparam", "else", "elseif",
            "end", "exit", "filter", "finally", "for", "foreach", "from", "function", "if", "in", "param",
            "process", "return", "switch", "throw", "trap", "try", "until", "using", "var", "while",
            "workflow", "true", "false", "null",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Batch",
        exts: &["bat", "cmd"],
        files: &[],
        line_comments: &["REM ", "rem ", "::"],
        block_comment: NONE,
        quotes: "\"",
        triple_quotes: false,
        keywords: &[
            "echo", "set", "if", "else", "for", "in", "do", "goto", "call", "exit", "pause", "setlocal",
            "endlocal", "copy", "move", "del", "dir", "cd", "md", "rd", "start", "not", "exist", "errorlevel",
        ],
        ignore_case: false,
    },
    Spec {
        name: "LaTeX",
        exts: &["tex", "sty", "cls", "bib"],
        files: &[],
        line_comments: &["%"],
        block_comment: NONE,
        quotes: "",
        triple_quotes: false,
        keywords: &[
            "\\begin", "\\end", "\\documentclass", "\\usepackage", "\\section", "\\subsection", "\\chapter",
            "\\title", "\\author", "\\label", "\\ref", "\\cite", "\\item", "\\newcommand", "\\input",
            "\\include",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Clojure / Lisp",
        exts: &["clj", "cljs", "cljc", "edn", "lisp", "lsp", "el", "scm", "rkt", "ss"],
        files: &[],
        line_comments: &[";"],
        block_comment: NONE,
        quotes: "\"",
        triple_quotes: false,
        keywords: &[
            "def", "defn", "defmacro", "defmulti", "defmethod", "defprotocol", "defrecord", "defstruct",
            "defun", "defvar", "defparameter", "defconstant", "define", "lambda", "let", "let*", "letrec",
            "if", "cond", "case", "when", "unless", "do", "loop", "recur", "fn", "ns", "require", "import",
            "use", "quote", "setq", "set!", "begin", "and", "or", "not", "true", "false", "nil", "t",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Protocol Buffers",
        exts: &["proto"],
        files: &[],
        line_comments: C_LINE,
        block_comment: C_BLOCK,
        quotes: "\"'",
        triple_quotes: false,
        keywords: &[
            "syntax", "package", "import", "option", "message", "enum", "service", "rpc", "returns", "stream",
            "oneof", "map", "repeated", "optional", "required", "reserved", "extensions", "extend", "public",
            "weak", "true", "false", "int32", "int64", "uint32", "uint64", "sint32", "sint64", "fixed32",
            "fixed64", "sfixed32", "sfixed64", "bool", "string", "bytes", "float", "double",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Terraform / HCL",
        exts: &["tf", "tfvars", "hcl"],
        files: &[],
        line_comments: &["#", "//"],
        block_comment: C_BLOCK,
        quotes: "\"",
        triple_quotes: false,
        keywords: &[
            "resource", "data", "variable", "output", "module", "provider", "locals", "terraform", "for",
            "in", "if", "true", "false", "null", "dynamic", "backend", "required_providers",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Dockerfile",
        exts: &["dockerfile", "containerfile"],
        files: &["Dockerfile", "Containerfile"],
        line_comments: HASH,
        block_comment: NONE,
        quotes: "\"'",
        triple_quotes: false,
        keywords: &[
            "FROM", "RUN", "CMD", "LABEL", "MAINTAINER", "EXPOSE", "ENV", "ADD", "COPY", "ENTRYPOINT",
            "VOLUME", "USER", "WORKDIR", "ARG", "ONBUILD", "STOPSIGNAL", "HEALTHCHECK", "SHELL", "AS",
        ],
        ignore_case: false,
    },
    Spec {
        name: "GraphQL",
        exts: &["graphql", "gql"],
        files: &[],
        line_comments: HASH,
        block_comment: NONE,
        quotes: "\"",
        triple_quotes: true,
        keywords: &[
            "query", "mutation", "subscription", "fragment", "on", "type", "interface", "union", "enum",
            "input", "scalar", "schema", "extend", "directive", "implements", "true", "false", "null",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Erlang",
        exts: &["erl", "hrl"],
        files: &[],
        line_comments: &["%"],
        block_comment: NONE,
        quotes: "\"'",
        triple_quotes: false,
        keywords: &[
            "after", "and", "andalso", "band", "begin", "bnot", "bor", "bsl", "bsr", "bxor", "case", "catch",
            "cond", "div", "end", "fun", "if", "let", "not", "of", "or", "orelse", "receive", "rem", "try",
            "when", "xor", "true", "false",
        ],
        ignore_case: false,
    },
    Spec {
        name: "OCaml / F#",
        exts: &["ml", "mli", "fs", "fsx", "fsi", "re"],
        files: &[],
        line_comments: &["//"],
        block_comment: Some(("(*", "*)")),
        quotes: "\"",
        triple_quotes: false,
        keywords: &[
            "and", "as", "assert", "begin", "class", "do", "done", "downto", "else", "end", "exception",
            "external", "for", "fun", "function", "functor", "if", "in", "include", "inherit", "let", "match",
            "method", "module", "mutable", "new", "of", "open", "or", "rec", "sig", "struct", "then", "to",
            "try", "type", "val", "when", "while", "with", "true", "false", "namespace", "member", "use",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Assembly",
        exts: &["asm", "s", "nasm", "inc65"],
        files: &[],
        line_comments: &[";", "#", "//"],
        block_comment: C_BLOCK,
        quotes: "\"'",
        triple_quotes: false,
        keywords: &[
            "mov", "push", "pop", "call", "ret", "jmp", "je", "jne", "jz", "jnz", "jl", "jg", "jle", "jge",
            "cmp", "test", "add", "sub", "mul", "div", "inc", "dec", "and", "or", "xor", "not", "shl", "shr",
            "lea", "nop", "int", "syscall", "section", "global", "extern", "db", "dw", "dd", "dq", "equ",
        ],
        ignore_case: false,
    },
    Spec {
        name: "Nginx",
        exts: &["nginx"],
        files: &["nginx.conf"],
        line_comments: HASH,
        block_comment: NONE,
        quotes: "\"'",
        triple_quotes: false,
        keywords: &[
            "server", "location", "upstream", "http", "events", "listen", "server_name", "root", "index",
            "return", "rewrite", "proxy_pass", "fastcgi_pass", "include", "if", "set", "try_files", "error_page",
            "ssl_certificate", "ssl_certificate_key", "on", "off",
        ],
        ignore_case: false,
    },
    Spec {
        name: "C++",
        exts: &["cpp", "cc", "cxx", "c++", "hpp", "hh", "hxx", "h++", "ino", "ipp", "tpp"],
        files: &[],
        line_comments: C_LINE,
        block_comment: C_BLOCK,
        quotes: "\"'",
        triple_quotes: false,
        keywords: &[
            "alignas", "alignof", "and", "asm", "auto", "bool", "break", "case", "catch", "char", "class",
            "co_await", "co_return", "co_yield", "concept", "const", "consteval", "constexpr", "constinit",
            "const_cast", "continue", "decltype", "default", "delete", "do", "double", "dynamic_cast", "else",
            "enum", "explicit", "export", "extern", "false", "final", "float", "for", "friend", "goto", "if",
            "inline", "int", "long", "mutable", "namespace", "new", "noexcept", "not", "nullptr", "operator",
            "or", "override", "private", "protected", "public", "register", "reinterpret_cast", "requires",
            "return", "short", "signed", "sizeof", "static", "static_assert", "static_cast", "struct", "switch",
            "template", "this", "thread_local", "throw", "true", "try", "typedef", "typeid", "typename",
            "union", "unsigned", "using", "virtual", "void", "volatile", "while", "#include", "#define",
            "#ifdef", "#ifndef", "#endif", "#if", "#else", "#pragma",
        ],
        ignore_case: false,
    },
    Spec {
        name: "C#",
        exts: &["cs", "csx"],
        files: &[],
        line_comments: C_LINE,
        block_comment: C_BLOCK,
        quotes: "\"'",
        triple_quotes: true,
        keywords: &[
            "abstract", "as", "async", "await", "base", "bool", "break", "byte", "case", "catch", "char",
            "checked", "class", "const", "continue", "decimal", "default", "delegate", "do", "double", "else",
            "enum", "event", "explicit", "extern", "false", "finally", "fixed", "float", "for", "foreach",
            "get", "goto", "if", "implicit", "in", "int", "interface", "internal", "is", "lock", "long",
            "namespace", "new", "null", "object", "operator", "out", "override", "params", "partial",
            "private", "protected", "public", "readonly", "record", "ref", "return", "sbyte", "sealed",
            "set", "short", "sizeof", "static", "string", "struct", "switch", "this", "throw", "true", "try",
            "typeof", "uint", "ulong", "unchecked", "unsafe", "ushort", "using", "var", "virtual", "void",
            "volatile", "when", "where", "while", "yield", "init", "required",
        ],
        ignore_case: false,
    },
    Spec {
        name: "SQL",
        exts: &["sql", "psql", "mysql", "pgsql", "ddl"],
        files: &[],
        line_comments: &["--", "#"],
        block_comment: C_BLOCK,
        quotes: "\"'`",
        triple_quotes: false,
        keywords: &[
            "select", "from", "where", "and", "or", "not", "in", "is", "null", "like", "between", "exists",
            "insert", "into", "values", "update", "set", "delete", "create", "alter", "drop", "table", "index",
            "view", "database", "schema", "primary", "key", "foreign", "references", "unique", "default",
            "check", "constraint", "join", "inner", "left", "right", "full", "outer", "cross", "on", "using",
            "group", "by", "order", "having", "limit", "offset", "union", "all", "distinct", "as", "case",
            "when", "then", "else", "end", "begin", "commit", "rollback", "transaction", "if", "asc", "desc",
            "with", "recursive", "returning", "grant", "revoke", "truncate", "explain", "analyze", "int",
            "integer", "bigint", "varchar", "char", "text", "boolean", "date", "timestamp", "decimal",
            "float", "double", "serial", "auto_increment", "engine", "true", "false", "add", "column",
            "rename", "to", "cascade", "restrict", "replace", "temporary", "procedure", "function",
            "trigger", "declare", "return", "returns", "language", "for", "each", "row", "after", "before",
        ],
        ignore_case: true,
    },
];

/// The language *inside* `{{ }}` / `{% %}` of Twig, Jinja, Nunjucks and Liquid
/// templates (not a file type of its own: see [`paint_template`]).
static TEMPLATE: Spec = Spec {
    name: "Twig",
    exts: &[],
    files: &[],
    line_comments: &[],
    block_comment: None,
    quotes: "\"'",
    triple_quotes: false,
    keywords: &[
        "if", "elseif", "elif", "else", "endif", "for", "endfor", "in", "not", "and", "or", "is", "as", "with",
        "only", "ignore", "missing", "block", "endblock", "extends", "include", "embed", "endembed", "set",
        "endset", "macro", "endmacro", "import", "from", "use", "apply", "endapply", "autoescape",
        "endautoescape", "spaceless", "endspaceless", "verbatim", "endverbatim", "raw", "endraw", "sandbox",
        "endsandbox", "flush", "do", "deprecated", "cache", "endcache", "call", "endcall", "filter",
        "endfilter", "trans", "endtrans", "matches", "starts", "ends", "same", "divisible", "by", "defined",
        "empty", "even", "odd", "iterable", "null", "none", "true", "false", "assign", "capture",
        "endcapture", "unless", "endunless", "case", "when", "endcase", "render", "layout", "section",
        "endsection", "comment", "endcomment", "break", "continue", "loop", "super", "parent",
    ],
    ignore_case: false,
};

/// Paints the template tags of Twig / Jinja / Nunjucks / Liquid onto `paint`
/// (the host HTML is already painted): `{# comments #}`, and the expression
/// and statement tags `{{ ... }}` / `{% ... %}` (delimiters, keywords, strings,
/// numbers, `|filters`, `.properties`, calls). Tags are searched at most
/// [`MAX_TAG`] bytes ahead, so a stray `{{` while typing cannot recolor the
/// rest of the file.
pub fn paint_template(text: &str, paint: &mut [u8]) {
    const MAX_TAG: usize = 4000;
    const NONE: u8 = u8::MAX;
    let b = text.as_bytes();
    let fill = |paint: &mut [u8], from: usize, to: usize, g: Group| paint[from..to].fill(g as u8);
    let mut i = 0;
    while i + 1 < b.len() {
        let kind = b[i + 1];
        if b[i] != b'{' || !matches!(kind, b'{' | b'%' | b'#') {
            i += 1;
            continue;
        }
        let close: [u8; 2] = match kind {
            b'{' => *b"}}",
            b'%' => *b"%}",
            _ => *b"#}",
        };
        let limit = (i + MAX_TAG).min(b.len());
        let Some(end) = find_close(b, i + 2, limit, close, kind != b'#') else {
            i += 2;
            continue;
        };
        let stop = end + 2;
        if kind == b'#' {
            fill(paint, i, stop, Group::Comment);
            i = stop;
            continue;
        }
        paint[i..stop].fill(NONE); // forget what the HTML grammar made of the tag
        let trim = |c: Option<&u8>| matches!(c, Some(b'-' | b'~' | b'+'));
        let open_end = i + 2 + usize::from(trim(b.get(i + 2)));
        let close_start = if end > open_end && trim(b.get(end - 1)) { end - 1 } else { end };
        fill(paint, i, open_end, Group::Operator);
        fill(paint, close_start, stop, Group::Operator);
        let inner = &text[open_end..close_start];
        let painted = self::paint(&TEMPLATE, inner);
        for (k, &g) in painted.iter().enumerate() {
            if g != NONE {
                paint[open_end + k] = g;
            }
        }
        // `|filter` and `.property`, outside strings.
        let ib = inner.as_bytes();
        let ident_end = |from: usize| from + ib[from..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'_').count();
        for (k, &c) in ib.iter().enumerate() {
            if painted[k] != NONE || !matches!(c, b'|' | b'.') {
                continue;
            }
            let start = k + 1 + ib[k + 1..].iter().take_while(|c| **c == b' ').count();
            let end = ident_end(start);
            if end > start && !ib[start].is_ascii_digit() && matches!(painted[start], NONE) {
                let g = if c == b'|' || ib.get(end) == Some(&b'(') { Group::Function } else { Group::Property };
                fill(paint, open_end + start, open_end + end, g);
            }
        }
        i = stop;
    }
}

/// Index of the `close` marker at or after `from` (before `limit`), skipping
/// quoted strings when `strings` is set.
fn find_close(b: &[u8], from: usize, limit: usize, close: [u8; 2], strings: bool) -> Option<usize> {
    let mut j = from;
    while j + 1 < limit {
        if strings && matches!(b[j], b'"' | b'\'') {
            let q = b[j];
            j += 1;
            while j < limit && b[j] != q {
                j += usize::from(b[j] == b'\\') + 1;
            }
            j += 1;
            continue;
        }
        if b[j] == close[0] && b[j + 1] == close[1] {
            return Some(j);
        }
        j += 1;
    }
    None
}

/// Table index for a file extension / name.
pub fn lookup(ext: Option<&str>, name: &str) -> Option<u8> {
    SPECS
        .iter()
        .position(|s| s.files.contains(&name) || ext.is_some_and(|e| s.exts.contains(&e)))
        .map(|i| i as u8)
}

pub fn spec(i: u8) -> Option<&'static Spec> {
    SPECS.get(i as usize)
}

/// Classifies every byte of `text` (one `Group` index per byte, `u8::MAX` = plain).
pub fn paint(spec: &Spec, text: &str) -> Vec<u8> {
    const NONE: u8 = u8::MAX;
    let b = text.as_bytes();
    let mut out = vec![NONE; b.len()];
    let mut i = 0;
    let at = |i: usize, s: &str| b[i..].starts_with(s.as_bytes());
    let fill = |out: &mut Vec<u8>, from: usize, to: usize, g: Group| {
        out[from.min(b.len())..to.min(b.len())].fill(g as u8);
    };

    while i < b.len() {
        let c = b[i];

        if let Some((open, close)) = spec.block_comment {
            if at(i, open) {
                let end = text[i + open.len()..].find(close).map_or(b.len(), |p| i + open.len() + p + close.len());
                fill(&mut out, i, end, Group::Comment);
                i = end;
                continue;
            }
        }
        if spec.line_comments.iter().any(|m| at(i, m)) {
            let end = b[i..].iter().position(|&x| x == b'\n').map_or(b.len(), |p| i + p);
            fill(&mut out, i, end, Group::Comment);
            i = end;
            continue;
        }
        if c < 0x80 && spec.quotes.as_bytes().contains(&c) {
            let end = string_end(spec, b, i);
            fill(&mut out, i, end, Group::String);
            i = end;
            continue;
        }
        if c.is_ascii_digit() {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || (b[j] == b'.' && b.get(j + 1).is_some_and(u8::is_ascii_digit))) {
                j += 1;
            }
            fill(&mut out, i, j, Group::Number);
            i = j;
            continue;
        }
        if c == b'#' && spec.keywords.iter().any(|k| k.starts_with('#')) {
            // Preprocessor directive: `#include`, `#define`, ...
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_alphabetic() {
                j += 1;
            }
            if spec.keywords.contains(&&text[i..j]) {
                fill(&mut out, i, j, Group::Keyword);
                i = j;
                continue;
            }
        }
        if c == b'\\' && spec.keywords.iter().any(|k| k.starts_with('\\')) {
            // LaTeX control sequence.
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_alphabetic() {
                j += 1;
            }
            let word = &text[i..j];
            let g = if spec.keywords.contains(&word) { Group::Keyword } else { Group::Function };
            fill(&mut out, i, j, g);
            i = j.max(i + 1);
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            let word = &text[i..j];
            let is_keyword = if spec.ignore_case {
                spec.keywords.iter().any(|k| k.eq_ignore_ascii_case(word))
            } else {
                spec.keywords.contains(&word)
            };
            let g = if is_keyword {
                Some(if matches!(word, "true" | "false" | "null" | "nil" | "None" | "True" | "False" | "NULL" | "undefined" | "nothing" | "TRUE" | "FALSE" | "NA" | "NaN" | "Inf") {
                    Group::Constant
                } else {
                    Group::Keyword
                })
            } else if b.get(j) == Some(&b'(') {
                Some(Group::Function)
            } else if !spec.ignore_case && word.len() > 1 && word.bytes().all(|x| x.is_ascii_uppercase() || x.is_ascii_digit() || x == b'_') {
                Some(Group::Constant)
            } else if c.is_ascii_uppercase() {
                Some(Group::Type)
            } else {
                None
            };
            if let Some(g) = g {
                fill(&mut out, i, j, g);
            }
            i = j;
            continue;
        }
        // Skip a whole UTF-8 sequence at once.
        i += utf8_len(c);
    }
    out
}

fn utf8_len(first: u8) -> usize {
    match first {
        0..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

/// End (exclusive) of the string literal starting at `start`.
fn string_end(spec: &Spec, b: &[u8], start: usize) -> usize {
    let q = b[start];
    if spec.triple_quotes && b[start..].starts_with(&[q, q, q]) {
        let mut j = start + 3;
        while j < b.len() {
            if b[j] == b'\\' {
                j += 2;
            } else if b[j..].starts_with(&[q, q, q]) {
                return j + 3;
            } else {
                j += 1;
            }
        }
        return b.len();
    }
    let multiline = q == b'`';
    let mut j = start + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            x if x == q => return j + 1,
            b'\n' if !multiline => return j,
            _ => j += 1,
        }
    }
    b.len()
}

// ------------------------------------------------------------------ Apache config

/// Paints Apache configuration (`.htaccess`, `httpd.conf`, virtual hosts): `# comments`
/// (whole lines only, as in Apache), `<Section args>` / `</Section>` tags, the directive name
/// at the start of a line, and in the arguments: strings, numbers, `%{VARIABLES}` / `$1`
/// back-references, `[FLAGS]`, `+Options` / `-Options`, patterns (`^...$`), URLs, module
/// names and the usual `on` / `off` / `all` words. A line ending in `\` continues on the next.
pub fn paint_apache(text: &str) -> Vec<u8> {
    let b = text.as_bytes();
    let mut out = vec![u8::MAX; b.len()];
    let (mut start, mut continued) = (0, false);
    while start < b.len() {
        let end = b[start..].iter().position(|&c| c == b'\n').map_or(b.len(), |p| start + p);
        apache_line(text, start, end, continued, &mut out);
        continued = text[start..end].trim_end().ends_with('\\');
        start = end + 1;
    }
    out
}

fn apache_line(text: &str, start: usize, end: usize, continued: bool, out: &mut [u8]) {
    let b = text.as_bytes();
    let body = text[start..end].trim_end();
    let end = start + body.len();
    let s = start + (body.len() - body.trim_start().len());
    if s >= end {
        return;
    }
    let fill = |out: &mut [u8], from: usize, to: usize, g: Group| out[from..to].fill(g as u8);
    if !continued && b[s] == b'#' {
        fill(out, s, end, Group::Comment);
        return;
    }
    let ident = |from: usize| from + b[from..end].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'_').count();
    if !continued && b[s] == b'<' {
        let name = s + 1 + usize::from(b.get(s + 1) == Some(&b'/'));
        let name_end = ident(name);
        fill(out, s, name, Group::Punctuation);
        fill(out, name, name_end, Group::Tag);
        let close = if b[end - 1] == b'>' && end - 1 >= name_end { end - 1 } else { end };
        fill(out, close, end, Group::Punctuation);
        apache_args(text, name_end, close, out);
        return;
    }
    let mut pos = s;
    if !continued {
        let name_end = ident(s);
        fill(out, s, name_end, Group::Keyword);
        pos = name_end;
    }
    apache_args(text, pos, end, out);
}

fn apache_args(text: &str, from: usize, to: usize, out: &mut [u8]) {
    let b = text.as_bytes();
    let mut i = from;
    while i < to {
        let c = b[i];
        if c == b' ' || c == b'\t' {
            i += 1;
        } else if c == b'"' || c == b'\'' {
            let mut j = i + 1;
            while j < to && b[j] != c {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            let j = (j + 1).min(to);
            out[i..j].fill(Group::String as u8);
            i = j;
        } else {
            let j = i + b[i..to].iter().take_while(|c| !matches!(**c, b' ' | b'\t')).count();
            apache_word(&text[i..j], i, out);
            i = j;
        }
    }
}

fn apache_word(w: &str, at: usize, out: &mut [u8]) {
    let end = at + w.len();
    let fill = |out: &mut [u8], from: usize, to: usize, g: Group| out[from..to].fill(g as u8);
    let lower = w.to_ascii_lowercase();
    if w.len() >= 2 && w.starts_with('[') && w.ends_with(']') {
        // `[NC,L,R=301]`
        fill(out, at, at + 1, Group::Punctuation);
        fill(out, end - 1, end, Group::Punctuation);
        let mut k = at + 1;
        for flag in w[1..w.len() - 1].split(',') {
            let (name, value) = flag.split_once('=').map_or((flag, None), |(n, v)| (n, Some(v)));
            fill(out, k, k + name.len(), Group::Type);
            if let Some(v) = value {
                fill(out, k + name.len(), k + name.len() + 1, Group::Operator);
                let g = if v.bytes().all(|c| c.is_ascii_digit()) && !v.is_empty() { Some(Group::Number) } else { None };
                if let Some(g) = g {
                    fill(out, k + name.len() + 1, k + flag.len(), g);
                }
            }
            k += flag.len();
            if k < end - 1 {
                fill(out, k, k + 1, Group::Punctuation);
                k += 1;
            }
        }
        return;
    }
    let group = if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("ftp://") {
        Some(Group::Attribute)
    } else if w.starts_with('^') || w.starts_with("!^") || w.contains('\\') || w.contains("(.*") || w.contains("[^") || w.contains("(?") || (w.len() > 1 && w.ends_with('$')) {
        Some(Group::String)
    } else if w.bytes().all(|c| c.is_ascii_digit()) {
        Some(Group::Number)
    } else if lower.trim_start_matches('!').starts_with("mod_") || lower.ends_with("_module") {
        Some(Group::Type)
    } else if matches!(
        lower.as_str(),
        "on" | "off" | "all" | "none" | "granted" | "denied" | "true" | "false" | "yes" | "no" | "env" | "expr"
            | "valid-user" | "user" | "group" | "ip" | "host" | "local" | "not" | "any" | "from" | "set"
            | "unset" | "append" | "merge" | "add" | "always" | "onsuccess" | "edit" | "setifempty" | "early"
    ) {
        Some(Group::Constant)
    } else {
        None
    };
    if let Some(g) = group {
        fill(out, at, end, g);
    } else {
        // `-MultiViews`, `+FollowSymLinks`, `!-f`, `-d`: the sign, then the word.
        let signs = w.bytes().take_while(|c| matches!(c, b'+' | b'-' | b'!')).count();
        if signs > 0 && signs < w.len() && w.as_bytes()[signs].is_ascii_alphabetic() && w[signs..].bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            fill(out, at, at + signs, Group::Operator);
            fill(out, at + signs, end, Group::Constant);
        }
    }
    // Variables and back-references, wherever they sit in the word.
    let b = w.as_bytes();
    let mut k = 0;
    while k + 1 < b.len() {
        if matches!(b[k], b'%' | b'$') && b[k + 1] == b'{' {
            if let Some(close) = b[k..].iter().position(|&c| c == b'}') {
                fill(out, at + k, at + k + close + 1, Group::Property);
                k += close + 1;
                continue;
            }
        } else if matches!(b[k], b'%' | b'$') && b[k + 1].is_ascii_digit() {
            fill(out, at + k, at + k + 2, Group::Property);
            k += 2;
            continue;
        }
        k += 1;
    }
}
