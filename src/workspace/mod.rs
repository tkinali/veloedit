//! Multi-root workspace model: a lazily populated forest of directory nodes.
//!
//! Directories are only scanned when first expanded (see [`scan::read_dir`]),
//! so adding a huge folder costs one `readdir`, not a full walk.

mod scan;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

pub use scan::{Entry, read_dir};

#[derive(Debug)]
pub struct Node {
    pub entry: Entry,
    pub expanded: bool,
    /// `None` until the directory has been scanned.
    pub children: Option<Vec<Node>>,
}

impl Node {
    fn new(entry: Entry) -> Self {
        Self {
            entry,
            expanded: false,
            children: None,
        }
    }

    fn find_mut(&mut self, path: &Path) -> Option<&mut Node> {
        if self.entry.path == path {
            return Some(self);
        }
        let kids = self.children.as_mut()?;
        let idx = kids.iter().position(|c| path.starts_with(&c.entry.path))?;
        kids[idx].find_mut(path)
    }

    fn collect_expanded(&self, out: &mut Vec<PathBuf>) {
        if self.entry.is_dir && self.expanded {
            out.push(self.entry.path.clone());
            for c in self.children.iter().flatten() {
                c.collect_expanded(out);
            }
        }
    }

    fn flatten<'a>(&'a self, depth: usize, out: &mut Vec<Row<'a>>) {
        out.push(Row { depth, node: self });
        if self.expanded {
            for c in self.children.iter().flatten() {
                c.flatten(depth + 1, out);
            }
        }
    }
}

/// One visible line of the sidebar.
pub struct Row<'a> {
    pub depth: usize,
    pub node: &'a Node,
}

#[derive(Default)]
pub struct Workspace {
    roots: Vec<Node>,
    /// Directories that should be expanded as soon as their parent is scanned
    /// (session restore: children arrive asynchronously, in any order).
    pending_expand: HashSet<PathBuf>,
}

impl Workspace {
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    pub fn root_paths(&self) -> Vec<PathBuf> {
        self.roots.iter().map(|r| r.entry.path.clone()).collect()
    }

    pub fn expanded_paths(&self) -> Vec<PathBuf> {
        let mut v = Vec::new();
        self.roots.iter().for_each(|r| r.collect_expanded(&mut v));
        v
    }

    /// Adds a root folder. Returns `true` when it needs an initial scan.
    pub fn add_root(&mut self, path: PathBuf, expanded: bool) -> bool {
        if self.roots.iter().any(|r| r.entry.path == path) {
            return false;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let mut node = Node::new(Entry {
            path,
            name,
            is_dir: true,
        });
        node.expanded = expanded;
        self.roots.push(node);
        expanded
    }

    pub fn remove_root(&mut self, path: &Path) {
        self.roots.retain(|r| r.entry.path != path);
    }

    pub fn expand_later(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.pending_expand.extend(paths);
    }

    /// Toggles a directory. Returns `true` when it must be (re)scanned:
    /// every expansion rescans, because a collapsed directory is not watched
    /// and may have changed (old children stay visible until the scan lands).
    pub fn toggle(&mut self, path: &Path) -> bool {
        let Some(node) = self.find_mut(path) else {
            return false;
        };
        if !node.entry.is_dir {
            return false;
        }
        node.expanded = !node.expanded;
        node.expanded
    }

    pub fn is_expanded(&mut self, path: &Path) -> bool {
        self.find_mut(path).is_some_and(|n| n.entry.is_dir && n.expanded)
    }

    /// Collapses `path` and every directory below it, so opening it again shows a tidy
    /// one-level listing.
    pub fn collapse_recursive(&mut self, path: &Path) {
        fn walk(n: &mut Node) {
            n.expanded = false;
            n.children.iter_mut().flatten().for_each(walk);
        }
        if let Some(node) = self.find_mut(path) {
            walk(node);
        }
    }

    /// Directories that are expanded *and* loaded: exactly what the sidebar
    /// shows, hence what the watcher has to observe.
    pub fn watched_dirs(&self) -> Vec<PathBuf> {
        fn walk(n: &Node, out: &mut Vec<PathBuf>) {
            if n.entry.is_dir && n.expanded {
                if let Some(kids) = &n.children {
                    out.push(n.entry.path.clone());
                    kids.iter().for_each(|k| walk(k, out));
                }
            }
        }
        let mut v = Vec::new();
        self.roots.iter().for_each(|r| walk(r, &mut v));
        v
    }

    /// Is `path` a directory whose listing is currently shown?
    pub fn is_loaded(&self, path: &Path) -> bool {
        self.watched_dirs().iter().any(|d| d == path)
    }

    /// Installs scan results; returns child directories that still have to be
    /// scanned because the restored session had them expanded.
    pub fn set_children(&mut self, path: &Path, entries: Vec<Entry>) -> Vec<PathBuf> {
        let mut follow_up = Vec::new();
        let pending = &mut self.pending_expand;
        let Some(node) = self
            .roots
            .iter_mut()
            .find(|r| path.starts_with(&r.entry.path))
            .and_then(|r| r.find_mut(path))
        else {
            return follow_up;
        };
        // Keep what the user already opened: unchanged children are reused with
        // their expansion state and loaded subtrees.
        let mut old: std::collections::HashMap<PathBuf, Node> = node
            .children
            .take()
            .into_iter()
            .flatten()
            .map(|n| (n.entry.path.clone(), n))
            .collect();
        node.children = Some(
            entries
                .into_iter()
                .map(|e| match old.remove(&e.path) {
                    Some(mut kept) if kept.entry.is_dir == e.is_dir => {
                        kept.entry = e;
                        kept
                    }
                    _ => {
                        let mut n = Node::new(e);
                        if n.entry.is_dir && pending.remove(&n.entry.path) {
                            n.expanded = true;
                            follow_up.push(n.entry.path.clone());
                        }
                        n
                    }
                })
                .collect(),
        );
        follow_up
    }

    pub fn rows(&self) -> Vec<Row<'_>> {
        let mut out = Vec::new();
        self.roots.iter().for_each(|r| r.flatten(0, &mut out));
        out
    }

    fn find_mut(&mut self, path: &Path) -> Option<&mut Node> {
        self.roots
            .iter_mut()
            .find(|r| path.starts_with(&r.entry.path))
            .and_then(|r| r.find_mut(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(p: &str, dir: bool) -> Entry {
        Entry {
            path: PathBuf::from(p),
            name: p.rsplit('/').next().unwrap().into(),
            is_dir: dir,
        }
    }

    #[test]
    fn lazy_expand_and_restore_cascade() {
        let mut ws = Workspace::default();
        assert!(ws.add_root("/r".into(), true));
        assert!(!ws.add_root("/r".into(), true), "duplicate root ignored");
        ws.expand_later(["/r/a".into(), "/r/a/b".into()]);

        let more = ws.set_children(Path::new("/r"), vec![e("/r/a", true), e("/r/f.txt", false)]);
        assert_eq!(more, vec![PathBuf::from("/r/a")]);
        let more = ws.set_children(Path::new("/r/a"), vec![e("/r/a/b", true)]);
        assert_eq!(more, vec![PathBuf::from("/r/a/b")]);

        let rows: Vec<_> = ws.rows().iter().map(|r| (r.depth, r.node.entry.name.clone())).collect();
        assert_eq!(rows, vec![(0, "r".into()), (1, "a".into()), (2, "b".into()), (1, "f.txt".into())]);
        assert!(!ws.toggle(Path::new("/r/a")), "collapse never needs a scan");
        assert_eq!(ws.rows().len(), 3);
    }

    #[test]
    fn rescan_keeps_expansion_and_tracks_changes() {
        let mut ws = Workspace::default();
        ws.add_root("/r".into(), true);
        ws.set_children(Path::new("/r"), vec![e("/r/a", true), e("/r/old.txt", false)]);
        ws.set_children(Path::new("/r/a"), vec![e("/r/a/deep.txt", false)]);
        assert_eq!(ws.watched_dirs(), vec![PathBuf::from("/r")]);
        assert!(ws.toggle(Path::new("/r/a")), "expanding asks for a fresh scan");
        assert_eq!(ws.watched_dirs().len(), 2);

        // A file appears, another vanishes; /r/a stays expanded and loaded.
        ws.set_children(Path::new("/r"), vec![e("/r/a", true), e("/r/new.txt", false)]);
        let rows: Vec<_> = ws.rows().iter().map(|r| r.node.entry.name.clone()).collect();
        assert_eq!(rows, vec!["r", "a", "deep.txt", "new.txt"]);
        assert!(ws.is_loaded(Path::new("/r/a")) && !ws.is_loaded(Path::new("/r/zzz")));
    }
}
