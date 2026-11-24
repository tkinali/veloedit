//! Git gutter: which lines differ from `HEAD`.
//!
//! The base text comes from the `git` binary (`git show HEAD:./file`), which
//! keeps VeloEdit free of a libgit2 dependency; without git, or for untracked
//! files, there simply are no marks. The diff itself is `similar` (Myers).

use std::{path::Path, process::{Command, Stdio}, sync::Arc};

pub mod status;

use similar::{DiffOp, TextDiff};

/// Gutter marker for one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Added,
    Modified,
    /// Lines were removed directly *above* this one.
    Removed,
}

/// Per-line markers (index = line number).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct GitMarks {
    marks: Vec<Option<Mark>>,
}

impl GitMarks {
    pub fn get(&self, line: usize) -> Option<Mark> {
        self.marks.get(line).copied().flatten()
    }

    #[cfg(test)]
    fn as_vec(&self) -> Vec<Option<Mark>> {
        self.marks.clone()
    }
}

/// Where a document stands with respect to its `HEAD` version.
#[derive(Default)]
pub enum Base {
    #[default]
    NotLoaded,
    Loading,
    /// Not in a repository / untracked / git unavailable.
    Absent,
    Ready(Arc<String>),
}

pub struct GitState {
    pub base: Base,
    pub marks: Option<Arc<GitMarks>>,
    /// Buffer version last sent to the diff worker (`u64::MAX` = never).
    pub requested: u64,
    pub in_flight: bool,
}

impl Default for GitState {
    fn default() -> Self {
        Self { base: Base::NotLoaded, marks: None, requested: u64::MAX, in_flight: false }
    }
}

impl GitState {
    pub fn wants_base(&self) -> bool {
        matches!(self.base, Base::NotLoaded)
    }

    pub fn wants_diff(&self, version: u64) -> bool {
        matches!(self.base, Base::Ready(_)) && !self.in_flight && self.requested != version
    }
}

/// `git`, without a console window: a Windows GUI program that starts a console program
/// otherwise shows a black window for as long as it runs.
fn git() -> Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = Command::new("git");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// The file's content at `HEAD`. Blocking (spawns `git`); call from a worker.
pub fn base_text(path: &Path) -> Option<String> {
    let dir = path.parent()?;
    let name = path.file_name()?.to_str()?;
    let out = git()
        .arg("-C")
        .arg(dir)
        .arg("show")
        .arg(format!("HEAD:./{name}"))
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8(out.stdout).ok()).flatten()
}

/// Line-level diff of `base` against `current`. Line endings are normalized
/// first so a CRLF checkout of an LF blob is not "all modified".
pub fn diff(base: &str, current: &str) -> GitMarks {
    let norm = |s: &str| if s.contains('\r') { s.replace("\r\n", "\n") } else { s.to_owned() };
    let (base, current) = (norm(base), norm(current));
    let n = current.split('\n').count();
    let mut marks = vec![None; n];
    let mut set = |line: usize, m: Mark| {
        if let Some(slot) = marks.get_mut(line) {
            *slot = Some(m);
        }
    };
    for op in TextDiff::from_lines(&base, &current).ops() {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Insert { new_index, new_len, .. } => (new_index..new_index + new_len).for_each(|l| set(l, Mark::Added)),
            DiffOp::Delete { new_index, .. } => set(new_index.min(n.saturating_sub(1)), Mark::Removed),
            DiffOp::Replace { old_len, new_index, new_len, .. } => {
                for k in 0..new_len {
                    set(new_index + k, if k < old_len { Mark::Modified } else { Mark::Added });
                }
            }
        }
    }
    GitMarks { marks }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Mark::*;

    #[test]
    fn classifies_added_modified_and_removed_lines() {
        let base = "a\nb\nc\nd\ne\n";
        assert_eq!(diff(base, base).as_vec(), vec![None; 6], "identical -> no marks (+ final empty line)");

        // line 2 changed, a line inserted after "c", "e" removed.
        let cur = "a\nB\nc\nnew\nd\n";
        let m = diff(base, cur);
        assert_eq!(m.get(1), Some(Modified));
        assert_eq!(m.get(3), Some(Added));
        assert_eq!(m.get(0), None);
        assert_eq!(m.get(2), None);
        assert_eq!(m.get(5), Some(Removed), "deletion at the end is flagged on the last line");

        // Replacing 1 line with 3: first is modified, the extra two are added.
        let m = diff("x\ny\nz\n", "x\nY1\nY2\nY3\nz\n");
        assert_eq!((m.get(1), m.get(2), m.get(3)), (Some(Modified), Some(Added), Some(Added)));

        // Deleting in the middle marks the line below.
        let m = diff("a\nb\nc\n", "a\nc\n");
        assert_eq!(m.get(1), Some(Removed));
    }

    #[test]
    fn crlf_checkout_of_an_lf_blob_is_not_all_modified() {
        let m = diff("a\nb\n", "a\r\nb\r\nc\r\n");
        assert_eq!((m.get(0), m.get(1), m.get(2)), (None, None, Some(Added)));
    }

    #[test]
    fn base_text_reads_head_and_is_none_outside_a_repo() {
        if git().arg("--version").output().is_err() {
            return; // no git here: the feature degrades to "no marks"
        }
        let dir = std::env::temp_dir().join(format!("veloedit-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let ok = git()
                .arg("-C")
                .arg(&dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        let file = dir.join("f ile.txt");
        std::fs::write(&file, "one\ntwo\n").unwrap();
        assert!(base_text(&file).is_none(), "not a repository yet");
        git(&["init", "-q"]);
        assert!(base_text(&file).is_none(), "untracked");
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);
        std::fs::write(&file, "one\nTWO\nthree\n").unwrap();
        assert_eq!(base_text(&file).as_deref(), Some("one\ntwo\n"), "HEAD content, not the working tree");
        std::fs::remove_dir_all(dir).ok();
    }
}
