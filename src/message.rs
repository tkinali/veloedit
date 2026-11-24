//! Every event the app can react to (Elm-style architecture).

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use iced::keyboard;

use crate::{
    git::{self, GitMarks},
    palette::{IndexedFile, Mode},
    syntax::{Lang, LineSpans, ParseState, Syntax},
};

use crate::{
    editor::{TabId, loader::Loaded, state::Eol},
    workspace::Entry,
};

/// iced requires `Message: Clone`. Large, non-`Clone` payloads (open files,
/// decoded images) travel in a take-once box instead.
pub struct Handoff<T>(Arc<Mutex<Option<T>>>);

impl<T> Handoff<T> {
    pub fn new(value: T) -> Self {
        Self(Arc::new(Mutex::new(Some(value))))
    }

    /// Returns the payload the first time it is called, `None` afterwards.
    pub fn take(&self) -> Option<T> {
        self.0.lock().ok()?.take()
    }
}

impl<T> Clone for Handoff<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

#[derive(Clone)]
pub enum Message {
    // --- window / chrome ---
    ToggleSidebar,
    Key(keyboard::Event),
    Tick,
    CloseRequested,
    Exit,
    Noop,

    // --- find / replace ---
    OpenFind { replace: bool },
    CloseFind,
    FindQuery(String),
    FindReplacement(String),
    FindToggle(FindFlag),
    FindNext,
    FindPrev,
    FindSubmit,
    FindResults { key: crate::find::Key, matches: Arc<Vec<(usize, usize)>>, error: Option<String> },
    ReplaceOne,
    ReplaceAll,
    Replaced { id: TabId, version: u64, edit: Option<crate::search::Edit>, count: usize, then_next: bool },

    // --- palette / commands ---
    OpenPalette(Mode),
    ClosePalette,
    PaletteQuery(String),
    PaletteMove(isize),
    PaletteToggle(FindFlag),
    PaletteSubmit,
    PaletteClick(usize),
    PaletteResults { seq: u64, hits: Vec<usize> },
    /// Find in Files: the "Files:" filter text, the folder scope, results list scrolling.
    SearchFilter(String),
    SearchScope(Option<PathBuf>),
    PickSearchScope,
    SearchScrolled { y: f32, height: f32 },
    /// Tab / Shift+Tab inside Find in Files: switch between its two inputs.
    SearchTab(bool),
    /// Open Find in Files restricted to a folder (sidebar context menu).
    FindInFolder(PathBuf),
    IndexBuilt(Arc<Vec<IndexedFile>>),
    SearchResults { seq: u64, outcome: Arc<crate::project_search::Outcome> },
    /// Open a file and select a range in it (search hit).
    OpenAt { path: PathBuf, line: usize, col: usize, len: usize },
    GotoLine(usize),
    OpenGoto,
    /// Esc: dismiss the topmost modal.
    Escape,
    ReloadActive,
    OpenSettings,
    SetTheme(String),
    /// View > Syntax: set (or, with `Auto`, give back) the active document's language.
    SetSyntax(Syntax),
    /// Opens the palette on the list of languages.
    OpenSyntaxPicker,
    /// View > Convert Indentation to Spaces / Tabs.
    ConvertIndent { spaces: bool },
    /// View > Line Endings.
    SetLineEnding(Eol),
    ChangeFontSize(i32),
    ChangeTabSize(i32),
    ToggleSetting(Toggle),
    OpenSettingsPanel,
    SettingsWritten(Result<(), String>),

    // --- git gutter ---
    GitBase { id: TabId, path: PathBuf, base: Option<Arc<String>> },
    GitMarksReady { id: TabId, marks: Arc<GitMarks> },
    RefreshGit,
    /// The repository folders below a workspace root were found.
    GitDiscovered { root: PathBuf, dirs: Vec<PathBuf> },
    /// `git status` of one repository arrived (`None`: not a repository, or no answer).
    GitStatus { dir: PathBuf, result: Option<Arc<git::status::Fetched>> },
    /// The pause before a repeated git job is over.
    GitDue(git::status::Job),

    // --- syntax ---
    HighlightTick,
    Highlighted {
        id: TabId,
        lang: Lang,
        /// `None`: the grammar could not be loaded.
        result: Option<(ParseState, Arc<LineSpans>)>,
    },

    // --- disk ---
    FsChanged(Vec<PathBuf>),
    DiskChanged {
        id: TabId,
        /// Reload even if the buffer has unsaved edits.
        force: bool,
        result: Result<Handoff<Loaded>, String>,
    },
    ReloadFromDisk(TabId),
    ConflictChoice(TabId, bool, ConflictChoice),

    // --- workspace / sidebar ---
    PickFolder,
    FolderPicked(Option<PathBuf>),
    /// Ctrl+O: choose any files (no project needed).
    PickFiles,
    FilesPicked(Vec<PathBuf>),
    /// A file or folder was dropped on the window.
    Dropped(PathBuf),
    /// Another `veloedit` launch handed us these paths (single window).
    External(Vec<PathBuf>),
    /// The single-window socket asks "is your UI thread alive?" after it passed on paths;
    /// answering lets the launch that handed them over exit (see `instance.rs`).
    Alive(Handoff<iced::futures::channel::oneshot::Sender<()>>),
    RemoveRoot(PathBuf),
    TreeClicked { path: PathBuf, is_dir: bool },
    /// Open a file in a tab of its own (and pin it if it was a preview).
    OpenPinned(PathBuf),
    DirScanned { path: PathBuf, entries: Vec<Entry> },
    SidebarScrolled { y: f32, height: f32 },
    /// The window was resized (also reported once at start-up).
    WindowResized(iced::Size),
    /// Result of asking whether the window is maximized (with the size it had when asked).
    WindowMaximized { size: iced::Size, maximized: bool },
    WindowMoved(iced::Point),
    MonitorSize(Option<iced::Size>),
    ResetFontSize,
    PreviewResizeStart,
    PreviewResizeTo(f32),
    PreviewResizeEnd,
    SidebarResizeStart,
    SidebarResizeTo(f32),
    SidebarResizeEnd,

    // --- documents ---
    Opened(Result<Handoff<Loaded>, String>),
    /// A file opened by a single sidebar click: it becomes (or replaces) the preview tab.
    OpenedPreview(Result<Handoff<Loaded>, String>),
    Restored(Handoff<Vec<Loaded>>, Option<usize>),
    NewFile,
    ActivateTab(TabId),
    /// The tab strip's viewport changed (scrolled, resized, tabs added).
    TabsViewport { x: f32, width: f32, content: f32 },
    /// Mouse wheel over the tab strip, in pixels.
    TabsWheel(f32),
    /// The ▾ button: list of all open tabs.
    OpenTabList,
    About,
    NextTab,
    PrevTab,
    CloseTab(TabId),
    CloseActive,
    /// Right click on a tab.
    TabContext(TabId),
    CloseOthers(TabId),
    CloseRight(TabId),
    CloseAll,
    CloseChoice(TabId, CloseChoice),

    // --- text ---
    Editor(TabId, crate::editor::state::Action),
    /// Input from one pane of a split tab (focuses that pane first, except for scrolling).
    EditorPane(TabId, crate::editor::Pane, crate::editor::state::Action),
    SplitTab(TabId, crate::editor::SplitDir),
    CloseSplit(TabId),
    /// Split / unsplit the active tab (menu, palette).
    SplitActive(crate::editor::SplitDir),
    CloseSplitActive,
    /// Mouse down on a tab: activates it and may start dragging it to a new place.
    TabPressed(TabId),
    TabDragTo(iced::Point),
    TabDragEnd,
    /// A click on a pane's name bar / its ×.
    FocusPane(TabId, crate::editor::Pane),
    ClosePane(TabId, crate::editor::Pane),
    SplitResizeStart,
    SplitResizeTo(iced::Point),
    SplitResizeEnd,
    SaveActive,
    SaveTab { id: TabId, then_close: bool, force: bool },
    SavePicked { id: TabId, path: Option<PathBuf>, then_close: bool },
    Saved {
        id: TabId,
        path: PathBuf,
        version: u64,
        then_close: bool,
        /// Hash of the bytes written.
        result: Result<u64, String>,
    },

    // --- menus / context menu / file operations ---
    /// Right click on a sidebar row.
    TreeContext { path: PathBuf, is_dir: bool, is_root: bool },
    /// Right click in the text editor.
    EditorContext,
    OpenMenu(usize),
    MenuHover(usize),
    CloseMenu,
    /// A menu entry was chosen: close the menu, then run the message.
    MenuPick(Box<Message>),
    NewFileIn(PathBuf),
    NewFolderIn(PathBuf),
    RenameEntry(PathBuf),
    TrashEntry(PathBuf),
    PromptInput(String),
    PromptSubmit,
    FileOp(Result<crate::fileops::Done, String>),
    CopyText(String),
    Reveal(PathBuf),
    RefreshDir(PathBuf),
    /// Edit menu.
    EditAction(crate::editor::state::Action),
    CopySelection,
    CutSelection,
    PasteRequest,
    PasteText(String),
    Quit,
    /// A link clicked in the Markdown preview.
    OpenLink(String),

    // --- hex ---
    Hex(TabId, crate::editor::hex::HexAction),
    HexSaved { id: TabId, saved: Arc<Vec<(u64, u8)>>, then_close: bool, result: Result<(), String> },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CloseChoice {
    Save,
    Discard,
    Cancel,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    WordWrap,
    Minimap,
    MenuBar,
    MarkdownPreview,
    /// Remove trailing spaces and tabs when saving.
    TrimOnSave,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FindFlag {
    Case,
    Word,
    Regex,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    Overwrite,
    Reload,
    Cancel,
}
