//! Which files of an open folder differ from `HEAD`: the colors and letters in the sidebar.
//!
//! One `git status` per repository, asked on a worker (never on the UI thread) and parsed
//! into a [`StatusMap`] keyed by absolute path, so a sidebar row looks itself up without
//! knowing which repository it belongs to. A folder inherits the strongest change inside it.
//! A workspace root that is inside a repository is one repository; one that is not (a
//! `/var/www` holding several sites) has one repository per direct subfolder that has a
//! `.git`. Without git, outside a repository, or on a mount that does not answer in time
//! there is simply no map and no colors.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use super::git;

/// How a path differs from `HEAD`. Ordered by what matters most (a folder shows the largest).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    /// Not tracked by git yet.
    Untracked,
    /// New in the index (`git add`).
    Added,
    Modified,
    /// A merge conflict.
    Conflict,
}

impl Change {
    /// The one-letter badge shown at the right edge of a file row.
    pub fn letter(self) -> &'static str {
        match self {
            Change::Untracked => "U",
            Change::Added => "A",
            Change::Modified => "M",
            Change::Conflict => "!",
        }
    }
}

/// The changes below one workspace root.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct StatusMap {
    /// Changed files, wholly untracked folders, and every folder that contains a change.
    exact: HashMap<PathBuf, Change>,
    /// Folders git reports as one untracked unit: what is inside is untracked as well.
    untracked_dirs: HashSet<PathBuf>,
    /// The strongest change anywhere in the repository (what its folder row shows).
    top: Option<Change>,
}

/// More changed paths than this are not worth coloring (and not worth the memory).
const MAX_ENTRIES: usize = 20_000;

impl StatusMap {
    pub fn get(&self, path: &Path) -> Option<Change> {
        if let Some(c) = self.exact.get(path) {
            return Some(*c);
        }
        if self.untracked_dirs.is_empty() {
            return None;
        }
        path.ancestors().skip(1).any(|a| self.untracked_dirs.contains(a)).then_some(Change::Untracked)
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.exact.is_empty()
    }

    /// Notes `change` for `path` and for the folders above it, up to (not including) `root`.
    fn note(&mut self, root: &Path, path: PathBuf, change: Change) {
        let mut up = path.parent().map(Path::to_path_buf);
        self.raise(path, change);
        while let Some(dir) = up {
            if dir == root || !dir.starts_with(root) {
                break;
            }
            up = dir.parent().map(Path::to_path_buf);
            self.raise(dir, change);
        }
    }

    fn raise(&mut self, path: PathBuf, change: Change) {
        self.top = self.top.max(Some(change));
        let slot = self.exact.entry(path).or_insert(change);
        *slot = (*slot).max(change);
    }
}

/// `git status` of one root, plus where git keeps its files (to watch for commits).
pub struct Fetched {
    pub map: StatusMap,
    /// The repository's own directory (`.git`), absolute.
    pub git_dir: PathBuf,
}

/// Longest a `git status` may take before nobody waits for it any more.
const DEADLINE: Duration = Duration::from_secs(30);

/// [`fetch`] on a worker thread with a deadline. `None`: not a repository, no git, too slow.
pub async fn fetch_bounded(root: PathBuf) -> Option<Arc<Fetched>> {
    let job = crate::fsx::off_thread(move || fetch(&root));
    crate::fsx::timeout(DEADLINE, job).await.flatten().flatten().map(Arc::new)
}

/// Blocking: spawns `git` twice. Call from a worker.
pub fn fetch(root: &Path) -> Option<Fetched> {
    // Where `root` sits inside the repository, and where the repository's files are.
    let head = run(root, &["rev-parse", "--show-prefix", "--absolute-git-dir"])?;
    let mut lines = head.split(|b| *b == b'\n');
    let prefix = String::from_utf8(lines.next()?.to_vec()).ok()?;
    let git_dir = PathBuf::from(String::from_utf8(lines.next()?.to_vec()).ok()?.trim_end_matches('\r'));

    // `--no-renames`: a rename is "new file + deleted file" (and costs no similarity search).
    // `--ignore-submodules=dirty`: do not descend into submodules. The pathspec keeps it to
    // what is below `root`; the paths in the answer are relative to the repository's top.
    let out = run(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal", "--no-renames", "--ignore-submodules=dirty", "--", "."],
    )?;
    Some(Fetched { map: parse(root, prefix.trim_end_matches('\r'), &out), git_dir })
}

fn run(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = git()
        .arg("-C")
        .arg(dir)
        .args(args)
        // A background `status` must not take the index lock: it would make a `git commit`
        // typed in a terminal at the same moment fail.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Parses `git status --porcelain=v1 -z`: `XY path` records separated by NUL.
fn parse(root: &Path, prefix: &str, out: &[u8]) -> StatusMap {
    let mut map = StatusMap::default();
    let mut records = out.split(|b| *b == 0).filter(|r| !r.is_empty());
    while let Some(rec) = records.next() {
        if rec.len() < 4 || rec[2] != b' ' {
            continue;
        }
        let (x, y) = (rec[0], rec[1]);
        if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
            records.next(); // a rename or copy is followed by its old name
        }
        let Some(rel) = path_of(&rec[3..]).and_then(|p| p.strip_prefix(prefix).map(str::to_owned)) else { continue };
        let is_dir = rel.ends_with('/');
        let rel = rel.trim_end_matches('/');
        if rel.is_empty() {
            continue;
        }
        let path = root.join(rel);

        let change = match (x, y) {
            (b'?', b'?') => Change::Untracked,
            (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => Change::Conflict,
            (b'A', _) => Change::Added,
            // A deleted file is not in the tree; the folders above it are "modified".
            (b'D', _) | (_, b'D') => {
                if let Some(dir) = path.parent().filter(|d| *d != root) {
                    map.note(root, dir.to_path_buf(), Change::Modified);
                }
                continue;
            }
            _ => Change::Modified,
        };
        if is_dir && change == Change::Untracked {
            map.untracked_dirs.insert(path.clone());
        }
        map.note(root, path, change);
        if map.exact.len() >= MAX_ENTRIES {
            break;
        }
    }
    map
}

fn path_of(raw: &[u8]) -> Option<String> {
    String::from_utf8(raw.to_vec()).ok()
}

/// One unit of background work.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Job {
    /// Which repositories does this workspace root hold? (See [`discover`].)
    Discover(PathBuf),
    /// `git status` of the repository whose working folder this is.
    Status(PathBuf),
}

/// Most jobs (each runs one `git` at a time) that run at once.
const MAX_RUNNING: usize = 2;
/// A root with more repositories directly below it than this shows the first ones only.
const MAX_REPOS: usize = 64;

/// The repository folders of a workspace root: the root itself when it lies inside a
/// repository, else every direct subfolder that has a `.git` (a folder of sites). Blocking:
/// stats files, call from a worker. Sorted; at most [`MAX_REPOS`].
pub fn discover(root: &Path) -> Vec<PathBuf> {
    if root.ancestors().any(|a| a.join(".git").exists()) {
        return vec![root.to_path_buf()];
    }
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_name() != ".git")
        .map(|e| e.path())
        .filter(|p| p.join(".git").exists())
        .collect();
    dirs.sort();
    dirs.truncate(MAX_REPOS);
    dirs
}

/// [`discover`] on a worker thread with a deadline (empty: nothing found, or no answer).
pub async fn discover_bounded(root: PathBuf) -> Vec<PathBuf> {
    let job = crate::fsx::off_thread(move || discover(&root));
    crate::fsx::timeout(DEADLINE, job).await.flatten().unwrap_or_default()
}

/// The status of every repository below the workspace roots, and the bookkeeping that keeps
/// the `git` processes few: a job that is already queued is not queued twice, one that is
/// running when it is asked for again is repeated once afterwards, and at most
/// [`MAX_RUNNING`] run at the same time.
#[derive(Default)]
pub struct Tree {
    /// Repository folder -> its status.
    units: HashMap<PathBuf, Arc<Fetched>>,
    /// Workspace root -> its repository folders, as of the last [`discover`].
    found: HashMap<PathBuf, Vec<PathBuf>>,
    queue: VecDeque<Job>,
    running: HashSet<Job>,
    /// Jobs asked for while they ran: they need another run afterwards.
    again: HashSet<Job>,
}

impl Tree {
    /// The change of a path in the sidebar, whichever repository it is in.
    pub fn change(&self, path: &Path) -> Option<Change> {
        if let Some(unit) = self.units.get(path) {
            // The folder of a repository shows the strongest change inside it, except when it
            // is the workspace root itself, which stays plain.
            return if self.found.contains_key(path) { None } else { unit.map.top };
        }
        self.units.iter().filter(|(dir, _)| path.starts_with(dir)).find_map(|(_, f)| f.map.get(path))
    }

    /// Asks for a job. It runs when [`Tree::start`] hands it out.
    pub fn request(&mut self, job: Job) {
        if self.running.contains(&job) {
            self.again.insert(job);
        } else if !self.queue.contains(&job) {
            self.queue.push_back(job);
        }
    }

    /// The jobs that may start now.
    pub fn start(&mut self) -> Vec<Job> {
        let mut out = Vec::new();
        while self.running.len() < MAX_RUNNING {
            let Some(job) = self.queue.pop_front() else { break };
            self.running.insert(job.clone());
            out.push(job);
        }
        out
    }

    /// A job ended. `true`: it was asked for again meanwhile.
    pub fn done(&mut self, job: &Job) -> bool {
        self.running.remove(job);
        self.again.remove(job)
    }

    /// The repositories found below `root`. Returns the ones that have no status yet.
    pub fn set_found(&mut self, root: &Path, dirs: Vec<PathBuf>) -> Vec<PathBuf> {
        if let Some(old) = self.found.insert(root.to_path_buf(), dirs.clone()) {
            for gone in old.iter().filter(|d| !dirs.contains(d)) {
                self.units.remove(gone);
            }
        }
        dirs.into_iter().filter(|d| !self.units.contains_key(d)).collect()
    }

    /// The status of one repository (`None`: it is not one, or git did not answer).
    pub fn set_status(&mut self, dir: &Path, result: Option<Arc<Fetched>>) {
        match result {
            Some(f) => self.units.insert(dir.to_path_buf(), f),
            None => self.units.remove(dir),
        };
    }

    /// A root left the workspace.
    pub fn forget(&mut self, root: &Path) {
        for dir in self.found.remove(root).unwrap_or_default() {
            self.units.remove(&dir);
        }
    }

    /// What has to run because `path` changed below the workspace root `root`.
    pub fn jobs_for_change(&self, root: &Path, path: &Path) -> Vec<Job> {
        match self.found.get(root) {
            // Never looked yet.
            None => vec![Job::Discover(root.to_path_buf())],
            Some(dirs) => match dirs.iter().find(|d| path.starts_with(d)) {
                Some(dir) => vec![Job::Status(dir.clone())],
                // Something appeared or vanished next to the repositories: a new site, maybe.
                None if path.parent() == Some(root) => vec![Job::Discover(root.to_path_buf())],
                None => Vec::new(),
            },
        }
    }

    /// The repositories of `root` (empty until the first [`discover`] ended).
    pub fn repos_of(&self, root: &Path) -> &[PathBuf] {
        self.found.get(root).map_or(&[], Vec::as_slice)
    }

    /// The repositories' own directories: watched so a commit made elsewhere shows up.
    pub fn git_dirs(&self) -> impl Iterator<Item = &Path> {
        self.units.values().map(|f| f.git_dir.as_path())
    }

    /// The repository (its working folder) whose own directory holds `path` (a file git
    /// rewrote), but only for files that mean "the index or a branch moved": not the lock
    /// files git itself creates.
    pub fn repo_event(&self, path: &Path) -> Option<&Path> {
        let name = path.file_name()?.to_str()?;
        if name.ends_with(".lock") {
            return None;
        }
        let parent = path.parent()?;
        self.units.iter().find(|(_, f)| f.git_dir == parent).map(|(dir, _)| dir.as_path())
    }

    /// Does `path` lie inside one of the repositories' own directories?
    pub fn in_repo_dir(&self, path: &Path) -> bool {
        self.units.values().any(|f| path.starts_with(&f.git_dir))
    }

    #[cfg(test)]
    pub fn has_status(&self, dir: &Path) -> bool {
        self.units.contains_key(dir)
    }

    #[cfg(test)]
    pub fn pending(&self) -> Vec<Job> {
        self.queue.iter().chain(self.running.iter()).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Change::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn a_status_listing_becomes_colors_for_files_and_the_folders_above_them() {
        let root = p("/r");
        let out = b" M src/a.rs\0A  src/new/b.rs\0?? notes.txt\0?? scratch/\0 D src/gone.rs\0UU merge.rs\0";
        let m = parse(&root, "", out);

        assert_eq!(m.get(&p("/r/src/a.rs")), Some(Modified));
        assert_eq!(m.get(&p("/r/src/new/b.rs")), Some(Added));
        assert_eq!(m.get(&p("/r/notes.txt")), Some(Untracked));
        assert_eq!(m.get(&p("/r/merge.rs")), Some(Conflict));
        assert_eq!(m.get(&p("/r/clean.rs")), None);
        assert_eq!(m.get(&p("/r/src/gone.rs")), None, "a deleted file is not in the tree");

        // A folder shows the strongest change inside it; the root itself stays plain.
        assert_eq!(m.get(&p("/r/src")), Some(Modified));
        assert_eq!(m.get(&p("/r/src/new")), Some(Added));
        assert_eq!(m.get(&p("/r")), None);

        // A folder that git lists as one untracked unit makes everything in it untracked.
        assert_eq!(m.get(&p("/r/scratch")), Some(Untracked));
        assert_eq!(m.get(&p("/r/scratch/deep/x.txt")), Some(Untracked));
    }

    #[test]
    fn deleting_a_file_marks_its_folder_as_modified() {
        let m = parse(&p("/r"), "", b" D src/lib/gone.rs\0D  top.rs\0");
        assert_eq!(m.get(&p("/r/src/lib")), Some(Modified));
        assert_eq!(m.get(&p("/r/src")), Some(Modified));
        assert_eq!(m.get(&p("/r/src/lib/gone.rs")), None);
    }

    #[test]
    fn a_root_below_the_top_of_the_repository_loses_the_prefix_and_ignores_the_rest() {
        // Opened folder: <top>/app/. Paths come relative to <top>.
        let out = b" M app/x.rs\0?? app/sub/y.rs\0 M other/z.rs\0";
        let m = parse(&p("/top/app"), "app/", out);
        assert_eq!(m.get(&p("/top/app/x.rs")), Some(Modified));
        assert_eq!(m.get(&p("/top/app/sub/y.rs")), Some(Untracked));
        assert_eq!(m.get(&p("/top/app/sub")), Some(Untracked));
        assert_eq!(m.get(&p("/top/other/z.rs")), None);
        assert_eq!(m.get(&p("/top/app/other/z.rs")), None);
    }

    #[test]
    fn what_a_folder_shows_is_the_strongest_change_in_it() {
        let m = parse(&p("/r"), "", b"?? d/new.txt\0 M d/old.txt\0UU d/clash.txt\0");
        assert_eq!(m.get(&p("/r/d")), Some(Conflict));
        let m = parse(&p("/r"), "", b"?? d/new.txt\0A  d/staged.txt\0");
        assert_eq!(m.get(&p("/r/d")), Some(Added));
        let m = parse(&p("/r"), "", b"?? d/new.txt\0");
        assert_eq!(m.get(&p("/r/d")), Some(Untracked));
        assert!(Conflict > Modified && Modified > Added && Added > Untracked);
    }

    #[test]
    fn renames_and_junk_do_not_confuse_the_parser() {
        // "R  new\0old\0" must not make "old" a record of its own.
        let m = parse(&p("/r"), "", b"R  new.rs\0old.rs\0 M kept.rs\0\0x\0");
        assert_eq!(m.get(&p("/r/kept.rs")), Some(Modified));
        assert_eq!(m.get(&p("/r/old.rs")), None);
        assert!(parse(&p("/r"), "", b"").is_empty());
    }

    fn fetched(git_dir: &str, top: Option<Change>) -> Arc<Fetched> {
        Arc::new(Fetched { map: StatusMap { top, ..StatusMap::default() }, git_dir: p(git_dir) })
    }

    #[test]
    fn jobs_are_queued_once_run_two_at_a_time_and_repeat_when_asked_for_again_while_running() {
        use Job::*;
        let mut t = Tree::default();
        let (a, b, c) = (Status(p("/r/a")), Status(p("/r/b")), Status(p("/r/c")));
        t.request(a.clone());
        t.request(a.clone());
        t.request(b.clone());
        t.request(c.clone());
        assert_eq!(t.start(), vec![a.clone(), b.clone()], "two at a time, each once, in order");
        assert!(t.start().is_empty(), "a third waits");

        t.request(a.clone()); // asked for while it runs
        assert!(t.start().is_empty());
        assert!(t.done(&a), "...so it is wanted again");
        assert_eq!(t.start(), vec![c.clone()], "the freed place goes to the one waiting");
        assert!(!t.done(&b) && !t.done(&c), "nobody asked for those again");
        t.request(a.clone());
        assert_eq!(t.start(), vec![a], "and the repeat is an ordinary job");
    }

    #[test]
    fn a_folder_of_sites_has_one_repository_per_subfolder_and_a_repository_is_one_by_itself() {
        let base = std::env::temp_dir().join(format!("veloedit-discover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let www = base.join("www");
        for site in ["b.loc", "a.loc", "plain", "gitfile.loc"] {
            std::fs::create_dir_all(www.join(site)).unwrap();
        }
        std::fs::create_dir_all(www.join("a.loc/.git")).unwrap();
        std::fs::create_dir_all(www.join("b.loc/.git")).unwrap();
        std::fs::write(www.join("gitfile.loc/.git"), "gitdir: elsewhere\n").unwrap(); // a worktree
        std::fs::write(www.join("loose.txt"), "x").unwrap();
        // Only direct subfolders count, not deeper ones.
        std::fs::create_dir_all(www.join("plain/deeper/.git")).unwrap();

        assert_eq!(discover(&www), vec![www.join("a.loc"), www.join("b.loc"), www.join("gitfile.loc")]);
        // Inside a repository (any folder of it, or itself): that repository, nothing else.
        assert_eq!(discover(&www.join("a.loc")), vec![www.join("a.loc")]);
        std::fs::create_dir_all(www.join("a.loc/src/deep")).unwrap();
        assert_eq!(discover(&www.join("a.loc/src/deep")), vec![www.join("a.loc/src/deep")]);
        // One level is counted from the root that is asked about.
        assert_eq!(discover(&www.join("plain")), vec![www.join("plain/deeper")]);
        std::fs::create_dir_all(www.join("empty")).unwrap();
        assert!(discover(&www.join("empty")).is_empty(), "no repository directly below it");
        assert!(discover(&www.join("missing")).is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn colors_come_from_the_repository_a_path_is_in_and_a_site_folder_shows_its_strongest_change() {
        let mut t = Tree::default();
        let root = p("/www");
        assert_eq!(t.set_found(&root, vec![p("/www/a"), p("/www/b")]), vec![p("/www/a"), p("/www/b")]);
        let mut map_a = StatusMap::default();
        map_a.note(&p("/www/a"), p("/www/a/src/x.php"), Change::Modified);
        t.set_status(&p("/www/a"), Some(Arc::new(Fetched { map: map_a, git_dir: p("/www/a/.git") })));
        t.set_status(&p("/www/b"), Some(fetched("/www/b/.git", None)));

        assert_eq!(t.change(&p("/www/a/src/x.php")), Some(Change::Modified));
        assert_eq!(t.change(&p("/www/a/src")), Some(Change::Modified));
        assert_eq!(t.change(&p("/www/a")), Some(Change::Modified), "the site's own folder row");
        assert_eq!(t.change(&p("/www/b")), None, "a clean site stays plain");
        assert_eq!(t.change(&p("/www/b/src/x.php")), None);
        assert_eq!(t.change(&p("/www")), None);

        // A workspace root that is itself a repository stays plain, whatever is inside it.
        let solo = p("/solo");
        t.set_found(&solo, vec![solo.clone()]);
        let mut map = StatusMap::default();
        map.note(&solo, p("/solo/y.rs"), Change::Untracked);
        t.set_status(&solo, Some(Arc::new(Fetched { map, git_dir: p("/solo/.git") })));
        assert_eq!(t.change(&solo), None);
        assert_eq!(t.change(&p("/solo/y.rs")), Some(Change::Untracked));

        // A site that vanished takes its colors with it; closing a root drops all of its own.
        assert_eq!(t.set_found(&root, vec![p("/www/b")]), Vec::<PathBuf>::new());
        assert_eq!(t.change(&p("/www/a/src/x.php")), None);
        t.forget(&solo);
        assert_eq!(t.change(&p("/solo/y.rs")), None);
    }

    #[test]
    fn a_change_asks_only_for_the_repository_it_is_in() {
        use Job::*;
        let mut t = Tree::default();
        let root = p("/www");
        // Not looked at yet: find the repositories first, whatever changed.
        assert_eq!(t.jobs_for_change(&root, &p("/www/a/x.php")), vec![Discover(root.clone())]);
        t.set_found(&root, vec![p("/www/a"), p("/www/b")]);
        assert_eq!(t.jobs_for_change(&root, &p("/www/a/src/x.php")), vec![Status(p("/www/a"))]);
        assert_eq!(t.jobs_for_change(&root, &p("/www/b")), vec![Status(p("/www/b"))]);
        // A new folder next to the sites: maybe a clone finished.
        assert_eq!(t.jobs_for_change(&root, &p("/www/new.loc")), vec![Discover(root.clone())]);
        // A file deep in a folder that is no repository: nothing to ask.
        assert!(t.jobs_for_change(&root, &p("/www/plain/deep/x.txt")).is_empty());
        assert_eq!(t.repos_of(&root), &[p("/www/a"), p("/www/b")]);
    }

    #[test]
    fn commits_made_elsewhere_are_recognised_but_lock_files_are_not() {
        let mut t = Tree::default();
        let (www, a, b) = (p("/www"), p("/www/a"), p("/www/b"));
        t.set_found(&www, vec![a.clone(), b.clone()]);
        t.set_status(&a, Some(fetched("/www/a/.git", None)));
        t.set_status(&b, Some(fetched("/www/b/.git", None)));
        assert_eq!(t.repo_event(&p("/www/a/.git/index")), Some(a.as_path()), "which repository moved");
        assert_eq!(t.repo_event(&p("/www/b/.git/HEAD")), Some(b.as_path()));
        assert_eq!(t.repo_event(&p("/www/a/.git/index.lock")), None);
        assert_eq!(t.repo_event(&p("/www/a/src/index")), None);
        assert!(t.in_repo_dir(&p("/www/a/.git/objects/ab")) && !t.in_repo_dir(&p("/www/a/src")));
        assert_eq!(t.git_dirs().count(), 2);
        t.forget(&www);
        assert!(!t.has_status(&a) && t.change(&p("/www/a/x")).is_none() && t.git_dirs().count() == 0);
    }

    #[test]
    fn a_commit_made_in_a_terminal_is_seen_in_the_git_folder_by_the_watcher() {
        use iced::futures::StreamExt;
        if git().arg("--version").stdout(Stdio::null()).status().map_or(true, |s| !s.success()) {
            return; // no git here
        }
        let root = std::env::temp_dir().join(format!("veloedit-status-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let sh = |args: &[&str]| {
            let ok = git()
                .arg("-C")
                .arg(&root)
                .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        sh(&["init", "-q"]);
        std::fs::write(root.join("a.txt"), "1\n").unwrap();
        sh(&["add", "."]);
        sh(&["commit", "-q", "-m", "first"]);
        let git_dir = fetch(&root).unwrap().git_dir;
        let tree = {
            let mut t = Tree::default();
            t.set_found(&root, vec![root.clone()]);
            t.set_status(&root, Some(Arc::new(Fetched { map: StatusMap::default(), git_dir: git_dir.clone() })));
            t
        };

        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        rt.block_on(async {
            let mut events = Box::pin(crate::watcher::events(vec![git_dir.clone()]));
            // (The watcher is registered by the first poll.)
            assert!(tokio::time::timeout(Duration::from_millis(300), events.next()).await.is_err());

            // Does the app hear about each of these, as an event it takes for "the repository moved"?
            async fn heard(
                events: &mut (impl iced::futures::Stream<Item = Vec<PathBuf>> + Unpin),
                tree: &Tree,
                root: &Path,
                what: &str,
            ) {
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while std::time::Instant::now() < deadline {
                    if let Ok(Some(batch)) = tokio::time::timeout(Duration::from_millis(500), events.next()).await {
                        if batch.iter().any(|p| tree.repo_event(p) == Some(root)) {
                            return;
                        }
                    }
                }
                panic!("nothing told the app about: {what}");
            }
            std::fs::write(root.join("a.txt"), "2\n").unwrap();
            sh(&["add", "a.txt"]);
            heard(&mut events, &tree, &root, "git add").await;
            sh(&["commit", "-q", "-m", "second"]);
            heard(&mut events, &tree, &root, "git commit").await;
            sh(&["checkout", "-q", "-b", "other"]);
            heard(&mut events, &tree, &root, "git checkout -b").await;
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_real_git_agrees() {
        if git().arg("--version").stdout(Stdio::null()).status().map_or(true, |s| !s.success()) {
            return; // no git here
        }
        let root = std::env::temp_dir().join(format!("veloedit-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        let sh = |args: &[&str]| {
            let ok = git()
                .arg("-C")
                .arg(&root)
                .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        sh(&["init", "-q"]);
        std::fs::write(root.join("src/a.rs"), "1\n").unwrap();
        std::fs::write(root.join("src/b.rs"), "1\n").unwrap();
        std::fs::write(root.join("keep.txt"), "1\n").unwrap();
        sh(&["add", "."]);
        sh(&["commit", "-q", "-m", "first"]);
        assert!(fetch(&root).unwrap().map.is_empty(), "a clean checkout has no colors");

        std::fs::write(root.join("src/a.rs"), "2\n").unwrap(); // modified
        std::fs::remove_file(root.join("src/b.rs")).unwrap(); // deleted
        std::fs::write(root.join("fresh.txt"), "new\n").unwrap(); // untracked
        std::fs::create_dir_all(root.join("tmp/deep")).unwrap();
        std::fs::write(root.join("tmp/deep/x"), "x").unwrap(); // untracked folder
        std::fs::write(root.join("staged.txt"), "s\n").unwrap();
        sh(&["add", "staged.txt"]); // added

        let got = fetch(&root).unwrap();
        let m = &got.map;
        assert_eq!(m.get(&root.join("src/a.rs")), Some(Modified));
        assert_eq!(m.get(&root.join("src")), Some(Modified));
        assert_eq!(m.get(&root.join("fresh.txt")), Some(Untracked));
        assert_eq!(m.get(&root.join("tmp/deep/x")), Some(Untracked));
        assert_eq!(m.get(&root.join("staged.txt")), Some(Added));
        assert_eq!(m.get(&root.join("keep.txt")), None);
        assert!(got.git_dir.ends_with(".git"), "{:?}", got.git_dir);

        // A folder opened inside the repository only sees its own part.
        let inner = fetch(&root.join("src")).unwrap();
        assert_eq!(inner.map.get(&root.join("src/a.rs")), Some(Modified));
        assert_eq!(inner.map.get(&root.join("fresh.txt")), None);

        // Not a repository: no map, no error.
        let plain = std::env::temp_dir().join(format!("veloedit-status-plain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&plain);
        std::fs::create_dir_all(&plain).unwrap();
        if !plain.ancestors().any(|a| a.join(".git").exists()) {
            assert!(fetch(&plain).is_none());
        }
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&plain);
    }
}
