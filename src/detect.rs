//! File-type detection: decides *how* a file gets opened (text / image / hex).
//!
//! Content beats extension: only the first [`SNIFF_LEN`] bytes are read, the
//! extension (via `mime_guess`) is merely a tie-breaker for formats whose magic
//! number is too weak to trust on its own (BMP, ICO).

use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

/// How many leading bytes are inspected.
pub const SNIFF_LEN: usize = 8 * 1024;

/// Text files larger than this are opened in the (lazy) hex view instead, so a
/// stray 2 GB log file can never freeze the UI.
pub const TEXT_MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    WebP,
    Bmp,
    Ico,
}

impl ImageFormat {
    pub fn name(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Gif => "GIF",
            Self::WebP => "WebP",
            Self::Bmp => "BMP",
            Self::Ico => "ICO",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Text,
    Image(ImageFormat),
    Binary,
}

#[derive(Debug, Clone, Copy)]
pub struct Detection {
    pub kind: FileKind,
    /// The content looked like text but the file exceeds [`TEXT_MAX_BYTES`].
    pub text_too_large: bool,
}

/// Inspects `path` (reads at most [`SNIFF_LEN`] bytes).
pub fn detect(path: &Path) -> io::Result<Detection> {
    let file = File::open(path)?;
    let meta = file.metadata()?;
    if meta.is_dir() {
        return Err(io::Error::other("is a directory"));
    }
    let size = meta.len();

    let mut head = Vec::with_capacity(SNIFF_LEN.min(size as usize + 1));
    file.take(SNIFF_LEN as u64).read_to_end(&mut head)?;

    let ext = path.extension().and_then(|e| e.to_str());
    let mut kind = classify(&head, ext);
    let mut text_too_large = false;
    if kind == FileKind::Text && size > TEXT_MAX_BYTES {
        kind = FileKind::Binary;
        text_too_large = true;
    }
    Ok(Detection {
        kind,
        text_too_large,
    })
}

/// Pure classification of a file head. Kept free of I/O so it is unit-testable.
pub fn classify(head: &[u8], ext: Option<&str>) -> FileKind {
    if let Some(fmt) = sniff_image(head, ext) {
        return FileKind::Image(fmt);
    }
    if looks_like_text(head) {
        FileKind::Text
    } else {
        FileKind::Binary
    }
}

fn sniff_image(head: &[u8], ext: Option<&str>) -> Option<ImageFormat> {
    // Strong signatures: trusted regardless of the extension.
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(ImageFormat::Png);
    }
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(ImageFormat::Jpeg);
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Some(ImageFormat::Gif);
    }
    if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return Some(ImageFormat::WebP);
    }
    // Weak signatures: only when the extension agrees it is an image.
    if ext.is_some_and(is_image_ext) {
        if head.starts_with(b"BM") {
            return Some(ImageFormat::Bmp);
        }
        if head.starts_with(&[0, 0, 1, 0]) {
            return Some(ImageFormat::Ico);
        }
    }
    None
}

/// `image/*` by extension, minus SVG (SVG is XML text and stays editable).
fn is_image_ext(ext: &str) -> bool {
    mime_guess::from_ext(ext).iter().any(|m| {
        m.type_() == mime_guess::mime::IMAGE && m.subtype().as_str() != "svg"
    })
}

fn looks_like_text(head: &[u8]) -> bool {
    if head.is_empty() {
        return true;
    }
    // NUL bytes: binary (also catches UTF-16/32 until encoding support lands).
    if head.contains(&0) {
        return false;
    }
    if let Err(e) = std::str::from_utf8(head) {
        // A multi-byte sequence cut off by the sniff window is fine; anything
        // else is invalid UTF-8 -> do not risk corrupting it on save.
        if e.error_len().is_some() || head.len() - e.valid_up_to() > 3 {
            return false;
        }
    }
    let ctrl = head
        .iter()
        .filter(|&&b| b < 0x20 && !matches!(b, b'\n' | b'\r' | b'\t' | 0x0c | 0x1b))
        .count();
    ctrl * 10 <= head.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_and_empty() {
        assert_eq!(classify(b"", None), FileKind::Text);
        assert_eq!(classify(b"fn main() {}\n", Some("rs")), FileKind::Text);
        assert_eq!(classify("merhaba dünya ğüşiöç\n".as_bytes(), None), FileKind::Text);
    }

    #[test]
    fn nul_means_binary() {
        assert_eq!(classify(b"MZ\x90\x00\x03\x00", Some("exe")), FileKind::Binary);
        assert_eq!(classify(&[0xFF, 0xFE, b'h', 0, b'i', 0], None), FileKind::Binary);
    }

    #[test]
    fn invalid_utf8_is_binary_but_truncated_tail_is_ok() {
        assert_eq!(classify(&[b'a', 0xC3, 0x28, b'b'], None), FileKind::Binary);
        // "ğ" = C4 9F, window cut after the first byte.
        assert_eq!(classify(&[b'a', b'b', 0xC4], None), FileKind::Text);
    }

    #[test]
    fn images_by_magic() {
        assert_eq!(
            classify(b"\x89PNG\r\n\x1a\n\0\0", Some("txt")),
            FileKind::Image(ImageFormat::Png)
        );
        assert_eq!(
            classify(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10], None),
            FileKind::Image(ImageFormat::Jpeg)
        );
        assert_eq!(
            classify(b"RIFF\x24\0\0\0WEBPVP8 ", None),
            FileKind::Image(ImageFormat::WebP)
        );
    }

    #[test]
    fn weak_magic_needs_extension() {
        // A text file that merely starts with "BM" must stay text.
        assert_eq!(classify(b"BM25 ranking notes\n", Some("txt")), FileKind::Text);
        assert_eq!(
            classify(b"BM\x36\x00\x00\x00\x00", Some("bmp")),
            FileKind::Image(ImageFormat::Bmp)
        );
    }

    #[test]
    fn svg_stays_text() {
        assert_eq!(classify(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>", Some("svg")), FileKind::Text);
    }
}
