//! Project-wide file index for Ctrl+P (built off-thread, `.gitignore`-aware).

use std::path::{Path, PathBuf};

/// Safety valve for gigantic trees (e.g. a home directory added as a root).
pub const MAX_FILES: usize = 200_000;

#[derive(Debug, Clone)]
pub struct IndexedFile {
    pub path: PathBuf,
    /// Shown and matched: path relative to its root (prefixed by the root's
    /// name when several roots are open).
    pub rel: String,
}

impl IndexedFile {
    pub fn file_name(&self) -> &str {
        self.rel.rsplit('/').next().unwrap_or(&self.rel)
    }

    /// Directory part of `rel` ("" for top-level files).
    pub fn dir(&self) -> &str {
        self.rel.rsplit_once('/').map_or("", |(d, _)| d)
    }
}

/// Blocking walk of all roots. Sorted by `rel` so an empty query is stable.
pub fn build(roots: &[PathBuf]) -> Vec<IndexedFile> {
    let prefix = roots.len() > 1;
    let mut out = Vec::new();
    'roots: for root in roots {
        let root_name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let walker = ignore::WalkBuilder::new(root)
            .hidden(false)
            .require_git(false)
            .follow_links(false)
            .filter_entry(|e| e.file_name() != ".git")
            .build();
        for entry in walker.filter_map(Result::ok) {
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let rel = relative(root, entry.path());
            out.push(IndexedFile {
                path: entry.path().to_path_buf(),
                rel: if prefix { format!("{root_name}/{rel}") } else { rel },
            });
            if out.len() >= MAX_FILES {
                break 'roots;
            }
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
