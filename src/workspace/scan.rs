//! Directory listing for the sidebar tree (one level at a time, `.gitignore`-aware).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

/// Lists the direct children of `dir`: directories first, then files, each
/// group sorted case-insensitively. Honors `.gitignore`/`.ignore` (even outside
/// a git repo) and never shows `.git`. Blocking: call from a worker task.
pub fn read_dir(dir: &Path) -> Vec<Entry> {
    let walker = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build();

    let mut out: Vec<Entry> = walker
        .filter_map(Result::ok)
        .filter(|e| e.depth() == 1)
        .map(|e| {
            let path = e.path().to_path_buf();
            // `is_dir()` on the path follows symlinks (symlinked dirs are dirs).
            let is_dir = e.file_type().is_some_and(|t| t.is_dir()) || path.is_dir();
            Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                path,
                is_dir,
            }
        })
        .collect();

    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}
