//! Async document loading. Everything here runs on a worker thread; the UI
//! thread only receives ready-made, `Send` results.

use std::{fs, path::PathBuf, time::Duration};

use iced::futures::executor::block_on;

use super::{HexDoc, ImageDoc, text};
use crate::{
    detect::{self, FileKind},
    fsx,
    session::{self, TabKind, TabRecord},
};

/// How long restoring one tab may wait for its file. A file on a mount that does not answer
/// (a dropped VPN) is left out instead of holding every other tab back.
const RESTORE_STEP: Duration = Duration::from_secs(5);

pub enum Body {
    /// Raw text; the (non-`Send`) editor `Content` is built on the UI thread.
    Text(String),
    Hex(HexDoc),
    Image(ImageDoc),
}

pub struct Loaded {
    pub path: Option<PathBuf>,
    pub body: Body,
    /// Text restored from a draft that differs from disk.
    pub dirty: bool,
    /// Shown in the status bar, e.g. why a file opened as hex.
    pub notice: Option<String>,
    /// Hash of the bytes read from disk (text only; 0 otherwise / unknown).
    pub disk_hash: u64,
}

/// Detects the file kind and loads it accordingly.
pub async fn open(path: PathBuf) -> Result<Loaded, String> {
    let det = detect::detect(&path).map_err(|e| format!("{}: {e}", path.display()))?;

    let mut disk_hash = 0;
    let (body, notice) = match det.kind {
        FileKind::Text => match fs::read(&path).map(|b| (text::hash_bytes(&b), String::from_utf8(b))) {
            Ok((h, Ok(text))) => {
                disk_hash = h;
                (Body::Text(text), None)
            }
            Ok((_, Err(_))) => (hex(&path)?, Some("Not valid UTF-8 - opened as hex".to_owned())),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        },
        FileKind::Image(fmt) => match ImageDoc::load(&path, fmt) {
            Ok(img) => (Body::Image(img), None),
            Err(e) => (hex(&path)?, Some(format!("Image decode failed ({e}) - opened as hex"))),
        },
        FileKind::Binary => {
            let notice = det
                .text_too_large
                .then(|| "Text file over 16 MiB - opened as hex".to_owned());
            (hex(&path)?, notice)
        }
    };
    Ok(Loaded {
        path: Some(path),
        body,
        dirty: false,
        notice,
        disk_hash,
    })
}

/// [`open`] on a thread of its own: a read that never returns (a dead network mount) pins
/// that thread only, not the async runtime.
pub async fn open_off(path: PathBuf) -> Result<Loaded, String> {
    fsx::off_thread(move || block_on(open(path))).await.unwrap_or_else(|| Err("could not read the file".to_owned()))
}

/// Re-reads a file that changed on disk. One that is gone says so (checked here, on a
/// worker: asking a dead mount on the UI thread would freeze the window).
pub async fn reload(path: PathBuf) -> Result<Loaded, String> {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    fsx::off_thread(move || {
        block_on(open(path.clone())).map_err(|e| if path.exists() { e } else { format!("{name} was deleted on disk") })
    })
    .await
    .unwrap_or_else(|| Err("could not read the file".to_owned()))
}

fn hex(path: &std::path::Path) -> Result<Body, String> {
    HexDoc::open(path)
        .map(Body::Hex)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Rebuilds the tabs of a previous session (unsaved drafts included).
/// Records whose file vanished are dropped; returns the loaded tabs and the
/// index of the one that was active, shifted for the dropped ones.
pub async fn restore(records: Vec<TabRecord>, active: Option<usize>) -> (Vec<Loaded>, Option<usize>) {
    restore_within(records, active, RESTORE_STEP).await
}

pub async fn restore_within(records: Vec<TabRecord>, active: Option<usize>, step: Duration) -> (Vec<Loaded>, Option<usize>) {
    let mut loaded = Vec::new();
    let mut new_active = None;
    for (i, rec) in records.into_iter().enumerate() {
        let item = match rec.kind {
            TabKind::Text => restore_text(rec, step).await,
            TabKind::Hex | TabKind::Image => match rec.path {
                Some(p) => fsx::timeout(step, open_off(p)).await.and_then(Result::ok).map(|mut l| {
                    if let Body::Hex(h) = &mut l.body {
                        h.set_edits(rec.hex_edits.iter().copied());
                    }
                    l
                }),
                None => None,
            },
        };
        if let Some(item) = item {
            if active == Some(i) {
                new_active = Some(loaded.len());
            }
            loaded.push(item);
        }
    }
    (loaded, new_active)
}

async fn restore_text(rec: TabRecord, step: Duration) -> Option<Loaded> {
    if let Some(name) = &rec.draft {
        if let Some(text) = session::read_draft(name) {
            // A draft identical to the file on disk is not an unsaved change. (A file that
            // does not answer in time counts as different: the draft is what matters.)
            if let Some(path) = &rec.path {
                let (p, draft) = (path.clone(), text.clone().into_bytes());
                let same = fsx::timeout(step, fsx::off_thread(move || fs::read(p).is_ok_and(|disk| disk == draft))).await;
                if same == Some(Some(true)) {
                    return fsx::timeout(step, open_off(path.clone())).await.and_then(Result::ok);
                }
            }
            // Untitled buffers are only "dirty" once they hold something.
            let dirty = rec.path.is_some() || !text.is_empty();
            return Some(Loaded {
                path: rec.path,
                body: Body::Text(text),
                dirty,
                notice: None,
                disk_hash: 0, // unknown: first disk event decides
            });
        }
    }
    match rec.path {
        Some(p) => fsx::timeout(step, open_off(p)).await.and_then(Result::ok),
        // Untitled buffer whose draft was lost: nothing to restore.
        None => None,
    }
}
