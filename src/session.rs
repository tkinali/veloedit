//! Session persistence: open tabs, workspace roots and *unsaved drafts* survive
//! restarts ("hot exit"). Everything is plain JSON/text under the XDG state dir
//! (`~/.local/state/veloedit`), written atomically, off the UI thread.
//!
//! Drafts live in the state dir rather than `~/.cache` on purpose: cache
//! cleaners may delete them, and they are the user's only copy of unsaved work.

use std::{
    collections::HashSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabKind {
    Text,
    Hex,
    Image,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabRecord {
    pub kind: TabKind,
    pub path: Option<PathBuf>,
    /// File name inside the drafts dir holding the unsaved buffer.
    pub draft: Option<String>,
    /// Unsaved byte edits of a hex tab (`offset`, `byte`).
    #[serde(default)]
    pub hex_edits: Vec<(u64, u8)>,
}

/// Where the window was when it was closed (size and position are of the *normal*
/// window, even if it was maximized).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    pub width: f32,
    pub height: f32,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self { width: 1180.0, height: 760.0, x: None, y: None, maximized: false }
    }
}

impl WindowState {
    /// Values from a hand-edited or ancient file are made usable.
    pub fn sane(self) -> Self {
        let ok = |v: f32, lo: f32, hi: f32| v.is_finite() && (lo..=hi).contains(&v);
        let d = Self::default();
        Self {
            width: if ok(self.width, 520.0, 16_000.0) { self.width } else { d.width },
            height: if ok(self.height, 360.0, 16_000.0) { self.height } else { d.height },
            x: self.x.filter(|&v| ok(v, -4_000.0, 12_000.0)),
            y: self.y.filter(|&v| ok(v, -200.0, 12_000.0)),
            maximized: self.maximized,
        }
    }

    pub fn position(self) -> Option<(f32, f32)> {
        Some((self.x?, self.y?))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionFile {
    pub version: u32,
    pub sidebar_visible: bool,
    pub window: WindowState,
    /// Width of the sidebar in logical pixels (dragged by the user).
    pub sidebar_width: f32,
    /// Share of the editor area taken by the Markdown preview (dragged by the user).
    pub preview_frac: f32,
    /// Folder of the last file picked in an Open / Save dialog: the next dialog starts there.
    pub last_dir: Option<PathBuf>,
    pub roots: Vec<PathBuf>,
    /// Expanded directories (roots included; a root not listed here is collapsed).
    pub expanded: Vec<PathBuf>,
    pub tabs: Vec<TabRecord>,
    pub active: Option<usize>,
}

impl Default for SessionFile {
    fn default() -> Self {
        Self {
            version: VERSION,
            sidebar_visible: true,
            window: WindowState::default(),
            sidebar_width: crate::ui::sidebar::DEFAULT_WIDTH,
            preview_frac: 0.5,
            last_dir: None,
            roots: Vec::new(),
            expanded: Vec::new(),
            tabs: Vec::new(),
            active: None,
        }
    }
}

/// A session plus the draft bodies that changed since the last flush.
pub struct Snapshot {
    pub file: SessionFile,
    pub drafts: Vec<(String, String)>,
}

impl Snapshot {
    /// Blocking; run inside an async task. Order matters for crash safety:
    /// new drafts -> session.json -> garbage-collect unreferenced drafts.
    pub async fn write(self) -> io::Result<()> {
        let dir = state_dir();
        let drafts_dir = dir.join("drafts");
        fs::create_dir_all(&drafts_dir)?;

        for (name, body) in &self.drafts {
            atomic_write(&drafts_dir.join(name), body.as_bytes())?;
        }
        let json = serde_json::to_vec_pretty(&self.file).map_err(io::Error::other)?;
        atomic_write(&dir.join("session.json"), &json)?;

        let keep: HashSet<&str> = self
            .file
            .tabs
            .iter()
            .filter_map(|t| t.draft.as_deref())
            .collect();
        for entry in fs::read_dir(&drafts_dir)?.flatten() {
            if !keep.contains(entry.file_name().to_string_lossy().as_ref()) {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
}

/// `$VELOEDIT_STATE_DIR`, else `$XDG_STATE_HOME/veloedit`, else the data dir.
pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("VELOEDIT_STATE_DIR") {
        return PathBuf::from(dir);
    }
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("veloedit")
}

/// Reads the last session (tiny file; fine to do synchronously at boot).
pub fn load() -> SessionFile {
    fs::read(state_dir().join("session.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<SessionFile>(&b).ok())
        .filter(|s| s.version == VERSION)
        .unwrap_or_default()
}

pub fn read_draft(name: &str) -> Option<String> {
    // Draft names are generated by us; refuse anything path-like.
    if name.contains(['/', '\\']) || name.starts_with('.') {
        return None;
    }
    fs::read_to_string(state_dir().join("drafts").join(name)).ok()
}

pub fn draft_name(tab_id: u64) -> String {
    format!("draft-{tab_id}.txt")
}

/// Writes `bytes` to `path` via a temp file + rename, preserving permissions
/// of an existing file. Resolves symlinks so the link itself survives.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let tmp = target.with_file_name(format!(
        ".{}.veloedit-tmp",
        target.file_name().and_then(|n| n.to_str()).unwrap_or("file")
    ));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if let Ok(meta) = fs::metadata(&target) {
        let _ = fs::set_permissions(&tmp, meta.permissions());
    }
    fs::rename(&tmp, &target).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_content_and_keeps_no_temp() {
        let dir = std::env::temp_dir().join(format!("veloedit-sess-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        atomic_write(&f, b"one").unwrap();
        atomic_write(&f, b"two").unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "two");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn session_roundtrip_and_defaults() {
        let s: SessionFile = serde_json::from_str(r#"{"roots":["/x"]}"#).unwrap();
        assert!(s.sidebar_visible && s.tabs.is_empty());
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"/x\""));
    }

    #[test]
    fn window_state_is_made_usable_and_defaults_when_missing() {
        let s: SessionFile = serde_json::from_str("{}").unwrap();
        assert_eq!(s.window, WindowState::default());
        let w: WindowState = serde_json::from_str(r#"{"width": 1e9, "height": 5, "x": 99999, "y": -5, "maximized": true}"#).unwrap();
        let w = w.sane();
        assert_eq!((w.width, w.height), (1180.0, 760.0), "absurd sizes fall back");
        assert_eq!(w.x, None, "absurd positions are dropped");
        assert_eq!(w.y, Some(-5.0));
        assert!(w.maximized);
        assert_eq!(WindowState { x: Some(10.0), y: None, ..WindowState::default() }.position(), None, "needs both");
    }

    #[test]
    fn draft_names_cannot_escape() {
        assert!(read_draft("../session.json").is_none());
        assert!(read_draft(".hidden").is_none());
    }
}
