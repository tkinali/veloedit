//! The short colored label in front of a file name in the sidebar ("PHP", "JS",
//! "MD", ...). Text, not images: no assets, no decoding, no binary size.

use iced::Color;

const fn rgb(hex: u32) -> Color {
    Color {
        r: ((hex >> 16) & 0xFF) as f32 / 255.0,
        g: ((hex >> 8) & 0xFF) as f32 / 255.0,
        b: (hex & 0xFF) as f32 / 255.0,
        a: 1.0,
    }
}

const DIM: u32 = 0x7d8590;

/// `(label, color)` for a file; `light` darkens the colors for light themes.
pub fn badge(name: &str, light: bool) -> (String, Color) {
    let (label, hex) = lookup(name);
    let mut color = rgb(hex);
    if light {
        color = Color { r: color.r * 0.72, g: color.g * 0.72, b: color.b * 0.72, a: 1.0 };
    }
    (label, color)
}

fn lookup(name: &str) -> (String, u32) {
    let by_name = match name {
        "Dockerfile" | "Containerfile" => Some(("DKR", 0x2496ed)),
        "Makefile" | "makefile" | "GNUmakefile" => Some(("MK", 0x8a9aa5)),
        "CMakeLists.txt" => Some(("CMK", 0x4a90d9)),
        "Cargo.toml" | "Cargo.lock" => Some(("CGO", 0xdea584)),
        "package.json" | "package-lock.json" => Some(("NPM", 0xcb3837)),
        "composer.json" | "composer.lock" => Some(("CMP", 0xb6845c)),
        "LICENSE" | "LICENCE" | "COPYING" => Some(("LIC", DIM)),
        ".gitignore" | ".gitattributes" | ".gitmodules" => Some(("GIT", 0xf1502f)),
        ".editorconfig" | ".htaccess" => Some(("CFG", 0x8a9aa5)),
        _ => None,
    };
    if let Some((l, c)) = by_name {
        return (l.to_owned(), c);
    }
    if name.starts_with(".env") {
        return ("ENV".to_owned(), 0xecd53f);
    }
    let Some((_, ext)) = name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()) else {
        return ("\u{00B7}".to_owned(), DIM);
    };
    let ext = ext.to_ascii_lowercase();
    let known = match ext.as_str() {
        "php" | "phtml" | "inc" => ("PHP", 0x8892bf),
        "twig" => ("TWG", 0x9bcf3a),
        "js" | "mjs" | "cjs" => ("JS", 0xe5c07b),
        "jsx" => ("JSX", 0x61dafb),
        "ts" | "mts" | "cts" => ("TS", 0x3b82c4),
        "tsx" => ("TSX", 0x3b82c4),
        "vue" => ("VUE", 0x41b883),
        "svelte" => ("SVE", 0xff3e00),
        "html" | "htm" | "xhtml" => ("HTM", 0xe44d26),
        "css" => ("CSS", 0x7e57c2),
        "scss" | "sass" | "less" => ("SCS", 0xd6609c),
        "json" | "jsonc" => ("JSON", 0xcbcb41),
        "yml" | "yaml" => ("YML", 0xd9534f),
        "toml" => ("TOML", 0xc2693a),
        "xml" | "xsl" | "xsd" | "plist" => ("XML", 0xe37933),
        "svg" => ("SVG", 0xffb13b),
        "md" | "markdown" | "mdx" => ("MD", 0x519aba),
        "rs" => ("RS", 0xdea584),
        "py" | "pyi" | "pyw" => ("PY", 0x4b8bbe),
        "rb" | "rake" | "gemspec" => ("RB", 0xcc342d),
        "go" => ("GO", 0x00add8),
        "java" => ("JV", 0xe76f00),
        "kt" | "kts" => ("KT", 0xa97bff),
        "swift" => ("SWF", 0xf05138),
        "dart" => ("DRT", 0x00b4ab),
        "c" => ("C", 0x599eff),
        "h" => ("H", 0x9ab3d5),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => ("C++", 0xf34b7d),
        "cs" => ("C#", 0x68b340),
        "lua" => ("LUA", 0x51a0cf),
        "pl" | "pm" => ("PL", 0x39a0d4),
        "sql" => ("SQL", 0xe38c00),
        "sh" | "bash" | "zsh" | "fish" => ("SH", 0x89e051),
        "ps1" | "psm1" => ("PS", 0x4a90d9),
        "bat" | "cmd" => ("BAT", 0x89e051),
        "env" => ("ENV", 0xecd53f),
        "ini" | "cfg" | "conf" | "properties" => ("CFG", 0x8a9aa5),
        "txt" | "text" => ("TXT", DIM),
        "log" => ("LOG", DIM),
        "csv" | "tsv" => ("CSV", 0x6cc070),
        "lock" => ("LCK", DIM),
        "diff" | "patch" => ("DIF", 0xe5c07b),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "avif" => ("IMG", 0xa074c4),
        "pdf" => ("PDF", 0xe5484d),
        "zip" | "tar" | "gz" | "tgz" | "xz" | "bz2" | "7z" | "rar" => ("ZIP", 0xb0b7c0),
        "bin" | "exe" | "so" | "o" | "a" | "dll" => ("BIN", DIM),
        "woff" | "woff2" | "ttf" | "otf" => ("FNT", 0xb0b7c0),
        "mp3" | "wav" | "ogg" | "flac" | "mp4" | "mkv" | "webm" | "mov" => ("MED", 0xe0a040),
        _ => ("", 0),
    };
    if !known.0.is_empty() {
        return (known.0.to_owned(), known.1);
    }
    // Unknown extension: show it, dimmed.
    (ext.chars().take(4).collect::<String>().to_uppercase(), DIM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_follow_name_then_extension() {
        let label = |n: &str| badge(n, false).0;
        assert_eq!(label("index.php"), "PHP");
        assert_eq!(label("INDEX.PHP"), "PHP");
        assert_eq!(label("base.html.twig"), "TWG");
        assert_eq!(label("README.md"), "MD");
        assert_eq!(label("Dockerfile"), "DKR");
        assert_eq!(label("Cargo.toml"), "CGO", "file names win over extensions");
        assert_eq!(label(".env.local"), "ENV");
        assert_eq!(label(".gitignore"), "GIT");
        assert_eq!(label("notes.weird"), "WEIR", "unknown extensions are shown, shortened");
        assert_eq!(label("LICENSE"), "LIC");
        assert_eq!(label("noextension"), "\u{00B7}");
        assert_eq!(label(".hidden"), "\u{00B7}", "a leading dot is not an extension");
        assert!(label("x.json").chars().count() <= 4);
    }

    #[test]
    fn light_themes_get_darker_colors() {
        let (_, dark) = badge("a.js", false);
        let (_, light) = badge("a.js", true);
        assert!(light.r < dark.r && light.g < dark.g && light.b < dark.b);
    }
}
