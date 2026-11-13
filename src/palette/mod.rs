//! Command palette / quick open: state, ranking, and the command list.
//! Rendering lives in `ui::palette`.

pub mod index;

pub use index::IndexedFile;

use std::path::PathBuf;

use nucleo_matcher::{
    Config, Matcher,
    pattern::{CaseMatching, Normalization, Pattern},
};

use crate::ui::theme;

/// Rows shown at once.
pub const VISIBLE: usize = 12;
/// Height assumed for the results list until its first scroll event reports the real one.
pub const SEARCH_DEFAULT_H: f32 = 2400.0;
/// Max results kept per query.
pub const LIMIT: usize = 50;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Ctrl+P: fuzzy-open a file from the workspace.
    Files,
    /// Ctrl+Shift+P: run a command.
    Commands,
    /// Ctrl+Shift+F: search the text of all workspace files.
    Search,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Command {
    GoToFile,
    GoToLine,
    FindInFiles,
    ToggleSidebar,
    ToggleMarkdownPreview,
    ToggleTrimOnSave,
    Fold,
    Unfold,
    FoldAll,
    UnfoldAll,
    FoldLevel(usize),
    NewFile,
    Save,
    CloseTab,
    OpenFile,
    AddFolder,
    ReloadFromDisk,
    NextTab,
    PrevTab,
    SplitRight,
    SplitDown,
    CloseSplit,
    FontBigger,
    FontSmaller,
    FontReset,
    OpenSettings,
    SettingsPanel,
    RefreshGit,
    Find,
    Replace,
    FindNext,
    FindPrev,
    SetTheme(&'static str),
}

impl Command {
    pub fn all() -> Vec<Command> {
        let mut v = vec![
            Command::GoToFile,
            Command::GoToLine,
            Command::FindInFiles,
            Command::Find,
            Command::Replace,
            Command::FindNext,
            Command::FindPrev,
            Command::ToggleSidebar,
            Command::ToggleMarkdownPreview,
            Command::ToggleTrimOnSave,
            Command::Fold,
            Command::Unfold,
            Command::FoldAll,
            Command::UnfoldAll,
            Command::FoldLevel(1),
            Command::FoldLevel(2),
            Command::FoldLevel(3),
            Command::NewFile,
            Command::Save,
            Command::CloseTab,
            Command::OpenFile,
            Command::AddFolder,
            Command::ReloadFromDisk,
            Command::NextTab,
            Command::PrevTab,
            Command::SplitRight,
            Command::SplitDown,
            Command::CloseSplit,
            Command::FontBigger,
            Command::FontSmaller,
            Command::FontReset,
            Command::SettingsPanel,
            Command::OpenSettings,
            Command::RefreshGit,
        ];
        v.extend(theme::ALL.iter().map(|c| Command::SetTheme(c.name)));
        v
    }

    pub fn title(self) -> String {
        match self {
            Command::GoToFile => "Go to File...".into(),
            Command::GoToLine => "Go to Line...".into(),
            Command::FindInFiles => "Find: Find in Files...".into(),
            Command::Find => "Find: Find...".into(),
            Command::Replace => "Find: Replace...".into(),
            Command::FindNext => "Find: Find Next".into(),
            Command::FindPrev => "Find: Find Previous".into(),
            Command::ToggleSidebar => "View: Toggle Sidebar".into(),
            Command::ToggleMarkdownPreview => "View: Toggle Markdown Preview".into(),
            Command::ToggleTrimOnSave => "Preferences: Toggle Trim Trailing Whitespace on Save".into(),
            Command::Fold => "Fold: Fold Code Block".into(),
            Command::Unfold => "Fold: Unfold Code Block".into(),
            Command::FoldAll => "Fold: Fold All".into(),
            Command::UnfoldAll => "Fold: Unfold All".into(),
            Command::FoldLevel(n) => format!("Fold: Fold Level {n}"),
            Command::NewFile => "File: New File".into(),
            Command::Save => "File: Save".into(),
            Command::CloseTab => "File: Close Tab".into(),
            Command::OpenFile => "File: Open File...".into(),
            Command::AddFolder => "File: Add Folder to Workspace...".into(),
            Command::ReloadFromDisk => "File: Reload from Disk".into(),
            Command::NextTab => "View: Next Tab".into(),
            Command::PrevTab => "View: Previous Tab".into(),
            Command::SplitRight => "View: Split Right".into(),
            Command::SplitDown => "View: Split Down".into(),
            Command::CloseSplit => "View: Close Split".into(),
            Command::FontBigger => "View: Increase Font Size".into(),
            Command::FontSmaller => "View: Decrease Font Size".into(),
            Command::FontReset => "View: Reset Font Size".into(),
            Command::OpenSettings => "Preferences: Open settings.json".into(),
            Command::SettingsPanel => "Preferences: Settings...".into(),
            Command::RefreshGit => "Git: Refresh Gutter".into(),
            Command::SetTheme(name) => {
                let mut cs = name.chars();
                let first = cs.next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                format!("Theme: {first}{}", cs.as_str())
            }
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Command::GoToFile => "Ctrl+P",
            Command::SettingsPanel => "Ctrl+,",
            Command::GoToLine => "Ctrl+G",
            Command::FindInFiles => "Ctrl+Shift+F",
            Command::Find => "Ctrl+F",
            Command::Replace => "Ctrl+H",
            Command::FindNext => "F3",
            Command::FindPrev => "Shift+F3",
            Command::ToggleSidebar => "Ctrl+B",
            Command::FontBigger => "Ctrl+=",
            Command::FontSmaller => "Ctrl+-",
            Command::FontReset => "Ctrl+0",
            Command::Fold => "Ctrl+Shift+[",
            Command::Unfold => "Ctrl+Shift+]",
            Command::NewFile => "Ctrl+N",
            Command::Save => "Ctrl+S",
            Command::CloseTab => "Ctrl+W",
            Command::OpenFile => "Ctrl+O",
            Command::AddFolder => "Ctrl+Shift+O",
            Command::NextTab => "Ctrl+Tab",
            Command::PrevTab => "Ctrl+Shift+Tab",
            _ => "",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Target {
    File(PathBuf),
    Command(Command),
    /// A search hit: open `path` and select `len` chars at `line`:`col`.
    Location { path: PathBuf, line: usize, col: usize, len: usize },
    /// `:42` in the file palette: jump to a line (1-based).
    Line(usize),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub title: String,
    pub detail: String,
    pub target: Target,
}

impl Item {
    pub fn file(f: &IndexedFile) -> Self {
        Item {
            title: f.file_name().to_owned(),
            detail: f.dir().to_owned(),
            target: Target::File(f.path.clone()),
        }
    }

    pub fn location(hit: &crate::project_search::Hit) -> Self {
        let label = shorten_left(&format!("{}:{}", hit.rel, hit.line + 1), 42);
        Item {
            title: hit.preview.clone(),
            detail: label,
            target: Target::Location { path: hit.path.clone(), line: hit.line, col: hit.col, len: hit.len },
        }
    }

    pub fn offset(n: usize) -> Self {
        Item { title: format!("Go to offset {n} (0x{n:X})"), detail: String::new(), target: Target::Line(n) }
    }

    pub fn line(n: usize) -> Self {
        Item { title: format!("Go to line {n}"), detail: String::new(), target: Target::Line(n) }
    }

    pub fn command(cmd: Command) -> Self {
        Item {
            title: cmd.title(),
            detail: cmd.hint().to_owned(),
            target: Target::Command(cmd),
        }
    }
}

/// One display row of the Find-in-Files results: a file header or one hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchRow {
    /// `(index of the file's first hit, number of hits)`.
    File(usize, usize),
    /// Index into `Palette::items` / `Outcome::hits`.
    Hit(usize),
}

pub struct Palette {
    pub mode: Mode,
    pub query: String,
    pub selected: usize,
    pub items: Vec<Item>,
    /// Search options (Search mode only).
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// `*.php, !vendor` (Search mode only).
    pub filter: String,
    /// Search only below this folder (`None` = every workspace folder).
    pub scope: Option<PathBuf>,
    /// Latest results, grouped per file in `rows`.
    pub results: Option<std::sync::Arc<crate::project_search::Outcome>>,
    pub rows: Vec<SearchRow>,
    /// Scroll offset / height of the results list (for virtualisation).
    pub scroll_y: f32,
    pub scroll_h: f32,
    /// One line under the input: "12 results in 3 files", errors, ...
    pub status: String,
}

impl Palette {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            query: String::new(),
            selected: 0,
            items: Vec::new(),
            case_sensitive: false,
            whole_word: false,
            regex: false,
            filter: String::new(),
            scope: None,
            results: None,
            rows: Vec::new(),
            scroll_y: 0.0,
            scroll_h: SEARCH_DEFAULT_H,
            status: String::new(),
        }
    }

    /// Installs a finished search: hits become selectable items, grouped per file.
    pub fn set_results(&mut self, outcome: std::sync::Arc<crate::project_search::Outcome>) {
        self.items = outcome.hits.iter().map(Item::location).collect();
        self.rows.clear();
        let mut i = 0;
        while i < outcome.hits.len() {
            let path = &outcome.hits[i].path;
            let n = outcome.hits[i..].iter().take_while(|h| &h.path == path).count();
            self.rows.push(SearchRow::File(i, n));
            self.rows.extend((i..i + n).map(SearchRow::Hit));
            i += n;
        }
        self.results = Some(outcome);
        self.selected = 0;
        self.scroll_y = 0.0;
    }

    /// Forgets results (a new query is running).
    pub fn clear_results(&mut self) {
        self.items.clear();
        self.rows.clear();
        self.results = None;
        self.selected = 0;
    }

    /// Display row of the selected hit.
    pub fn selected_row(&self) -> Option<usize> {
        self.rows.iter().position(|r| *r == SearchRow::Hit(self.selected))
    }

    /// The Search-mode query with its options.
    pub fn search_query(&self) -> crate::search::Query {
        crate::search::Query {
            text: self.query.clone(),
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            regex: self.regex,
        }
    }

    /// Moves the selection, wrapping around the ends.
    pub fn select(&mut self, delta: isize) {
        let n = self.items.len() as isize;
        if n == 0 {
            return;
        }
        let at_edge = (delta > 0 && self.selected as isize == n - 1) || (delta < 0 && self.selected == 0);
        self.selected = if at_edge && delta.abs() == 1 {
            (self.selected as isize + delta).rem_euclid(n) as usize
        } else {
            (self.selected as isize + delta).clamp(0, n - 1) as usize
        };
    }

    /// First row of the visible window: it follows the selection.
    pub fn window_start(&self) -> usize {
        self.selected.saturating_sub(VISIBLE - 1)
    }
}

/// Keeps the tail of a long label ("…/ui/palette.rs:12").
fn shorten_left(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_owned();
    }
    format!("\u{2026}{}", s.chars().skip(n - (max - 1)).collect::<String>())
}

struct Hay<'a>(usize, &'a str);

impl AsRef<str> for Hay<'_> {
    fn as_ref(&self) -> &str {
        self.1
    }
}

fn pattern(query: &str) -> Pattern {
    Pattern::parse(query, CaseMatching::Smart, Normalization::Smart)
}

/// Indices into `files` of the best matches, best first. Blocking.
pub fn rank_files(files: &[IndexedFile], query: &str, limit: usize) -> Vec<usize> {
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let hay = files.iter().enumerate().map(|(i, f)| Hay(i, f.rel.as_str()));
    let mut hits = pattern(query).match_list(hay, &mut matcher);
    // nucleo orders by score; prefer shorter paths among equals.
    hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| files[a.0.0].rel.len().cmp(&files[b.0.0].rel.len())));
    hits.into_iter().take(limit).map(|(h, _)| h.0).collect()
}

pub fn rank_commands(query: &str) -> Vec<Command> {
    let all = Command::all();
    if query.trim().is_empty() {
        return all;
    }
    let titles: Vec<String> = all.iter().map(|c| c.title()).collect();
    let mut matcher = Matcher::new(Config::DEFAULT);
    let hay = titles.iter().enumerate().map(|(i, t)| Hay(i, t.as_str()));
    pattern(query)
        .match_list(hay, &mut matcher)
        .into_iter()
        .map(|(h, _)| all[h.0])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[&str]) -> Vec<IndexedFile> {
        paths
            .iter()
            .map(|p| IndexedFile { path: PathBuf::from(format!("/r/{p}")), rel: (*p).to_owned() })
            .collect()
    }

    #[test]
    fn fuzzy_ranking_prefers_filename_hits_and_short_paths() {
        let f = files(&[
            "src/main.rs",
            "src/ui/sidebar.rs",
            "src/ui/status_bar.rs",
            "docs/maintenance-notes.md",
            "Cargo.toml",
        ]);
        let top = |q: &str| rank_files(&f, q, 5).first().map(|&i| f[i].rel.clone());
        assert_eq!(top("main.rs").as_deref(), Some("src/main.rs"));
        assert_eq!(top("sdbar").as_deref(), Some("src/ui/sidebar.rs"));
        assert_eq!(top("ctoml").as_deref(), Some("Cargo.toml"));
        assert!(rank_files(&f, "zzzz", 5).is_empty());
        // Smart case: an uppercase letter makes the match case-sensitive.
        assert_eq!(top("Cargo").as_deref(), Some("Cargo.toml"));
        assert!(rank_files(&f, "CARGO", 5).is_empty());
    }

    #[test]
    fn commands_match_loosely_and_selection_wraps() {
        assert_eq!(rank_commands("tgl sdbr").first().copied(), Some(Command::ToggleSidebar));
        assert!(rank_commands("dark").contains(&Command::SetTheme("dark")));
        assert!(rank_commands("").len() >= 12);

        let mut p = Palette::new(Mode::Commands);
        p.items = (0..3).map(|_| Item::command(Command::Save)).collect();
        p.select(-1);
        assert_eq!(p.selected, 2);
        p.select(1);
        assert_eq!(p.selected, 0);
        p.select(8);
        assert_eq!(p.selected, 2, "big jumps clamp instead of wrapping");
    }

    #[test]
    fn index_honors_gitignore_and_names_roots_when_several() {
        let dir = std::env::temp_dir().join(format!("veloedit-idx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = (dir.join("alpha"), dir.join("beta"));
        std::fs::create_dir_all(a.join("src")).unwrap();
        std::fs::create_dir_all(a.join("target")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(a.join("src/lib.rs"), "").unwrap();
        std::fs::write(a.join("target/junk.o"), "").unwrap();
        std::fs::write(b.join("lib.rs"), "").unwrap();

        let one = index::build(std::slice::from_ref(&a));
        let rels: Vec<_> = one.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, vec![".gitignore", "src/lib.rs"]);

        let two = index::build(&[a, b]);
        let rels: Vec<_> = two.iter().map(|f| f.rel.as_str()).collect();
        assert!(rels.contains(&"alpha/src/lib.rs") && rels.contains(&"beta/lib.rs"), "{rels:?}");
        assert_eq!(two[0].file_name(), ".gitignore");
        std::fs::remove_dir_all(dir).ok();
    }
}
