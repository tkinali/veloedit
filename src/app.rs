//! Application root: state, `update` (all state transitions), `view`, subscriptions.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use iced::{
    Alignment, Background, Border, Element, Event, Length, Point, Subscription, Task, Theme, event, keyboard, mouse, time,
    widget::{self, button, column, container, mouse_area, opaque, row, scrollable, space, stack, text},
    window,
};

use crate::{
    editor::{
        Doc, LONG_LABEL, Pane, SPLIT_MAX, SPLIT_MIN, SplitDir, TabId, TabSet, TextDoc, hex, state::Action,
        loader::{self, Body, Loaded},
    },
    fileops,
    find::{self, Find},
    fsx,
    message::{CloseChoice, ConflictChoice, FindFlag, Handoff, Message, Toggle},
    palette::{self, Command, IndexedFile, Item, Mode, Palette, Target},
    project_search,
    search,
    settings::{self, Settings},
    git::{self, Base, GitState},
    syntax::{self, Lang},
    session::{self, SessionFile, Snapshot, TabKind, TabRecord},
    ui::{
        find_bar, hex_view, image_view,
        menu::{self, Entry},
        markdown_view, menu_bar, palette as palette_view, search_panel, settings_panel,
        sidebar::{self, Scroll},
        status_bar, tab_bar, text_view, theme,
    },
    watcher,
    workspace::{self, Workspace},
};

/// How often unsaved work / session state is flushed after a change.
const FLUSH_EVERY: Duration = Duration::from_millis(1500);

static START: OnceLock<Instant> = OnceLock::new();

pub fn init_timing() {
    let _ = START.set(Instant::now());
}

/// Modal questions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dialog {
    /// Closing a tab with unsaved edits.
    CloseDirty(TabId),
    /// Saving over a file that changed on disk meanwhile.
    Conflict { id: TabId, then_close: bool },
    /// Help > About.
    About,
}

/// A floating menu: the sidebar's context menu or one of the menu bar's drop-downs.
enum Popup {
    Context { at: Point, entries: Vec<Entry>, width: f32 },
    Menu(usize),
}

/// File operations that need a name or a confirmation.
#[derive(Clone)]
enum PromptKind {
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
    Trash(PathBuf),
}

struct Prompt {
    kind: PromptKind,
    text: String,
    error: Option<String>,
}

/// Bounds of the preview's share of the editor area.
const PREVIEW_MIN: f32 = 0.2;
const PREVIEW_MAX: f32 = 0.85;

const PROMPT_INPUT: &str = "veloedit-prompt-input";

/// Last mouse position (window coordinates), written by a subscription that
/// never produces messages - a right click needs to know where it happened,
/// and tracking moves through `update` would redraw on every pixel.
static POINTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn pointer() -> Point {
    let v = POINTER.load(std::sync::atomic::Ordering::Relaxed);
    Point::new(f32::from_bits((v >> 32) as u32), f32::from_bits(v as u32))
}

/// Whether the left mouse button is down (a drag that missed its release must not go on).
static LEFT_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn track_pointer(event: Event, _status: event::Status, _window: window::Id) -> Option<Message> {
    use std::sync::atomic::Ordering::Relaxed;
    match event {
        Event::Mouse(mouse::Event::CursorMoved { position }) => {
            let v = (u64::from(position.x.to_bits()) << 32) | u64::from(position.y.to_bits());
            POINTER.store(v, Relaxed);
        }
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => LEFT_DOWN.store(true, Relaxed),
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => LEFT_DOWN.store(false, Relaxed),
        _ => {}
    }
    None
}

/// A tab pressed with the mouse: it becomes a drag once the pointer has moved a little.
struct TabDrag {
    id: TabId,
    from: Point,
    live: bool,
    /// The gap the tab would be dropped into.
    slot: Option<usize>,
}

pub struct App {
    ws: Workspace,
    tabs: TabSet,
    sidebar_visible: bool,
    sidebar_width: f32,
    /// The divider is being dragged.
    resizing_sidebar: bool,
    /// Share of the editor area given to the Markdown preview, and its divider's drag state.
    preview_frac: f32,
    resizing_preview: bool,
    /// Dragging the divider between the two panes of the active tab: where the pointer was
    /// and the share the first pane had then.
    split_drag: Option<(Point, f32)>,
    tab_drag: Option<TabDrag>,
    /// The last click on a file in the sidebar / on a tab (a second one soon after pins the preview).
    last_tree_click: Option<(PathBuf, Instant)>,
    last_tab_press: Option<(TabId, Instant)>,
    /// Where the last file was picked or saved (file dialogs start there).
    last_dir: Option<PathBuf>,
    window_size: iced::Size,
    sidebar_scroll: Scroll,
    /// Modal question awaiting an answer.
    dialog: Option<Dialog>,
    notice: Option<String>,
    /// Lines whose trailing blanks the last save removed (tab, count): said in the notice.
    trimmed: Option<(TabId, usize)>,
    session_dirty: bool,
    /// Session flushes stay disabled until restore finished (else an empty
    /// in-memory state would overwrite the saved one).
    restored: bool,
    first_view_logged: std::cell::Cell<bool>,
    settings: Settings,
    palette: Option<Palette>,
    /// Bumped per query so late results of older queries are dropped.
    palette_seq: u64,
    file_index: Option<Arc<Vec<IndexedFile>>>,
    index_stale: bool,
    index_building: bool,
    find: Find,
    settings_open: bool,
    /// Where to put the selection once a file opened from a search hit is loaded.
    goto: Option<(PathBuf, usize, usize, usize)>,
    /// Latest search generation; running searches abort when it moves on.
    search_gen: Arc<std::sync::atomic::AtomicU64>,
    /// Tracked from keyboard events: Shift+Enter in the find box goes backwards.
    shift_down: bool,
    ctrl_down: bool,
    /// The font size changed and has not been written to settings.json yet.
    font_dirty: bool,
    /// The window as it should be restored: normal size, last position, maximized.
    window_state: session::WindowState,
    maximized: bool,
    /// The "Files:" filter of Find in Files, kept between openings.
    search_filter: String,
    popup: Option<Popup>,
    prompt: Option<Prompt>,
    /// Where the Markdown preview was last scrolled to (0..1), to skip redundant snaps.
    preview_pos: f32,
    /// Tabs still to close by "Close Others / to the Right / All" (a dirty one asks first).
    close_queue: Vec<TabId>,
    /// Tab strip viewport: scroll offset, visible width, content width (0 = not reported yet).
    tab_x: f32,
    tab_w: f32,
    tab_content: f32,
}

impl App {
    pub fn boot(args: Vec<PathBuf>) -> (Self, Task<Message>) {
        let session = session::load();
        let mut app = App {
            ws: Workspace::default(),
            tabs: TabSet::default(),
            sidebar_visible: session.sidebar_visible,
            sidebar_width: session.sidebar_width.clamp(sidebar::MIN_WIDTH, sidebar::MAX_WIDTH),
            resizing_sidebar: false,
            preview_frac: session.preview_frac.clamp(PREVIEW_MIN, PREVIEW_MAX),
            resizing_preview: false,
            split_drag: None,
            tab_drag: None,
            last_tree_click: None,
            last_tab_press: None,
            last_dir: session.last_dir.clone(),
            window_size: iced::Size::new(1180.0, 760.0),
            sidebar_scroll: Scroll { y: 0.0, height: sidebar::DEFAULT_VIEWPORT_H },
            dialog: None,
            notice: None,
            trimmed: None,
            session_dirty: false,
            restored: false,
            first_view_logged: std::cell::Cell::new(false),
            settings: Settings::default(),
            palette: None,
            palette_seq: 0,
            file_index: None,
            index_stale: true,
            index_building: false,
            find: Find::default(),
            settings_open: false,
            goto: None,
            search_gen: Arc::default(),
            shift_down: false,
            ctrl_down: false,
            font_dirty: false,
            window_state: session.window.sane(),
            maximized: session.window.maximized,
            search_filter: String::new(),
            popup: None,
            prompt: None,
            preview_pos: -1.0,
            close_queue: Vec::new(),
            tab_x: 0.0,
            tab_w: 0.0,
            tab_content: 0.0,
        };
        match settings::load() {
            Ok(s) => app.settings = s,
            Err(e) => app.notice = Some(e),
        }
        let mut tasks = Vec::new();

        // Workspace roots first: the sidebar is what the user sees first.
        let expanded: HashSet<&PathBuf> = session.expanded.iter().collect();
        for root in &session.roots {
            // (A root on a mount that does not answer stays: its listing arrives - or not -
            // on a worker, the window does not wait for it.)
            if fsx::is_dir(root) != Some(false) && app.ws.add_root(root.clone(), expanded.contains(root)) {
                tasks.push(scan(root.clone()));
            }
        }
        app.ws.expand_later(
            session
                .expanded
                .iter()
                .filter(|p| !session.roots.contains(p))
                .cloned(),
        );

        // Paths from the command line: folders become roots, files open as tabs.
        for arg in args {
            let path = fsx::canonical(&arg);
            if fsx::is_dir(&path) == Some(true) {
                if app.ws.add_root(path.clone(), true) {
                    tasks.push(scan(path));
                }
            } else {
                tasks.push(open_task(path));
            }
        }

        // Some window managers ignore the "start maximized" hint: ask again once the window exists.
        if app.maximized {
            tasks.push(window::oldest().and_then(|id| window::maximize(id, true)));
        }
        tasks.push(window::oldest().and_then(window::size).map(Message::WindowResized));
        if app.window_state.position().is_some() && !app.maximized {
            tasks.push(window::oldest().and_then(window::monitor_size).map(Message::MonitorSize));
        }
        tasks.push(Task::perform(
            loader::restore(session.tabs, session.active),
            |(loaded, active)| Message::Restored(Handoff::new(loaded), active),
        ));
        (app, Task::batch(tasks))
    }

    pub fn title(&self) -> String {
        match self.tabs.active() {
            // A saved file shows its whole path (the folder tells similar names apart).
            Some(t) => {
                let name = match &t.path {
                    Some(path) => path.display().to_string(),
                    None => t.label(LONG_LABEL).into_owned(),
                };
                format!("{}{name} - VeloEdit", if t.is_dirty() { "\u{2022} " } else { "" })
            }
            None => "VeloEdit".to_owned(),
        }
    }

    pub fn theme(&self) -> Theme {
        theme::theme(self.settings.colors())
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            // Only sees keys no widget consumed (e.g. hex/image tabs).
            keyboard::listen().map(Message::Key),
            window::close_requests().map(|_| Message::CloseRequested),
        ];
        if self.session_dirty && self.restored {
            subs.push(time::every(FLUSH_EVERY).map(|_| Message::Tick));
        }
        if self.wants_highlight() {
            // Debounce: re-parse ~90 ms after the last edit.
            subs.push(time::every(Duration::from_millis(90)).map(|_| Message::HighlightTick));
        }
        subs.push(watcher::subscription(self.watch_set()).map(Message::FsChanged));
        subs.push(event::listen_with(|event, _, _| match event {
            Event::Window(window::Event::FileDropped(path)) => Some(Message::Dropped(path)),
            _ => None,
        }));
        if self.find.open && self.dialog.is_none() && self.palette.is_none() {
            subs.push(event::listen_with(find_key));
        }
        subs.push(event::listen_with(track_pointer));
        subs.push(crate::instance::subscription());
        subs.push(window::resize_events().map(|(_, size)| Message::WindowResized(size)));
        subs.push(event::listen_with(|event, _, _| match event {
            Event::Window(window::Event::Moved(p)) => Some(Message::WindowMoved(p)),
            _ => None,
        }));
        if self.resizing_preview {
            subs.push(event::listen_with(|event, _, _| match event {
                Event::Mouse(mouse::Event::CursorMoved { position }) => Some(Message::PreviewResizeTo(position.x)),
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => Some(Message::PreviewResizeEnd),
                _ => None,
            }));
        }
        if self.tab_drag.is_some() {
            subs.push(event::listen_with(|event, _, _| match event {
                Event::Mouse(mouse::Event::CursorMoved { position }) => Some(Message::TabDragTo(position)),
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => Some(Message::TabDragEnd),
                _ => None,
            }));
        }
        if self.split_drag.is_some() {
            subs.push(event::listen_with(|event, _, _| match event {
                Event::Mouse(mouse::Event::CursorMoved { position }) => Some(Message::SplitResizeTo(position)),
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => Some(Message::SplitResizeEnd),
                _ => None,
            }));
        }
        if self.resizing_sidebar {
            subs.push(event::listen_with(|event, _, _| match event {
                Event::Mouse(mouse::Event::CursorMoved { position }) => Some(Message::SidebarResizeTo(position.x)),
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => Some(Message::SidebarResizeEnd),
                _ => None,
            }));
        }
        if self.modal_open() || self.popup.is_some() {
            // Sees *all* key events (even those a focused text input consumed).
            subs.push(event::listen_with(modal_key));
        }
        Subscription::batch(subs)
    }

    // ------------------------------------------------------------------ update

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        // Whatever changed a document, a second pane of it must show the same text.
        for tab in self.tabs.iter_mut() {
            tab.sync_split();
            // Working in a preview tab makes it a real tab.
            if tab.preview && tab.is_dirty() {
                tab.preview = false;
            }
        }
        self.refresh_preview();
        task
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Noop => Task::none(),
            Message::ToggleSidebar => {
                self.sidebar_visible = !self.sidebar_visible;
                self.touch();
                if self.sidebar_visible {
                    scroll_to(sidebar::SCROLL_ID, self.sidebar_scroll.y)
                } else {
                    Task::none()
                }
            }
            Message::Key(event) => {
                if let keyboard::Event::ModifiersChanged(m) = &event {
                    self.shift_down = m.shift();
                    self.ctrl_down = m.command();
                }
                if self.modal_open() || self.popup.is_some() {
                    return Task::none();
                }
                if let keyboard::Event::KeyPressed { key, modified_key, physical_key, modifiers, .. } = event {
                    if let Some(msg) = shortcut(&key, &modified_key, physical_key, modifiers) {
                        return self.update(msg);
                    }
                }
                Task::none()
            }
            Message::Tick => {
                // Font zoom (Ctrl+wheel) changes many times a second: written once it settles.
                let font = std::mem::take(&mut self.font_dirty)
                    .then(|| persist_setting("font_size", serde_json::json!(self.settings.font_size)));
                Task::batch([self.flush_session(), font.unwrap_or_else(Task::none)])
            }
            Message::CloseRequested => {
                // Hot exit: unsaved buffers are persisted, never prompted for.
                if !self.restored {
                    return iced::exit();
                }
                let snapshot = self.snapshot();
                Task::perform(
                    async move {
                        if let Err(e) = snapshot.write().await {
                            eprintln!("veloedit: could not save session: {e}");
                        }
                    },
                    |_| Message::Exit,
                )
            }
            Message::Exit => {
                // The session is saved. iced's shutdown waits for every worker thread, and one
                // stuck in the kernel on a network mount that stopped answering would keep the
                // process alive for minutes (and, through the single-window socket, block the
                // next launch): so whatever happens, the process ends shortly.
                #[cfg(not(test))]
                std::thread::spawn(|| {
                    std::thread::sleep(Duration::from_millis(1500));
                    crate::instance::cleanup();
                    std::process::exit(0);
                });
                iced::exit()
            }

            // ---- workspace ----
            Message::PickFolder => {
                let dir = self.dialog_dir();
                Task::perform(
                    async move {
                        with_dir(rfd::AsyncFileDialog::new().set_title("Add folder to workspace"), dir)
                            .pick_folder()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    Message::FolderPicked,
                )
            }
            Message::FolderPicked(Some(path)) => {
                if self.ws.add_root(path.clone(), true) {
                    self.index_stale = true;
                    self.touch();
                    return scan(path);
                }
                Task::none()
            }
            Message::FolderPicked(None) => Task::none(),
            Message::PickFiles => {
                let dir = self.dialog_dir();
                Task::perform(
                    async move {
                        with_dir(rfd::AsyncFileDialog::new().set_title("Open files"), dir)
                            .pick_files()
                            .await
                            .map(|hs| hs.into_iter().map(|h| h.path().to_path_buf()).collect::<Vec<_>>())
                            .unwrap_or_default()
                    },
                    Message::FilesPicked,
                )
            }
            Message::FilesPicked(paths) => {
                self.remember_dir(paths.first());
                Task::batch(paths.into_iter().map(|p| self.open_path(p)))
            }
            Message::Alive(ack) => {
                if let Some(tx) = ack.take() {
                    let _ = tx.send(());
                }
                Task::none()
            }
            Message::External(paths) => {
                let mut tasks: Vec<Task<Message>> = paths.into_iter().map(|p| self.open_external(p)).collect();
                // Bring the window to the front (the window manager may still decline to steal focus).
                tasks.push(window::oldest().and_then(|id| Task::batch([window::minimize(id, false), window::gain_focus(id)])));
                Task::batch(tasks)
            }
            Message::Dropped(path) => {
                if fsx::is_dir(&path) == Some(true) {
                    self.update(Message::FolderPicked(Some(path)))
                } else {
                    self.open_path(path)
                }
            }
            Message::RemoveRoot(path) => {
                self.ws.remove_root(&path);
                self.index_stale = true;
                self.touch();
                Task::none()
            }
            Message::TreeClicked { path, is_dir } => {
                if is_dir {
                    self.touch();
                    // Ctrl+click on an open folder tidies it away with everything inside it; on a
                    // closed one it opens just that folder.
                    if self.ctrl_down && self.ws.is_expanded(&path) {
                        self.ws.collapse_recursive(&path);
                        return Task::none();
                    }
                    if self.ws.toggle(&path) {
                        return scan(path);
                    }
                    Task::none()
                } else {
                    // One click previews (the next file replaces it); two pin the file in a tab.
                    let now = Instant::now();
                    let double = self
                        .last_tree_click
                        .as_ref()
                        .is_some_and(|(p, t)| *p == path && now.duration_since(*t) < DOUBLE_CLICK);
                    self.last_tree_click = (!double).then(|| (path.clone(), now));
                    if double { self.open_path(path) } else { self.open_path_preview(path) }
                }
            }
            Message::OpenPinned(path) => self.open_path(path),
            Message::DirScanned { path, entries } => {
                let more = self.ws.set_children(&path, entries);
                Task::batch(more.into_iter().map(scan))
            }
            Message::WindowResized(size) => {
                self.window_size = size;
                // Resizing is also how maximizing shows up: ask which one it was.
                window::oldest()
                    .and_then(window::is_maximized)
                    .map(move |maximized| Message::WindowMaximized { size, maximized })
            }
            Message::WindowMaximized { size, maximized } => {
                self.maximized = maximized;
                if !maximized && size.width >= 520.0 && size.height >= 360.0 {
                    self.window_state.width = size.width;
                    self.window_state.height = size.height;
                }
                self.window_state.maximized = maximized;
                if self.restored {
                    self.touch();
                }
                Task::none()
            }
            Message::WindowMoved(pos) => {
                if !self.maximized && self.restored {
                    self.window_state.x = Some(pos.x);
                    self.window_state.y = Some(pos.y);
                    self.touch();
                }
                Task::none()
            }
            Message::MonitorSize(monitor) => {
                // A saved position that is off every screen now (monitor unplugged): centre instead.
                let (w, h) = (self.window_state.width, self.window_state.height);
                match (monitor, self.window_state.position()) {
                    (Some(m), Some((x, y))) if x + 120.0 > m.width || y + 60.0 > m.height || x + w < 120.0 || y < -20.0 => {
                        let centred = iced::Point::new(((m.width - w) / 2.0).max(0.0), ((m.height - h) / 2.0).max(0.0));
                        window::oldest().and_then(move |id| window::move_to(id, centred))
                    }
                    _ => Task::none(),
                }
            }
            Message::PreviewResizeStart => {
                self.resizing_preview = true;
                Task::none()
            }
            Message::PreviewResizeTo(x) => {
                if self.resizing_preview {
                    // The editor area runs from the sidebar's edge to the window's right edge;
                    // the preview owns what is right of the pointer.
                    let left = if self.sidebar_visible { self.sidebar_width.clamp(sidebar::MIN_WIDTH, sidebar::MAX_WIDTH) + 5.0 } else { 0.0 };
                    let area = (self.window_size.width - left).max(100.0);
                    self.preview_frac = ((self.window_size.width - x) / area).clamp(PREVIEW_MIN, PREVIEW_MAX);
                }
                Task::none()
            }
            Message::PreviewResizeEnd => {
                self.resizing_preview = false;
                self.touch(); // remember the split
                self.settle()
            }
            Message::SidebarResizeStart => {
                self.resizing_sidebar = true;
                Task::none()
            }
            Message::SidebarResizeTo(x) => {
                if self.resizing_sidebar {
                    self.sidebar_width = x.clamp(sidebar::MIN_WIDTH, sidebar::MAX_WIDTH);
                }
                Task::none()
            }
            Message::SidebarResizeEnd => {
                self.resizing_sidebar = false;
                self.touch(); // remember the width
                self.settle()
            }
            Message::SidebarScrolled { y, height } => {
                self.sidebar_scroll = Scroll { y, height };
                Task::none()
            }

            // ---- documents ----
            Message::Opened(Ok(loaded)) => match loaded.take() {
                Some(loaded) => self.opened(loaded, false),
                None => Task::none(),
            },
            Message::OpenedPreview(Ok(loaded)) => match loaded.take() {
                Some(loaded) => self.opened(loaded, true),
                None => Task::none(),
            },
            Message::Opened(Err(e)) | Message::OpenedPreview(Err(e)) => {
                self.notice = Some(e);
                Task::none()
            }
            Message::Restored(loaded, active) => {
                let had_tabs = !self.tabs.is_empty();
                let ids: Vec<TabId> = loaded
                    .take()
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                    .map(|(i, l)| self.install(l, Some(i)))
                    .collect();
                if !had_tabs {
                    if let Some(&id) = active.and_then(|a| ids.get(a)) {
                        self.tabs.activate(id);
                    }
                }
                if self.tabs.active_id().is_none() {
                    self.tabs.activate_index(0);
                }
                self.restored = true;
                self.touch(); // re-persist drafts under this run's tab ids
                self.settle()
            }
            Message::NewFile => {
                let id = self.tabs.insert(None, None, Doc::Text(TextDoc::untitled()));
                self.tabs.activate(id);
                self.touch();
                self.settle()
            }
            Message::ActivateTab(id) => {
                self.tabs.activate(id);
                self.touch();
                self.settle()
            }
            Message::TabPressed(id) => {
                // A double click on a tab pins it.
                let now = Instant::now();
                if self.last_tab_press.is_some_and(|(t, at)| t == id && now.duration_since(at) < DOUBLE_CLICK) {
                    self.last_tab_press = None;
                    if let Some(tab) = self.tabs.get_mut(id) {
                        tab.preview = false;
                    }
                } else {
                    self.last_tab_press = Some((id, now));
                }
                self.tab_drag = Some(TabDrag { id, from: pointer(), live: false, slot: None });
                self.update(Message::ActivateTab(id))
            }
            Message::TabDragTo(p) => {
                if !LEFT_DOWN.load(std::sync::atomic::Ordering::Relaxed) {
                    self.tab_drag = None; // the release was missed
                    return Task::none();
                }
                let left = if self.sidebar_visible { self.sidebar_width.clamp(sidebar::MIN_WIDTH, sidebar::MAX_WIDTH) + 5.0 } else { 0.0 };
                let top = if self.settings.menu_bar { menu_bar::HEIGHT } else { 0.0 };
                let (strip_x, tab_w) = (self.tab_x, self.tab_w);
                let Some(d) = self.tab_drag.as_mut() else { return Task::none() };
                if !d.live {
                    if (p.x - d.from.x).abs() < 6.0 && (p.y - d.from.y).abs() < 6.0 {
                        return Task::none();
                    }
                    d.live = true;
                }
                // Dropping only counts near the strip: pulling the tab far away cancels.
                let near = p.y >= top - 12.0 && p.y <= top + 33.0 + 48.0;
                d.slot = near.then(|| tab_bar::slot_at(&self.tabs, p.x - left + strip_x));
                // At either end of the strip the strip follows the pointer.
                let nudge = if !near || tab_w <= 0.0 {
                    0.0
                } else if p.x < left + 28.0 {
                    -18.0
                } else if p.x > left + tab_w - 28.0 {
                    18.0
                } else {
                    0.0
                };
                if nudge != 0.0 { self.update(Message::TabsWheel(nudge)) } else { Task::none() }
            }
            Message::TabDragEnd => {
                let Some(d) = self.tab_drag.take() else { return Task::none() };
                if let (true, Some(slot)) = (d.live, d.slot) {
                    if self.tabs.move_to_slot(d.id, slot) {
                        self.touch(); // the order is part of the session
                        return self.reveal_active_tab();
                    }
                }
                Task::none()
            }
            Message::FocusPane(id, pane) => {
                if let Some(tab) = self.tabs.get_mut(id) {
                    tab.focus_pane(pane);
                }
                self.settle()
            }
            Message::ClosePane(id, pane) => {
                // Closing a pane keeps the other one (and its view of the text).
                if let Some(tab) = self.tabs.get_mut(id) {
                    tab.focus_pane(if pane == Pane::First { Pane::Second } else { Pane::First });
                    tab.close_split();
                }
                self.split_drag = None;
                self.settle()
            }
            Message::NextTab => {
                self.tabs.cycle(1);
                self.touch();
                self.settle()
            }
            Message::PrevTab => {
                self.tabs.cycle(-1);
                self.touch();
                self.settle()
            }
            Message::CloseActive => match self.tabs.active_id() {
                Some(id) => self.update(Message::CloseTab(id)),
                None => Task::none(),
            },
            Message::CloseTab(id) => {
                let dirty = self.tabs.get(id).is_some_and(|t| t.is_dirty());
                if dirty {
                    self.tabs.activate(id);
                    self.dialog = Some(Dialog::CloseDirty(id));
                    Task::none()
                } else {
                    self.close_now(id)
                }
            }
            Message::CloseChoice(id, choice) => {
                self.dialog = None;
                match choice {
                    CloseChoice::Save => self.update(Message::SaveTab { id, then_close: true, force: false }),
                    CloseChoice::Discard => self.close_now(id),
                    CloseChoice::Cancel => {
                        self.close_queue.clear();
                        Task::none()
                    }
                }
            }
            Message::TabContext(id) => {
                if self.modal_open() || self.tabs.get(id).is_none() {
                    return Task::none();
                }
                let entries = self.tab_entries(id);
                self.popup = Some(Popup::Context { at: pointer(), entries, width: menu::WIDTH });
                Task::none()
            }
            Message::CloseOthers(keep) => {
                let ids = self.tabs.iter().map(|t| t.id).filter(|&i| i != keep).collect();
                self.close_many(ids)
            }
            Message::CloseRight(of) => {
                let ids: Vec<TabId> = self.tabs.iter().map(|t| t.id).skip_while(|&i| i != of).skip(1).collect();
                self.close_many(ids)
            }
            Message::CloseAll => {
                let ids = self.tabs.iter().map(|t| t.id).collect();
                self.close_many(ids)
            }

            // ---- text ----
            Message::Editor(_, Action::Zoom(steps)) => self.update(Message::ChangeFontSize(steps)),
            Message::Editor(id, action) => {
                let tab_size = self.settings.tab_size();
                let mut edited = false;
                if let Some(Doc::Text(doc)) = self.tabs.get_mut(id).map(|t| &mut t.doc) {
                    doc.state.set_tab_size(tab_size);
                    edited = doc.state.apply(action, Instant::now());
                }
                if edited {
                    self.session_dirty = true;
                }
                let rebuilt = self.refresh_preview();
                self.preview_task(rebuilt)
            }
            Message::EditorPane(id, pane, action) => {
                // Looking around (wheel, scrollbar, minimap, fold arrows) does not move the
                // user's place of work: only the pane that was touched reacts.
                let passive = matches!(
                    action,
                    Action::Scroll { .. } | Action::ScrollTo { .. } | Action::ScrollToX { .. } | Action::ToggleFold { .. } | Action::Zoom(_)
                );
                if let Some(tab) = self.tabs.get_mut(id) {
                    match tab.split.as_mut() {
                        Some(s) if passive && s.focus != pane && !matches!(action, Action::Zoom(_)) => {
                            s.mirror.apply(action, Instant::now());
                            return Task::none();
                        }
                        _ if !passive => tab.focus_pane(pane),
                        _ => {}
                    }
                }
                self.update(Message::Editor(id, action))
            }
            Message::SplitTab(id, dir) => {
                if self.tabs.get(id).is_some_and(|t| matches!(t.doc, Doc::Text(_))) {
                    self.tabs.activate(id);
                    if let Some(tab) = self.tabs.get_mut(id) {
                        tab.preview = false;
                        tab.split(dir);
                    }
                    self.touch();
                    return self.settle();
                }
                Task::none()
            }
            Message::CloseSplit(id) => {
                if let Some(tab) = self.tabs.get_mut(id) {
                    tab.close_split();
                }
                self.split_drag = None;
                self.settle()
            }
            Message::SplitResizeStart => {
                if let Some(s) = self.tabs.active().and_then(|t| t.split.as_ref()) {
                    self.split_drag = Some((pointer(), s.frac));
                }
                Task::none()
            }
            Message::SplitResizeTo(p) => {
                if let Some((from, frac0)) = self.split_drag {
                    let extent = self.split_extent();
                    if let Some(s) = self.tabs.active_mut().and_then(|t| t.split.as_mut()) {
                        let moved = if s.dir == SplitDir::Right { p.x - from.x } else { p.y - from.y };
                        s.frac = (frac0 + moved / extent).clamp(SPLIT_MIN, SPLIT_MAX);
                    }
                }
                Task::none()
            }
            Message::SplitResizeEnd => {
                self.split_drag = None;
                Task::none()
            }
            Message::SplitActive(dir) => match self.tabs.active_id() {
                Some(id) => self.update(Message::SplitTab(id, dir)),
                None => Task::none(),
            },
            Message::CloseSplitActive => match self.tabs.active_id() {
                Some(id) => self.update(Message::CloseSplit(id)),
                None => Task::none(),
            },
            Message::SaveActive => match self.tabs.active_id() {
                Some(id) => self.update(Message::SaveTab { id, then_close: false, force: false }),
                None => Task::none(),
            },
            Message::SaveTab { id, then_close, force } => {
                let Some(tab) = self.tabs.get(id) else { return Task::none() };
                if matches!(tab.doc, Doc::Hex(_)) {
                    return self.save_hex(id, then_close);
                }
                let Doc::Text(doc) = &tab.doc else { return Task::none() };
                if doc.conflict && !force && tab.path.is_some() {
                    self.dialog = Some(Dialog::Conflict { id, then_close });
                    return Task::none();
                }
                match tab.path.clone() {
                    Some(path) => match self.text_to_save(id, &path) {
                        Some((text, version)) => self.write(id, path, text, version, then_close),
                        None => Task::none(),
                    },
                    None => {
                        let suggested = tab.title.clone();
                        let dir = self.dialog_dir();
                        Task::perform(
                            async move {
                                with_dir(rfd::AsyncFileDialog::new().set_title("Save file").set_file_name(&suggested), dir)
                                    .save_file()
                                    .await
                                    .map(|h| h.path().to_path_buf())
                            },
                            move |path| Message::SavePicked { id, path, then_close },
                        )
                    }
                }
            }
            Message::SavePicked { id, path, then_close } => {
                self.remember_dir(path.as_ref());
                let (Some(path), Some(_)) = (path, self.tabs.get(id)) else {
                    return Task::none(); // dialog cancelled
                };
                match self.text_to_save(id, &path) {
                    Some((text, version)) => self.write(id, path, text, version, then_close),
                    None => Task::none(),
                }
            }
            Message::Saved { id, path, version, then_close, result } => {
                if let Some(Doc::Text(doc)) = self.tabs.get_mut(id).map(|t| &mut t.doc) {
                    doc.saving = false;
                    doc.saved_at = Some(Instant::now());
                }
                match result {
                    Ok(hash) => {
                        let mut unchanged = false;
                        if let Some(tab) = self.tabs.get_mut(id) {
                            if let Doc::Text(doc) = &mut tab.doc {
                                unchanged = doc.version() == version;
                                if unchanged {
                                    doc.mark_saved(hash);
                                } else {
                                    doc.disk_hash = hash;
                                    doc.conflict = false;
                                }
                            }
                        }
                        if self.tabs.get(id).is_some_and(|t| t.path.as_deref() != Some(&path)) {
                            self.tabs.set_path(id, path.clone());
                            if let Some(Doc::Text(doc)) = self.tabs.get_mut(id).map(|t| &mut t.doc) {
                                doc.git = GitState::default(); // another file now
                                doc.hl.set_lang(Lang::detect_with(&path, &doc.state.byte_slice(0, 8192)));
                            }
                        }
                        if path == settings::path() {
                            match settings::load() {
                                Ok(s) => self.settings = s,
                                Err(e) => self.notice = Some(e),
                            }
                        }
                        self.notice = Some(match self.trimmed.take() {
                            Some((tab, n)) if tab == id => {
                                format!("Saved {} \u{2014} trimmed {n} line{}", display_name(&path), if n == 1 { "" } else { "s" })
                            }
                            _ => format!("Saved {}", display_name(&path)),
                        });
                        self.touch();
                        if then_close && unchanged {
                            return self.close_now(id);
                        }
                    }
                    Err(e) => self.notice = Some(format!("Save failed: {e}")),
                }
                Task::none()
            }

            // ---- menus, context menu, file operations ----
            Message::TreeContext { path, is_dir, is_root } => {
                if self.modal_open() {
                    return Task::none();
                }
                let entries = self.context_entries(&path, is_dir, is_root);
                self.popup = Some(Popup::Context { at: pointer(), entries, width: menu::WIDTH });
                Task::none()
            }
            Message::EditorContext => {
                if self.modal_open() {
                    return Task::none();
                }
                let entries = self.editor_entries();
                if !entries.is_empty() {
                    self.popup = Some(Popup::Context { at: pointer(), entries, width: menu::WIDTH });
                }
                Task::none()
            }
            Message::OpenMenu(i) => {
                if self.modal_open() {
                    return Task::none();
                }
                self.popup = match self.popup {
                    Some(Popup::Menu(open)) if open == i => None,
                    _ => Some(Popup::Menu(i)),
                };
                Task::none()
            }
            Message::MenuHover(i) => {
                if matches!(self.popup, Some(Popup::Menu(_))) {
                    self.popup = Some(Popup::Menu(i));
                }
                Task::none()
            }
            Message::CloseMenu => {
                self.popup = None;
                self.settle()
            }
            Message::MenuPick(msg) => {
                self.popup = None;
                // The click that picked the entry took the editor's focus away.
                let run = self.update(*msg);
                Task::batch([run, self.settle()])
            }
            Message::NewFileIn(dir) => self.open_prompt(PromptKind::NewFile(dir), String::new()),
            Message::NewFolderIn(dir) => self.open_prompt(PromptKind::NewFolder(dir), String::new()),
            Message::RenameEntry(path) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.open_prompt(PromptKind::Rename(path), name)
            }
            Message::TrashEntry(path) => self.open_prompt(PromptKind::Trash(path), String::new()),
            Message::PromptInput(text) => {
                if let Some(p) = &mut self.prompt {
                    p.text = text;
                    p.error = None;
                }
                Task::none()
            }
            Message::PromptSubmit => {
                let Some(p) = &self.prompt else { return Task::none() };
                let (kind, name) = (p.kind.clone(), p.text.clone());
                Task::perform(
                    async move {
                        fsx::off_thread(move || match kind {
                            PromptKind::NewFile(dir) => fileops::create_file(&dir, &name),
                            PromptKind::NewFolder(dir) => fileops::create_dir(&dir, &name),
                            PromptKind::Rename(path) => fileops::rename(&path, &name),
                            PromptKind::Trash(path) => fileops::trash(&path),
                        })
                        .await
                        .unwrap_or_else(|| Err("the file operation failed".to_owned()))
                    },
                    Message::FileOp,
                )
            }
            Message::FileOp(Err(e)) => {
                match &mut self.prompt {
                    Some(p) => p.error = Some(e),
                    None => self.notice = Some(e),
                }
                Task::none()
            }
            Message::FileOp(Ok(done)) => {
                self.prompt = None;
                self.index_stale = true;
                self.touch();
                let mut tasks = vec![self.settle()];
                match done {
                    fileops::Done::Created { path, is_dir } => {
                        if let Some(dir) = path.parent().map(Path::to_path_buf) {
                            // Show the new entry: expand a collapsed folder, else just refresh it.
                            if !self.ws.is_loaded(&dir) && self.ws.toggle(&dir) || self.ws.is_loaded(&dir) {
                                tasks.push(scan(dir));
                            }
                        }
                        if !is_dir {
                            tasks.push(self.open_path(path));
                        }
                    }
                    fileops::Done::Renamed { from, to } => {
                        let moved: Vec<(TabId, PathBuf)> = self
                            .tabs
                            .iter()
                            .filter_map(|t| {
                                let p = t.path.as_deref()?;
                                Some((t.id, to.join(p.strip_prefix(&from).ok()?)))
                            })
                            .collect();
                        for (id, path) in moved {
                            self.tabs.set_path(id, path.clone());
                            if let Some(Doc::Text(doc)) = self.tabs.get_mut(id).map(|t| &mut t.doc) {
                                doc.hl.set_lang(Lang::detect_with(&path, &doc.state.byte_slice(0, 8192)));
                            }
                        }
                        if let Some(dir) = from.parent() {
                            tasks.push(scan(dir.to_path_buf()));
                        }
                    }
                    fileops::Done::Trashed(path) => {
                        let gone: Vec<TabId> = self
                            .tabs
                            .iter()
                            .filter(|t| !t.is_dirty() && t.path.as_deref().is_some_and(|p| p.starts_with(&path)))
                            .map(|t| t.id)
                            .collect();
                        for id in gone {
                            tasks.push(self.close_now(id));
                        }
                        self.notice = Some(format!("Moved to Trash: {}", display_name(&path)));
                        if let Some(dir) = path.parent() {
                            tasks.push(scan(dir.to_path_buf()));
                        }
                    }
                }
                Task::batch(tasks)
            }
            Message::CopyText(text) => iced::clipboard::write(text),
            Message::Reveal(path) => {
                let dir = if fsx::is_dir(&path) == Some(true) { path } else { path.parent().map(Path::to_path_buf).unwrap_or(path) };
                open_with_desktop(dir.into_os_string());
                Task::none()
            }
            Message::OpenLink(url) => self.open_link(url),
            Message::RefreshDir(path) => {
                self.index_stale = true;
                scan(path)
            }
            Message::EditAction(action) => match self.tabs.active_id() {
                Some(id) if matches!(self.tabs.get(id).map(|t| &t.doc), Some(Doc::Text(_))) => {
                    let t = self.update(Message::Editor(id, action));
                    Task::batch([t, self.settle()])
                }
                _ => Task::none(),
            },
            Message::CopySelection => self.copy_selection(false),
            Message::CutSelection => self.copy_selection(true),
            Message::PasteRequest => iced::clipboard::read().map(|t| t.map_or(Message::Noop, Message::PasteText)),
            Message::PasteText(text) => self.update(Message::EditAction(Action::Insert(text))),
            Message::Quit => self.update(Message::CloseRequested),
            Message::About => {
                if self.dialog.is_none() {
                    self.palette = None;
                    self.dialog = Some(Dialog::About);
                }
                Task::none()
            }
            Message::TabsViewport { x, width, content } => {
                let first = self.tab_w <= 0.0;
                (self.tab_x, self.tab_w, self.tab_content) = (x, width, content);
                // The very first report: the window just opened with a restored active tab.
                if first { self.reveal_active_tab() } else { Task::none() }
            }
            Message::TabsWheel(px) => {
                let max = (self.tab_content - self.tab_w).max(0.0);
                self.tab_x = (self.tab_x + px).clamp(0.0, max);
                scroll_x_to(tab_bar::SCROLL_ID, self.tab_x)
            }
            Message::OpenTabList => {
                if self.modal_open() || self.tabs.is_empty() {
                    return Task::none();
                }
                let active = self.tabs.active_id();
                let entries = self
                    .tabs
                    .iter()
                    .map(|t| {
                        let hint = match &t.path {
                            Some(p) => {
                                let dir: Vec<_> = p.parent().into_iter().flat_map(|d| d.components().rev().take(2)).collect();
                                dir.iter().rev().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
                            }
                            None => "unsaved".to_owned(),
                        };
                        let label = format!("{}{}", if t.is_dirty() { "\u{25CF} " } else { "" }, t.label(LONG_LABEL));
                        Entry::with_hint(label, hint, Message::ActivateTab(t.id), Some(Some(t.id) == active))
                    })
                    .collect();
                self.popup = Some(Popup::Context { at: pointer(), entries, width: 360.0 });
                Task::none()
            }

            // ---- palette / commands ----
            Message::OpenPalette(mode) => {
                if self.dialog.is_some() {
                    return Task::none();
                }
                if self.palette.as_ref().is_some_and(|p| p.mode == mode) {
                    self.palette = None; // same shortcut again toggles
                    return self.settle();
                }
                self.open_palette(mode)
            }
            Message::ClosePalette => {
                self.palette = None;
                self.settle()
            }
            Message::Escape => {
                if self.tab_drag.take().is_some_and(|d| d.live) {
                    return Task::none();
                }
                if self.popup.take().is_some() {
                    return Task::none();
                }
                if self.prompt.take().is_some() {
                    return self.settle();
                }
                if self.dialog.take().is_some() {
                    self.close_queue.clear();
                    return Task::none();
                }
                if self.palette.take().is_some() || std::mem::take(&mut self.settings_open) {
                    return self.settle();
                }
                if self.find.open {
                    self.find.open = false;
                    return self.settle();
                }
                Task::none()
            }
            Message::PaletteQuery(q) => {
                if let Some(p) = &mut self.palette {
                    p.query = q;
                }
                self.refresh_palette()
            }
            Message::PaletteToggle(flag) => {
                if let Some(p) = &mut self.palette {
                    match flag {
                        FindFlag::Case => p.case_sensitive = !p.case_sensitive,
                        FindFlag::Word => p.whole_word = !p.whole_word,
                        FindFlag::Regex => p.regex = !p.regex,
                    }
                }
                self.refresh_palette()
            }
            Message::SearchResults { seq, outcome } => {
                if seq != self.palette_seq {
                    return Task::none();
                }
                let Some(p) = self.palette.as_mut().filter(|p| p.mode == Mode::Search) else { return Task::none() };
                let (n, f) = (outcome.hits.len(), outcome.files);
                p.status = match (n, outcome.truncated) {
                    (0, _) => format!("No results in {f} files"),
                    (n, true) => format!("First {n} results (more exist) in {f} files"),
                    (n, false) => format!("{n} result{} in {f} file{}", if n == 1 { "" } else { "s" }, if f == 1 { "" } else { "s" }),
                };
                p.set_results(outcome);
                scroll_to(search_panel::SCROLL_ID, 0.0)
            }
            Message::SearchFilter(filter) => {
                self.search_filter = filter.clone();
                if let Some(p) = &mut self.palette {
                    p.filter = filter;
                }
                self.refresh_palette()
            }
            Message::SearchScope(scope) => {
                if let Some(p) = &mut self.palette {
                    p.scope = scope;
                }
                self.refresh_palette()
            }
            Message::PickSearchScope => {
                let dir = self.dialog_dir();
                Task::perform(
                    async move {
                        with_dir(rfd::AsyncFileDialog::new().set_title("Search in folder"), dir)
                            .pick_folder()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    |picked| picked.map_or(Message::Noop, |p| Message::SearchScope(Some(p))),
                )
            }
            Message::SearchScrolled { y, height } => {
                if let Some(p) = &mut self.palette {
                    p.scroll_y = y;
                    p.scroll_h = height;
                }
                Task::none()
            }
            Message::SearchTab(back) => match self.palette.as_ref().map(|p| p.mode) {
                Some(Mode::Search) => widget::operation::focus(if back { palette_view::INPUT_ID } else { search_panel::FILTER_ID }),
                _ => Task::none(),
            },
            Message::FindInFolder(path) => {
                if self.dialog.is_some() {
                    return Task::none();
                }
                // An open Find in Files keeps its query; otherwise start a fresh one.
                let open = if self.palette.as_ref().is_some_and(|p| p.mode == Mode::Search) {
                    Task::none()
                } else {
                    self.open_palette(Mode::Search)
                };
                if let Some(p) = &mut self.palette {
                    p.scope = Some(path);
                }
                Task::batch([open, self.refresh_palette()])
            }
            Message::OpenAt { path, line, col, len } => {
                self.goto = Some((path.clone(), line, col, len));
                match self.tabs.by_path(&path) {
                    Some(id) => {
                        self.tabs.activate(id);
                        self.apply_goto();
                        self.touch();
                        self.settle()
                    }
                    None => open_task(path),
                }
            }
            Message::GotoLine(n) => {
                if let Some(Doc::Hex(doc)) = self.tabs.active_id().and_then(|id| self.tabs.get_mut(id)).map(|t| &mut t.doc) {
                    doc.apply(hex::HexAction::GoTo(n as u64));
                    return self.settle();
                }
                if let Some(Doc::Text(doc)) = self.tabs.active_id().and_then(|id| self.tabs.get_mut(id)).map(|t| &mut t.doc) {
                    let line = n.saturating_sub(1).min(doc.state.line_count().saturating_sub(1));
                    let at = doc.state.line_start(line);
                    doc.state.select_range(at, at);
                }
                self.settle()
            }
            Message::OpenGoto => {
                let t = self.open_palette(Mode::Files);
                if let Some(p) = &mut self.palette {
                    p.query = ":".to_owned();
                }
                Task::batch([t, self.refresh_palette()])
            }
            Message::PaletteMove(delta) => {
                let Some(p) = &mut self.palette else { return Task::none() };
                p.select(delta);
                if p.mode != Mode::Search {
                    return Task::none();
                }
                // Keep the selected hit inside the visible part of the results list.
                let Some(row) = p.selected_row() else { return Task::none() };
                let (top, bottom) = (row as f32 * search_panel::ROW_H, (row + 1) as f32 * search_panel::ROW_H);
                let target = if top < p.scroll_y + search_panel::ROW_H {
                    // Show the file header above the first hit of a file as well.
                    Some((top - search_panel::ROW_H).max(0.0))
                } else if bottom > p.scroll_y + p.scroll_h {
                    Some(bottom - p.scroll_h)
                } else {
                    None
                };
                match target {
                    Some(y) => {
                        p.scroll_y = y;
                        scroll_to(search_panel::SCROLL_ID, y)
                    }
                    None => Task::none(),
                }
            }
            Message::PaletteSubmit => {
                let target = self.palette.as_ref().and_then(|p| palette_view::target_of(p, p.selected));
                self.run_target(target)
            }
            Message::PaletteClick(i) => {
                let target = self.palette.as_ref().and_then(|p| palette_view::target_of(p, i));
                self.run_target(target)
            }
            Message::PaletteResults { seq, hits } => {
                if seq == self.palette_seq {
                    if let (Some(p), Some(index)) = (&mut self.palette, &self.file_index) {
                        p.items = hits.iter().filter_map(|&i| index.get(i)).map(Item::file).collect();
                        p.selected = 0;
                    }
                }
                Task::none()
            }
            Message::IndexBuilt(index) => {
                self.file_index = Some(index);
                self.index_building = false;
                self.index_stale = false;
                self.refresh_palette()
            }
            Message::ReloadActive => match self.tabs.active_id() {
                Some(id) => self.update(Message::ReloadFromDisk(id)),
                None => Task::none(),
            },
            Message::OpenSettings => Task::perform(
                async {
                    match settings::ensure_file() {
                        Ok(path) => loader::open(path).await.map(Handoff::new),
                        Err(e) => Err(e.to_string()),
                    }
                },
                Message::Opened,
            ),
            Message::SetTheme(name) => {
                let Some(colors) = theme::by_name(&name) else { return Task::none() };
                self.settings.theme = colors.name.to_owned();
                persist_setting("theme", serde_json::json!(colors.name))
            }
            Message::OpenSettingsPanel => {
                if self.dialog.is_some() {
                    return Task::none();
                }
                self.palette = None;
                self.settings_open = true;
                Task::none()
            }
            Message::ChangeTabSize(delta) => {
                self.settings.tab_size = (self.settings.tab_size() as i32 + delta).clamp(1, 16) as u8;
                persist_setting("tab_size", serde_json::json!(self.settings.tab_size))
            }
            Message::ToggleSetting(toggle) => match toggle {
                Toggle::WordWrap => {
                    self.settings.word_wrap = !self.settings.word_wrap;
                    persist_setting("word_wrap", serde_json::json!(self.settings.word_wrap))
                }
                Toggle::Minimap => {
                    self.settings.minimap = !self.settings.minimap;
                    persist_setting("minimap", serde_json::json!(self.settings.minimap))
                }
                Toggle::MarkdownPreview => {
                    self.settings.markdown_preview = !self.settings.markdown_preview;
                    let on = self.settings.markdown_preview;
                    let save = persist_setting("markdown_preview", serde_json::json!(on));
                    Task::batch([save, self.settle()])
                }
                Toggle::MenuBar => {
                    self.settings.menu_bar = !self.settings.menu_bar;
                    persist_setting("menu_bar", serde_json::json!(self.settings.menu_bar))
                }
                Toggle::TrimOnSave => {
                    self.settings.trim_trailing_whitespace = !self.settings.trim_trailing_whitespace;
                    persist_setting("trim_trailing_whitespace", serde_json::json!(self.settings.trim_trailing_whitespace))
                }
            },
            Message::ChangeFontSize(delta) => {
                let wanted = (self.settings.font_size() + delta as f32).clamp(8.0, 40.0);
                self.set_font_size(wanted)
            }
            Message::ResetFontSize => self.set_font_size(Settings::default().font_size),
            Message::SettingsWritten(result) => {
                if let Err(e) = result {
                    self.notice = Some(e);
                }
                Task::none()
            }

            // ---- find / replace ----
            Message::OpenFind { replace } => {
                if self.dialog.is_some() || self.palette.is_some() {
                    return Task::none();
                }
                if let Some(Doc::Text(doc)) = self.tabs.active().map(|t| &t.doc) {
                    if let Some(t) = doc.state.selected_text().filter(|t| !t.contains('\n') && t.len() <= 200) {
                        self.find.query.text = t;
                    }
                }
                self.find.open = true;
                self.find.replace = replace;
                self.find.jump = true;
                // The field comes up fully selected: typing starts a new search, an arrow key
                // edits the old one.
                let input = if replace { find_bar::REPLACE_INPUT_ID } else { find_bar::FIND_INPUT_ID };
                Task::batch([widget::operation::focus(input), widget::operation::select_all(input), self.start_analysis()])
            }
            Message::CloseFind => {
                self.find.open = false;
                self.settle()
            }
            Message::FindQuery(text) => {
                self.find.query.text = text;
                self.find.jump = true;
                self.start_analysis()
            }
            Message::FindReplacement(text) => {
                self.find.replacement = text;
                Task::none()
            }
            Message::FindToggle(flag) => {
                let q = &mut self.find.query;
                match flag {
                    FindFlag::Case => q.case_sensitive = !q.case_sensitive,
                    FindFlag::Word => q.whole_word = !q.whole_word,
                    FindFlag::Regex => q.regex = !q.regex,
                }
                self.find.jump = true;
                self.start_analysis()
            }
            Message::FindNext => self.find_step(true),
            Message::FindPrev => self.find_step(false),
            Message::FindSubmit => self.find_step(!self.shift_down),
            Message::FindResults { key, matches, error } => {
                if self.find.requested.as_ref() == Some(&key) {
                    self.find.requested = None;
                }
                if key.query != self.find.query {
                    return self.start_analysis(); // typed on meanwhile
                }
                self.find.matches = matches;
                self.find.matches_key = Some(key.clone());
                self.find.error = error;
                let current = self.tabs.active_id().zip(self.tabs.active().and_then(|t| match &t.doc {
                    Doc::Text(d) => Some(d.version()),
                    _ => None,
                }));
                if self.find.jump && current == Some((key.tab, key.version)) {
                    self.find.jump = false;
                    self.find_jump();
                }
                self.start_analysis()
            }
            Message::ReplaceOne => self.replace(false),
            Message::ReplaceAll => self.replace(true),
            Message::Replaced { id, version, edit, count, then_next } => {
                let mut applied = false;
                if let (Some(edit), Some(Doc::Text(doc))) = (edit, self.tabs.get_mut(id).map(|t| &mut t.doc)) {
                    if doc.version() == version {
                        doc.state.replace_range(edit.start, edit.end, &edit.text, Instant::now());
                        applied = true;
                    }
                }
                if applied {
                    self.session_dirty = true;
                    if !then_next {
                        self.notice = Some(format!("Replaced {count} occurrence{}", if count == 1 { "" } else { "s" }));
                    }
                } else if !then_next {
                    self.notice = Some("Nothing to replace".to_owned());
                }
                self.find.jump = then_next;
                self.start_analysis()
            }

            // ---- syntax ----
            Message::HighlightTick => self.start_analysis(),
            Message::RefreshGit => {
                if let Some(Doc::Text(doc)) = self.tabs.active_id().and_then(|id| self.tabs.get_mut(id)).map(|t| &mut t.doc) {
                    doc.git = GitState::default();
                }
                self.start_analysis()
            }
            Message::GitBase { id, path, base } => {
                if let Some(tab) = self.tabs.get_mut(id).filter(|t| t.path.as_deref() == Some(path.as_path())) {
                    if let Doc::Text(doc) = &mut tab.doc {
                        doc.git.base = base.map_or(Base::Absent, Base::Ready);
                        doc.git.marks = None;
                        doc.git.requested = u64::MAX;
                        doc.git.in_flight = false;
                    }
                }
                self.start_analysis()
            }
            Message::GitMarksReady { id, marks, .. } => {
                if let Some(Doc::Text(doc)) = self.tabs.get_mut(id).map(|t| &mut t.doc) {
                    doc.git.in_flight = false;
                    doc.git.marks = Some(marks);
                }
                Task::none()
            }
            Message::Highlighted { id, lang, result } => {
                if let Some(Doc::Text(doc)) = self.tabs.get_mut(id).map(|t| &mut t.doc) {
                    if doc.hl.lang == Some(lang) {
                        doc.hl.in_flight = false;
                        match result {
                            Some((parse, spans)) => {
                                doc.hl.parse = Some(parse);
                                doc.hl.spans = Some(spans);
                            }
                            None => doc.hl.disabled = true,
                        }
                    }
                }
                Task::none()
            }

            // ---- disk ----
            Message::FsChanged(paths) => self.fs_changed(paths),
            Message::DiskChanged { id, force, result } => self.disk_changed(id, force, result),
            Message::ReloadFromDisk(id) => match self.tabs.get(id).and_then(|t| t.path.clone()) {
                Some(path) => reload_task(id, path, true),
                None => Task::none(),
            },
            Message::ConflictChoice(id, then_close, choice) => {
                self.dialog = None;
                match choice {
                    ConflictChoice::Overwrite => {
                        self.update(Message::SaveTab { id, then_close, force: true })
                    }
                    ConflictChoice::Reload => self.update(Message::ReloadFromDisk(id)),
                    ConflictChoice::Cancel => Task::none(),
                }
            }

            // ---- hex ----
            Message::Hex(id, action) => {
                let mut changed = false;
                if let Some(Doc::Hex(doc)) = self.tabs.get_mut(id).map(|t| &mut t.doc) {
                    changed = doc.apply(action);
                }
                if changed {
                    self.session_dirty = true;
                }
                Task::none()
            }
            Message::HexSaved { id, saved, then_close, result } => match result {
                Ok(()) => {
                    let mut clean = false;
                    if let Some(Doc::Hex(doc)) = self.tabs.get_mut(id).map(|t| &mut t.doc) {
                        doc.finish_save(&saved);
                        clean = !doc.dirty();
                    }
                    self.notice = Some(format!("Saved {} byte{}", saved.len(), if saved.len() == 1 { "" } else { "s" }));
                    self.touch();
                    if then_close && clean { self.close_now(id) } else { Task::none() }
                }
                Err(e) => {
                    self.notice = Some(format!("Save failed: {e}"));
                    Task::none()
                }
            },
        }
    }

    // ---------------------------------------------------------------- tab strip

    /// Scrolls the tab strip so the active tab is fully visible.
    fn reveal_active_tab(&mut self) -> Task<Message> {
        let Some((left, right)) = self.tabs.active_id().and_then(|id| tab_bar::span_of(&self.tabs, id)) else {
            return Task::none();
        };
        if self.tab_w <= 0.0 {
            return Task::none(); // the strip has not reported its size yet
        }
        let target = if left < self.tab_x {
            left - 8.0
        } else if right > self.tab_x + self.tab_w {
            right - self.tab_w + 8.0
        } else {
            return Task::none();
        };
        self.tab_x = target.max(0.0);
        scroll_x_to(tab_bar::SCROLL_ID, self.tab_x)
    }

    // ------------------------------------------------------------ markdown preview

    /// Width of the Markdown preview pane in logical pixels.
    fn preview_width(&self) -> f32 {
        let left = if self.sidebar_visible { self.sidebar_width.clamp(sidebar::MIN_WIDTH, sidebar::MAX_WIDTH) + 5.0 } else { 0.0 };
        ((self.window_size.width - left) * self.preview_frac - 5.0).max(0.0)
    }

    /// Is the active tab a Markdown file shown with its preview?
    fn preview_active(&self) -> bool {
        self.settings.markdown_preview
            && matches!(self.tabs.active().map(|t| &t.doc), Some(Doc::Text(d)) if d.hl.lang == Some(Lang::Markdown))
    }

    /// Re-parses the active Markdown document when it changed since the last parse
    /// (or, for a big one, when the editor scrolled to another part). Returns whether it did.
    fn refresh_preview(&mut self) -> bool {
        if !self.preview_active() {
            return false;
        }
        let Some(id) = self.tabs.active_id() else { return false };
        let Some(Doc::Text(doc)) = self.tabs.get_mut(id).map(|t| &mut t.doc) else { return false };
        if doc.preview.as_ref().is_some_and(|p| p.is_current(doc)) {
            return false;
        }
        let prev = doc.preview.take();
        doc.preview = Some(crate::editor::preview::Preview::build(doc, prev));
        true
    }

    /// Scrolls the preview to where the source is. `force`: even if it hardly moved.
    fn preview_task(&mut self, force: bool) -> Task<Message> {
        if !self.preview_active() {
            return Task::none();
        }
        let Some(Doc::Text(doc)) = self.tabs.active().map(|t| &t.doc) else { return Task::none() };
        let Some(preview) = &doc.preview else { return Task::none() };
        let frac = preview.scroll_fraction(doc);
        if !force && (frac - self.preview_pos).abs() < 0.002 {
            return Task::none();
        }
        self.preview_pos = frac;
        widget::operation::snap_to(widget::Id::new(markdown_view::SCROLL_ID), scrollable::RelativeOffset { x: Some(0.0), y: Some(frac) })
    }

    fn open_link(&mut self, url: String) -> Task<Message> {
        let lower = url.to_ascii_lowercase();
        if ["http://", "https://", "mailto:"].iter().any(|p| lower.starts_with(p)) {
            open_with_desktop(url.into());
            return Task::none();
        }
        // A relative link to another file next to this document.
        let target = self
            .tabs
            .active()
            .and_then(|t| t.path.as_deref())
            .and_then(Path::parent)
            .map(|dir| dir.join(url.split(['#', '?']).next().unwrap_or("")));
        match target {
            Some(path) if fsx::is_file(&path) == Some(true) => self.open_path(path),
            _ => {
                self.notice = Some(format!("Can't open link: {url}"));
                Task::none()
            }
        }
    }

    // ------------------------------------------------------- menus / file operations

    /// Any modal layer that owns the keyboard.
    fn modal_open(&self) -> bool {
        self.dialog.is_some() || self.palette.is_some() || self.settings_open || self.prompt.is_some()
    }

    fn open_prompt(&mut self, kind: PromptKind, text: String) -> Task<Message> {
        if self.dialog.is_some() {
            return Task::none();
        }
        let select = matches!(kind, PromptKind::Rename(_));
        let input = !matches!(kind, PromptKind::Trash(_));
        self.palette = None;
        self.prompt = Some(Prompt { kind, text, error: None });
        match (input, select) {
            (false, _) => Task::none(),
            (true, false) => widget::operation::focus(PROMPT_INPUT),
            (true, true) => Task::batch([widget::operation::focus(PROMPT_INPUT), widget::operation::select_all(PROMPT_INPUT)]),
        }
    }

    fn copy_selection(&mut self, cut: bool) -> Task<Message> {
        let text = match self.tabs.active().map(|t| &t.doc) {
            Some(Doc::Text(doc)) => doc.state.selected_text(),
            _ => None,
        };
        let Some(text) = text else { return Task::none() };
        let write = iced::clipboard::write(text);
        if cut {
            Task::batch([write, self.update(Message::EditAction(Action::DeleteSelection))])
        } else {
            write
        }
    }

    /// `path` relative to the workspace folder that contains it.
    fn relative_path(&self, path: &Path) -> String {
        self.ws
            .root_paths()
            .iter()
            .find_map(|root| path.strip_prefix(root).ok())
            .map(|rel| rel.to_string_lossy().into_owned())
            .filter(|rel| !rel.is_empty())
            .unwrap_or_else(|| path.to_string_lossy().into_owned())
    }

    /// Right-click menu of the text editor.
    fn editor_entries(&self) -> Vec<Entry> {
        let Some(tab) = self.tabs.active() else { return Vec::new() };
        let Doc::Text(doc) = &tab.doc else { return Vec::new() };
        let has_selection = doc.state.selected_text().is_some();
        let mut e = vec![
            Entry::item("Cut", "Ctrl+X", Message::CutSelection).when(has_selection),
            Entry::item("Copy", "Ctrl+C", Message::CopySelection).when(has_selection),
            Entry::item("Paste", "Ctrl+V", Message::PasteRequest),
            Entry::item("Select All", "Ctrl+A", Message::EditAction(Action::SelectAll)),
            Entry::Sep,
            Entry::item("Undo", "Ctrl+Z", Message::EditAction(Action::Undo)),
            Entry::item("Redo", "Ctrl+Shift+Z", Message::EditAction(Action::Redo)),
            Entry::Sep,
            Entry::item("Find...", "Ctrl+F", Message::OpenFind { replace: false }),
            Entry::item("Replace...", "Ctrl+H", Message::OpenFind { replace: true }),
            Entry::item(
                if has_selection { "Find Selection in Files..." } else { "Find in Files..." },
                "Ctrl+Shift+F",
                Message::OpenPalette(Mode::Search),
            ),
            Entry::item("Go to Line...", "Ctrl+G", Message::OpenGoto),
        ];
        if let Some(path) = &tab.path {
            e.push(Entry::Sep);
            e.push(Entry::item("Copy File Path", "", Message::CopyText(path.to_string_lossy().into_owned())));
            e.push(Entry::item("Reveal in File Manager", "", Message::Reveal(path.clone())));
        }
        e
    }

    fn context_entries(&self, path: &Path, is_dir: bool, is_root: bool) -> Vec<Entry> {
        let p = || path.to_path_buf();
        let mut e = Vec::new();
        if is_dir {
            e.push(Entry::item("New File...", "", Message::NewFileIn(p())));
            e.push(Entry::item("New Folder...", "", Message::NewFolderIn(p())));
            e.push(Entry::Sep);
            e.push(Entry::item("Find in Folder...", "", Message::FindInFolder(p())));
        } else {
            e.push(Entry::item("Open", "", Message::OpenPinned(p())));
        }
        if !is_root {
            e.push(Entry::Sep);
            e.push(Entry::item("Rename...", "", Message::RenameEntry(p())));
            e.push(Entry::item("Move to Trash...", "", Message::TrashEntry(p())));
        }
        e.push(Entry::Sep);
        e.push(Entry::item("Copy Path", "", Message::CopyText(path.to_string_lossy().into_owned())));
        if !is_root {
            e.push(Entry::item("Copy Relative Path", "", Message::CopyText(self.relative_path(path))));
        }
        e.push(Entry::item("Reveal in File Manager", "", Message::Reveal(p())));
        if is_dir {
            e.push(Entry::item("Refresh", "", Message::RefreshDir(p())));
        }
        if is_root {
            e.push(Entry::Sep);
            e.push(Entry::item("Remove Folder from Workspace", "", Message::RemoveRoot(p())));
        }
        e
    }

    /// Entries of menu bar drop-down `i` (see `menu_bar::MENUS`).
    fn menu_entries(&self, i: usize) -> Vec<Entry> {
        let text_tab = matches!(self.tabs.active().map(|t| &t.doc), Some(Doc::Text(_)));
        let split_dir = self.tabs.active().and_then(|t| t.split.as_ref()).map(|s| s.dir);
        let any_tab = self.tabs.active().is_some();
        let s = &self.settings;
        match i {
            0 => vec![
                Entry::item("New File", "Ctrl+N", Message::NewFile),
                Entry::item("Open File...", "Ctrl+O", Message::PickFiles),
                Entry::item("Add Folder to Workspace...", "Ctrl+Shift+O", Message::PickFolder),
                Entry::Sep,
                Entry::item("Save", "Ctrl+S", Message::SaveActive).when(any_tab),
                Entry::item("Reload from Disk", "", Message::ReloadActive).when(any_tab),
                Entry::Sep,
                Entry::item("Close Tab", "Ctrl+W", Message::CloseActive).when(any_tab),
                Entry::item("Close All Tabs", "", Message::CloseAll).when(any_tab),
                Entry::item("Quit", "Ctrl+Q", Message::Quit),
            ],
            1 => vec![
                Entry::item("Undo", "Ctrl+Z", Message::EditAction(Action::Undo)).when(text_tab),
                Entry::item("Redo", "Ctrl+Shift+Z", Message::EditAction(Action::Redo)).when(text_tab),
                Entry::Sep,
                Entry::item("Cut", "Ctrl+X", Message::CutSelection).when(text_tab),
                Entry::item("Copy", "Ctrl+C", Message::CopySelection).when(text_tab),
                Entry::item("Paste", "Ctrl+V", Message::PasteRequest).when(text_tab),
                Entry::item("Select All", "Ctrl+A", Message::EditAction(Action::SelectAll)).when(text_tab),
                Entry::Sep,
                Entry::item("Find...", "Ctrl+F", Message::OpenFind { replace: false }).when(text_tab),
                Entry::item("Replace...", "Ctrl+H", Message::OpenFind { replace: true }).when(text_tab),
                Entry::item("Find Next", "F3", Message::FindNext).when(text_tab),
                Entry::item("Find Previous", "Shift+F3", Message::FindPrev).when(text_tab),
                Entry::Sep,
                Entry::item("Find in Files...", "Ctrl+Shift+F", Message::OpenPalette(Mode::Search)),
            ],
            2 => vec![
                Entry::toggle("Sidebar", self.sidebar_visible, Message::ToggleSidebar),
                Entry::toggle("Minimap", s.minimap, Message::ToggleSetting(Toggle::Minimap)),
                Entry::toggle("Word Wrap", s.word_wrap, Message::ToggleSetting(Toggle::WordWrap)),
                Entry::toggle("Markdown Preview", s.markdown_preview, Message::ToggleSetting(Toggle::MarkdownPreview)),
                Entry::toggle("Menu Bar", s.menu_bar, Message::ToggleSetting(Toggle::MenuBar)),
                Entry::Sep,
                Entry::item("Fold Code Block", "Ctrl+Shift+[", Message::EditAction(Action::Fold)).when(text_tab),
                Entry::item("Unfold Code Block", "Ctrl+Shift+]", Message::EditAction(Action::Unfold)).when(text_tab),
                Entry::item("Fold All", "", Message::EditAction(Action::FoldAll)).when(text_tab),
                Entry::item("Fold Level 1", "", Message::EditAction(Action::FoldLevel(1))).when(text_tab),
                Entry::item("Fold Level 2", "", Message::EditAction(Action::FoldLevel(2))).when(text_tab),
                Entry::item("Unfold All", "", Message::EditAction(Action::UnfoldAll)).when(text_tab),
                Entry::Sep,
                Entry::toggle("Split Right", split_dir == Some(SplitDir::Right), Message::SplitActive(SplitDir::Right)).when(text_tab),
                Entry::toggle("Split Down", split_dir == Some(SplitDir::Down), Message::SplitActive(SplitDir::Down)).when(text_tab),
                Entry::item("Close Split", "", Message::CloseSplitActive).when(split_dir.is_some()),
                Entry::Sep,
                Entry::item("Increase Font Size", "Ctrl+=", Message::ChangeFontSize(1)),
                Entry::item("Decrease Font Size", "Ctrl+-", Message::ChangeFontSize(-1)),
                Entry::item("Reset Font Size", "Ctrl+0", Message::ResetFontSize),
                Entry::Sep,
                Entry::item("Next Tab", "Ctrl+Tab", Message::NextTab),
                Entry::item("Previous Tab", "Ctrl+Shift+Tab", Message::PrevTab),
            ],
            3 => vec![
                Entry::item("Go to File...", "Ctrl+P", Message::OpenPalette(Mode::Files)),
                Entry::item("Go to Line...", "Ctrl+G", Message::OpenGoto).when(any_tab),
                Entry::item("Command Palette...", "Ctrl+Shift+P", Message::OpenPalette(Mode::Commands)),
            ],
            4 => {
                let mut v = vec![
                    Entry::item("Settings...", "Ctrl+,", Message::OpenSettingsPanel),
                    Entry::item("Open settings.json", "", Message::OpenSettings),
                    Entry::Sep,
                    Entry::toggle("Trim Trailing Spaces on Save", s.trim_trailing_whitespace, Message::ToggleSetting(Toggle::TrimOnSave)),
                    Entry::Sep,
                ];
                v.extend(theme::ALL.iter().map(|c| {
                    let mut name = c.name.to_owned();
                    name[..1].make_ascii_uppercase();
                    Entry::toggle(format!("Theme: {name}"), s.colors().name == c.name, Message::SetTheme(c.name.to_owned()))
                }));
                v
            }
            _ => vec![
                Entry::item("Command Palette...", "Ctrl+Shift+P", Message::OpenPalette(Mode::Commands)),
                Entry::item("About VeloEdit", "", Message::About),
            ],
        }
    }

    // ----------------------------------------------------------------- palette

    fn open_palette(&mut self, mode: Mode) -> Task<Message> {
        let mut palette = Palette::new(mode);
        if mode == Mode::Search {
            palette.filter = self.search_filter.clone();
            if let Some(Doc::Text(doc)) = self.tabs.active().map(|t| &t.doc) {
                if let Some(t) = doc.state.selected_text().filter(|t| !t.contains('\n') && t.len() <= 200) {
                    palette.query = t;
                }
            }
        }
        self.palette = Some(palette);
        let mut tasks = vec![widget::operation::focus(palette_view::INPUT_ID)];
        if mode == Mode::Search {
            // A query taken from the selection comes up selected: typing replaces it.
            tasks.push(widget::operation::select_all(palette_view::INPUT_ID));
        }
        if mode == Mode::Files && (self.file_index.is_none() || self.index_stale) && !self.index_building {
            self.index_building = true;
            let roots = self.ws.root_paths();
            tasks.push(Task::perform(
                async move { Arc::new(palette::index::build(&roots)) },
                Message::IndexBuilt,
            ));
        }
        tasks.push(self.refresh_palette());
        Task::batch(tasks)
    }

    /// Recomputes the result list for the current query. Commands are ranked
    /// inline (tiny list); files on a worker, tagged so stale answers are dropped.
    fn refresh_palette(&mut self) -> Task<Message> {
        self.palette_seq += 1;
        let seq = self.palette_seq;
        let defaults = self.default_file_items();
        let hex_active = matches!(self.tabs.active().map(|t| &t.doc), Some(Doc::Hex(_)));
        let Some(p) = &mut self.palette else { return Task::none() };
        p.selected = 0;
        match p.mode {
            Mode::Commands => {
                p.items = palette::rank_commands(&p.query).into_iter().map(Item::command).collect();
                Task::none()
            }
            Mode::Search => {
                let query = p.search_query();
                p.clear_results();
                p.status.clear();
                let re = match query.compile() {
                    Ok(Some(re)) => re,
                    Ok(None) => return Task::none(),
                    Err(e) => {
                        p.status = e;
                        return Task::none();
                    }
                };
                let roots = match &p.scope {
                    Some(dir) => vec![dir.clone()],
                    None => self.ws.root_paths(),
                };
                if roots.is_empty() {
                    return Task::none(); // the panel explains how to add a folder
                }
                p.status = "Searching...".to_owned();
                let filter = p.filter.clone();
                let generation = self.search_gen.clone();
                generation.store(seq, std::sync::atomic::Ordering::Relaxed);
                Task::perform(
                    async move {
                        let stale = || generation.load(std::sync::atomic::Ordering::Relaxed) != seq;
                        Arc::new(project_search::run(&roots, &filter, &re, stale))
                    },
                    move |outcome| Message::SearchResults { seq, outcome },
                )
            }
            Mode::Files => {
                if let Some(rest) = p.query.strip_prefix(':') {
                    let rest = rest.trim();
                    let n = match rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")) {
                        Some(h) if hex_active => usize::from_str_radix(h, 16).ok(),
                        _ => rest.parse::<usize>().ok(),
                    };
                    p.items = n.filter(|n| hex_active || *n > 0).map(|n| if hex_active { Item::offset(n) } else { Item::line(n) }).into_iter().collect();
                    return Task::none();
                }
                let Some(index) = self.file_index.clone() else {
                    p.items.clear(); // still indexing
                    return Task::none();
                };
                if p.query.trim().is_empty() {
                    p.items = defaults;
                    return Task::none();
                }
                let query = p.query.clone();
                Task::perform(
                    async move { palette::rank_files(&index, &query, palette::LIMIT) },
                    move |hits| Message::PaletteResults { seq, hits },
                )
            }
        }
    }

    /// Length of the area the active tab's two panes share, along the split's axis (px).
    fn split_extent(&self) -> f32 {
        let dir = self.tabs.active().and_then(|t| t.split.as_ref()).map_or(SplitDir::Right, |s| s.dir);
        match dir {
            SplitDir::Right => {
                let left = if self.sidebar_visible { self.sidebar_width.clamp(sidebar::MIN_WIDTH, sidebar::MAX_WIDTH) + 5.0 } else { 0.0 };
                let md = self.settings.markdown_preview
                    && matches!(self.tabs.active().map(|t| &t.doc), Some(Doc::Text(d)) if d.hl.lang == Some(Lang::Markdown));
                let share = if md { 1.0 - self.preview_frac } else { 1.0 };
                ((self.window_size.width - left) * share - 5.0).max(100.0)
            }
            // Menu bar, tab strip, status bar (and the find bar when it is open).
            SplitDir::Down => (self.window_size.height - menu_bar::HEIGHT - 33.0 - 26.0 - if self.find.open { 40.0 } else { 0.0 } - 5.0).max(100.0),
        }
    }

    /// Empty query: open tabs first, then the index in path order.
    fn default_file_items(&self) -> Vec<Item> {
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        for tab in self.tabs.iter() {
            if let Some(path) = &tab.path {
                seen.insert(path.clone());
                items.push(Item {
                    title: tab.title.clone(),
                    detail: path.parent().map(|d| d.display().to_string()).unwrap_or_default(),
                    target: Target::File(path.clone()),
                });
            }
        }
        for f in self.file_index.iter().flat_map(|i| i.iter()) {
            if items.len() >= palette::LIMIT {
                break;
            }
            if !seen.contains(&f.path) {
                items.push(Item::file(f));
            }
        }
        items
    }

    fn run_target(&mut self, target: Option<Target>) -> Task<Message> {
        self.palette = None;
        match target {
            Some(Target::File(path)) => self.open_path(path),
            Some(Target::Command(cmd)) => self.update(command_message(cmd)),
            Some(Target::Location { path, line, col, len }) => self.update(Message::OpenAt { path, line, col, len }),
            Some(Target::Line(n)) => self.update(Message::GotoLine(n)),
            None => self.settle(),
        }
    }

    /// Visible directories + parents of open files.
    fn watch_set(&self) -> watcher::WatchSet {
        let parents = self
            .tabs
            .iter()
            .filter_map(|t| t.path.as_deref()?.parent().map(Path::to_path_buf));
        watcher::WatchSet::new(self.ws.watched_dirs().into_iter().chain(parents))
    }

    /// Debounced batch of changed paths from the watcher.
    fn fs_changed(&mut self, paths: Vec<PathBuf>) -> Task<Message> {
        self.index_stale = true;
        let mut tasks = Vec::new();

        // Sidebar: rescan every shown directory that contains a changed path
        // (or is itself the changed path).
        let mut dirs: Vec<PathBuf> = Vec::new();
        for p in &paths {
            for cand in [Some(p.as_path()), p.parent()].into_iter().flatten() {
                if self.ws.is_loaded(cand) && !dirs.iter().any(|d| d == cand) {
                    dirs.push(cand.to_path_buf());
                }
            }
        }
        tasks.extend(dirs.into_iter().map(scan));

        // Open documents: re-read what changed; `disk_changed` decides.
        for tab in self.tabs.iter() {
            if matches!(&tab.doc, Doc::Text(doc) if doc.just_saved()) {
                continue; // the echo of our own write
            }
            if let Some(path) = &tab.path {
                if paths.iter().any(|p| p == path) {
                    tasks.push(reload_task(tab.id, path.clone(), false));
                }
            }
        }
        Task::batch(tasks)
    }

    fn disk_changed(
        &mut self,
        id: TabId,
        force: bool,
        result: Result<Handoff<Loaded>, String>,
    ) -> Task<Message> {
        let is_active = self.tabs.active_id() == Some(id);
        let Some(tab) = self.tabs.get_mut(id) else { return Task::none() };
        let name = tab.title.clone();

        let loaded = match result.map(|h| h.take()) {
            Ok(Some(l)) => l,
            Ok(None) => return Task::none(),
            Err(e) => {
                self.notice = Some(e); // (a deleted file says so: see `loader::reload`)
                return Task::none();
            }
        };

        let note: String;
        match (&mut tab.doc, loaded.body) {
            (Doc::Text(doc), Body::Text(text)) => {
                if !force && loaded.disk_hash == doc.disk_hash {
                    return Task::none(); // our own save, or a touch without changes
                }
                if doc.dirty() && !force {
                    doc.conflict = true;
                    note = format!("{name} changed on disk - your unsaved edits are kept");
                } else if text.is_empty() && doc.state.len_chars() > 0 && !force {
                    // A file that suddenly reads as empty (a save cut short, a network mount that
                    // lost its connection) is not taken over: the text in the buffer may be the
                    // only copy left. It counts as unsaved so that Ctrl+S writes it back.
                    doc.conflict = true;
                    doc.force_dirty = true;
                    doc.disk_hash = loaded.disk_hash;
                    note = format!("{name} is empty on disk - your text is kept, Ctrl+S writes it back");
                } else {
                    doc.replace(&text, loaded.disk_hash);
                    note = format!("Reloaded {name} (changed on disk) - Ctrl+Z undoes it");
                }
            }
            (Doc::Hex(h), _) if !force && h.recently_saved() => return Task::none(), // our own patch
            (slot, body) => {
                if matches!(slot, Doc::Hex(h) if h.dirty() && !force) {
                    note = format!("{name} changed on disk - your unsaved byte edits are kept");
                } else if matches!(slot, Doc::Text(d) if d.dirty() && !force) {
                    if let Doc::Text(d) = slot {
                        d.conflict = true;
                    }
                    note = format!("{name} changed on disk - your unsaved edits are kept");
                } else {
                    *slot = match body {
                        Body::Text(t) => Doc::Text(
                            TextDoc::new(&t, false, loaded.disk_hash)
                                .with_lang(tab.path.as_deref().and_then(|p| Lang::detect_text(p, &t))),
                        ),
                        Body::Hex(h) => Doc::Hex(h),
                        Body::Image(i) => Doc::Image(i),
                    };
                    note = format!("Reloaded {name} (changed on disk)");
                }
            }
        }
        self.notice = Some(note);
        self.touch();
        if is_active { self.settle() } else { Task::none() }
    }

    fn touch(&mut self) {
        self.session_dirty = true;
    }

    /// Starts writing `text` (what `text_to_save` returned) to `path`.
    fn write(&mut self, id: TabId, path: PathBuf, text: String, version: u64, then_close: bool) -> Task<Message> {
        if let Some(Doc::Text(doc)) = self.tabs.get_mut(id).map(|t| &mut t.doc) {
            doc.saving = true;
        }
        write_task(id, path, text, version, then_close)
    }

    /// What goes to disk, at `path`, for text tab `id`, with the document's version it
    /// belongs to. With "trim on save" the blanks at the ends of the lines are removed from
    /// the buffer first (one undo step, like any edit), so what is written is what the buffer
    /// holds. A Markdown file (by its *new* name: Save As may rename it) keeps its hard line
    /// breaks, the two spaces at the end of a line.
    fn text_to_save(&mut self, id: TabId, path: &Path) -> Option<(String, u64)> {
        let trim = self.settings.trim_trailing_whitespace;
        let markdown = Lang::detect(path) == Some(Lang::Markdown);
        let Doc::Text(doc) = &mut self.tabs.get_mut(id)?.doc else { return None };
        let trimmed = if trim { doc.trim_trailing_whitespace(markdown) } else { 0 };
        let saved = (doc.text(), doc.version());
        if trimmed > 0 {
            self.touch();
            self.trimmed = Some((id, trimmed));
        }
        Some(saved)
    }

    /// Runs after anything that swaps the widget tree under the active tab.
    /// iced executes these operations *after* rebuilding the UI, so they see
    /// the new widgets. Needed because a `scrollable` keeps its own offset
    /// (new/reused widget state) while our model keeps the authoritative one.
    fn settle(&mut self) -> Task<Message> {
        let ui = match self.tabs.active().map(|t| &t.doc) {
            Some(Doc::Text(_)) if self.palette.is_none() && self.dialog.is_none() && self.prompt.is_none() => {
                let second = self.tabs.active().and_then(|t| t.split.as_ref()).is_some_and(|s| s.focus == Pane::Second);
                widget::operation::focus(if second { text_view::EDITOR2_ID } else { text_view::EDITOR_ID })
            }
            Some(Doc::Hex(_)) if self.palette.is_none() && self.dialog.is_none() && self.prompt.is_none() && !self.settings_open => {
                widget::operation::focus(hex_view::HEX_ID)
            }
            _ => Task::none(),
        };
        let preview = self.preview_task(true);
        let tab = self.reveal_active_tab();
        Task::batch([ui, preview, tab, self.start_analysis()])
    }

    /// Background analysis of the active buffer, on workers: syntax parse and
    /// git diff against `HEAD`. Only the visible tab is analysed; others catch
    /// up when activated. The (O(n)) text copy is taken once for both jobs.
    fn start_analysis(&mut self) -> Task<Message> {
        Task::batch([self.start_doc_analysis(), self.start_find()])
    }

    fn start_doc_analysis(&mut self) -> Task<Message> {
        let Some(id) = self.tabs.active_id() else { return Task::none() };
        let Some(tab) = self.tabs.get_mut(id) else { return Task::none() };
        let path = tab.path.clone();
        let Doc::Text(doc) = &mut tab.doc else { return Task::none() };
        let mut tasks = Vec::new();

        if doc.git.wants_base() {
            match path {
                Some(path) => {
                    doc.git.base = Base::Loading;
                    tasks.push(Task::perform(
                        async move {
                            let file = path.clone();
                            let base = fsx::off_thread(move || git::base_text(&file)).await.flatten().map(Arc::new);
                            (path, base)
                        },
                        move |(path, base)| Message::GitBase { id, path, base },
                    ));
                }
                None => doc.git.base = Base::Absent,
            }
        }

        let version = doc.version();
        let (hl, gt) = (doc.hl.wants(version), doc.git.wants_diff(version));
        if !hl && !gt {
            return Task::batch(tasks);
        }
        if doc.state.len_bytes() > syntax::MAX_BYTES {
            // Too big to analyse: decided without copying the buffer.
            doc.hl.disabled = true;
            doc.hl.spans = None;
            doc.git.base = Base::Absent;
            doc.git.marks = None;
            return Task::batch(tasks);
        }
        let text = Arc::new(doc.text());
        if let (true, Some(lang)) = (hl, doc.hl.lang) {
            doc.hl.in_flight = true;
            doc.hl.requested = version;
            let (prev, text) = (doc.hl.parse.clone(), text.clone());
            tasks.push(Task::perform(
                async move { syntax::highlight(lang, prev.as_ref(), text).map(|(p, s)| (p, Arc::new(s))) },
                move |result| Message::Highlighted { id, lang, result },
            ));
        }
        if let (true, Base::Ready(base)) = (gt, &doc.git.base) {
            doc.git.in_flight = true;
            doc.git.requested = version;
            let base = base.clone();
            tasks.push(Task::perform(
                async move { Arc::new(git::diff(&base, &text)) },
                move |marks| Message::GitMarksReady { id, marks },
            ));
        }
        Task::batch(tasks)
    }

    fn wants_highlight(&self) -> bool {
        let id = self.tabs.active_id().unwrap_or(0);
        matches!(
            self.tabs.active().map(|t| &t.doc),
            Some(Doc::Text(d)) if d.hl.wants(d.version())
                || d.git.wants_diff(d.version())
                || d.git.wants_base()
                || self.find.wants_search(id, d.version())
        )
    }

    // -------------------------------------------------------------------- find

    /// Searches the active buffer on a worker when the results are stale.
    fn start_find(&mut self) -> Task<Message> {
        if !self.find.open {
            return Task::none();
        }
        let Some(id) = self.tabs.active_id() else { return Task::none() };
        let Some(Doc::Text(doc)) = self.tabs.get(id).map(|t| &t.doc) else { return Task::none() };
        let version = doc.version();
        if !self.find.wants_search(id, version) {
            return Task::none();
        }
        let key = self.find.key(id, version);
        if self.find.query.text.is_empty() {
            self.find.matches = Arc::default();
            self.find.matches_key = Some(key);
            self.find.error = None;
            self.find.jump = false;
            return Task::none();
        }
        self.find.requested = Some(key.clone());
        let (text, query) = (doc.text(), self.find.query.clone());
        Task::perform(
            async move {
                match query.compile() {
                    Ok(Some(re)) => (Arc::new(search::find_all(&text, &re)), None),
                    Ok(None) => (Arc::default(), None),
                    Err(e) => (Arc::default(), Some(e)),
                }
            },
            move |(matches, error)| Message::FindResults { key: key.clone(), matches, error },
        )
    }

    /// The up-to-date matches of the active buffer, if any.
    fn live_matches(&self) -> Option<(TabId, u64, &[(usize, usize)])> {
        let tab = self.tabs.active()?;
        let Doc::Text(doc) = &tab.doc else { return None };
        self.find.valid_matches(tab.id, doc.version()).map(|m| (tab.id, doc.version(), m))
    }

    /// Selects the first match at or after the selection start (incremental search).
    fn find_jump(&mut self) {
        let Some(Doc::Text(doc)) = self.tabs.active_id().and_then(|id| self.tabs.get_mut(id)).map(|t| &mut t.doc) else { return };
        let matches = self.find.matches.clone();
        if matches.is_empty() {
            return;
        }
        let from = doc.state.selection().range().0;
        let i = matches.partition_point(|m| m.0 < from);
        let (a, b) = matches[if i == matches.len() { 0 } else { i }];
        doc.state.select_range(a, b);
    }

    fn find_step(&mut self, forward: bool) -> Task<Message> {
        if self.live_matches().is_none() {
            self.find.jump = true; // results pending: select once they arrive
            return self.start_analysis();
        }
        let matches = self.find.matches.clone();
        let Some(Doc::Text(doc)) = self.tabs.active_id().and_then(|id| self.tabs.get_mut(id)).map(|t| &mut t.doc) else {
            return Task::none();
        };
        match find::step(&matches, doc.state.selection().range(), forward) {
            Some(i) => {
                let (a, b) = matches[i];
                doc.state.select_range(a, b);
                self.notice = None;
            }
            None => self.notice = Some("No results".to_owned()),
        }
        Task::none()
    }

    /// Replace (selected match, then on to the next) or Replace All.
    fn replace(&mut self, all: bool) -> Task<Message> {
        let Some(id) = self.tabs.active_id() else { return Task::none() };
        let Some(Doc::Text(doc)) = self.tabs.get(id).map(|t| &t.doc) else { return Task::none() };
        let version = doc.version();
        let (query, replacement) = (self.find.query.clone(), self.find.replacement.clone());
        let literal = !query.regex;
        let at = if all {
            None
        } else {
            let sel = doc.state.selection().range();
            match self.live_matches().and_then(|(_, _, m)| find::current(m, sel)) {
                Some(_) => Some(sel.0),
                None => return self.find_step(true), // nothing selected to replace yet
            }
        };
        let text = doc.text();
        Task::perform(
            async move {
                let Ok(Some(re)) = query.compile() else { return (None, 0) };
                match at {
                    None => search::replace_all(&text, &re, &replacement, literal)
                        .map_or((None, 0), |(e, n)| (Some(e), n)),
                    Some(at) => (search::replace_at(&text, &re, at, &replacement, literal), 1),
                }
            },
            move |(edit, count)| Message::Replaced { id, version, edit, count, then_next: !all },
        )
    }

    /// Writes a hex tab's edits into the file in place (worker), then reports back.
    fn save_hex(&mut self, id: TabId, then_close: bool) -> Task<Message> {
        let Some(tab) = self.tabs.get(id) else { return Task::none() };
        let (Some(path), Doc::Hex(h)) = (tab.path.clone(), &tab.doc) else { return Task::none() };
        if !h.dirty() {
            return if then_close { self.close_now(id) } else { Task::none() };
        }
        let edits = h.edits().clone();
        let saved: Arc<Vec<(u64, u8)>> = Arc::new(edits.iter().map(|(&o, &b)| (o, b)).collect());
        Task::perform(
            async move {
                let shown = path.clone();
                fsx::off_thread(move || hex::write_patches(&path, &edits).map_err(|e| format!("{}: {e}", path.display())))
                    .await
                    .unwrap_or_else(|| Err(format!("{}: saving failed", shown.display())))
            },
            move |result| Message::HexSaved { id, saved: saved.clone(), then_close, result },
        )
    }

    /// Selects the pending search hit in the (now active) tab.
    fn apply_goto(&mut self) {
        let Some((path, line, col, len)) = self.goto.take() else { return };
        let Some(tab) = self.tabs.active() else { return };
        if tab.path.as_deref() != Some(path.as_path()) {
            return;
        }
        let id = tab.id;
        if let Some(Doc::Text(doc)) = self.tabs.get_mut(id).map(|t| &mut t.doc) {
            let line = line.min(doc.state.line_count().saturating_sub(1));
            let start = doc.state.line_start(line) + col;
            doc.state.select_range(start, start + len);
        }
    }

    /// Applies a new font size at once; the settings file is written when the zoom settles.
    fn set_font_size(&mut self, wanted: f32) -> Task<Message> {
        let old = self.settings.font_size();
        let new = wanted.clamp(8.0, 40.0);
        if (new - old).abs() < f32::EPSILON {
            return Task::none();
        }
        self.settings.font_size = new;
        // Line height follows the font: keep the same lines at the top of every text view.
        let line_h = |f: f32| (f * 1.4).round();
        let factor = line_h(new) / line_h(old);
        for tab in self.tabs.iter_mut() {
            if let Doc::Text(doc) = &mut tab.doc {
                doc.state.rescale_scroll(factor);
            }
            if let Some(s) = &mut tab.split {
                s.mirror.rescale_scroll(factor);
            }
        }
        self.font_dirty = true;
        self.touch(); // wakes the flush timer that writes it
        Task::none()
    }

    /// A path from outside (another launch, drag and drop): folders join the workspace, files open.
    fn open_external(&mut self, path: PathBuf) -> Task<Message> {
        if fsx::is_dir(&path) == Some(true) {
            if self.ws.add_root(path.clone(), true) {
                self.index_stale = true;
                self.touch();
                return scan(path);
            }
            Task::none()
        } else {
            self.open_path(path)
        }
    }

    /// Opens a file in a tab of its own (a preview tab of it becomes a real tab).
    fn open_path(&mut self, path: PathBuf) -> Task<Message> {
        if let Some(id) = self.tabs.by_path(&path) {
            self.tabs.activate(id);
            if let Some(tab) = self.tabs.get_mut(id) {
                tab.preview = false;
            }
            self.touch();
            return self.settle();
        }
        open_task(path)
    }

    /// A single sidebar click: shows the file in the preview tab (a file already open is
    /// just brought to the front, whatever kind of tab it is in).
    fn open_path_preview(&mut self, path: PathBuf) -> Task<Message> {
        if let Some(id) = self.tabs.by_path(&path) {
            self.tabs.activate(id);
            self.touch();
            return self.settle();
        }
        Task::perform(loader::open_off(path), |r| Message::OpenedPreview(r.map(Handoff::new)))
    }

    /// A file arrived from disk: show it. As a preview it takes the place of the previous
    /// preview tab (unless that holds unsaved work); a file that is already open stays as it is.
    fn opened(&mut self, loaded: Loaded, preview: bool) -> Task<Message> {
        let existing = loaded.path.as_deref().and_then(|p| self.tabs.by_path(p));
        let replaced = if preview && existing.is_none() { self.tabs.replaceable_preview() } else { None };
        let at = replaced.and_then(|id| self.tabs.index_of(id));
        let id = self.install(loaded, at);
        if let Some(tab) = self.tabs.get_mut(id) {
            if existing.is_none() {
                tab.preview = preview;
            } else if !preview {
                tab.preview = false;
            }
        }
        if let Some(old) = replaced {
            self.tabs.close(old);
        }
        self.tabs.activate(id);
        self.touch();
        self.apply_goto();
        self.settle()
    }

    /// The folder a file dialog opens in: where the user last picked or saved a file; before
    /// that the active file's folder, then the first workspace folder. (Without one the
    /// desktop shows its "Recent" page.)
    fn dialog_dir(&self) -> Option<PathBuf> {
        self.last_dir
            .clone()
            .filter(|d| fsx::is_dir(d) == Some(true))
            .or_else(|| self.tabs.active().and_then(|t| t.path.as_deref()).and_then(Path::parent).map(Path::to_path_buf))
            .or_else(|| self.ws.root_paths().into_iter().next())
            .filter(|d| fsx::is_dir(d) == Some(true))
    }

    /// Remembers the folder of a path the user just picked in a dialog.
    fn remember_dir(&mut self, picked: Option<&PathBuf>) {
        if let Some(dir) = picked.and_then(|p| p.parent()).filter(|d| !d.as_os_str().is_empty()) {
            if self.last_dir.as_deref() != Some(dir) {
                self.last_dir = Some(dir.to_path_buf());
                self.touch();
            }
        }
    }

    /// Turns a loaded file into a tab (re-uses an existing tab for the same path).
    fn install(&mut self, loaded: Loaded, index: Option<usize>) -> TabId {
        if let Some(id) = loaded.path.as_deref().and_then(|p| self.tabs.by_path(p)) {
            return id;
        }
        if loaded.notice.is_some() {
            self.notice = loaded.notice;
        }
        let doc = match loaded.body {
            Body::Text(text) => Doc::Text(
                TextDoc::new(&text, loaded.dirty, loaded.disk_hash)
                    .with_lang(loaded.path.as_deref().and_then(|p| Lang::detect_text(p, &text))),
            ),
            Body::Hex(hex) => Doc::Hex(hex),
            Body::Image(img) => Doc::Image(img),
        };
        self.tabs.insert(index, loaded.path, doc)
    }

    fn close_now(&mut self, id: TabId) -> Task<Message> {
        self.tabs.close(id);
        if matches!(self.dialog, Some(Dialog::CloseDirty(d) | Dialog::Conflict { id: d, .. }) if d == id) {
            self.dialog = None;
        }
        self.touch();
        let next = self.pump_close_queue();
        Task::batch([self.settle(), next])
    }

    /// Closes the given tabs; a tab with unsaved changes asks first (and the
    /// rest waits for the answer).
    fn close_many(&mut self, ids: Vec<TabId>) -> Task<Message> {
        self.close_queue = ids;
        let t = self.pump_close_queue();
        Task::batch([t, self.settle()])
    }

    fn pump_close_queue(&mut self) -> Task<Message> {
        if self.dialog.is_some() {
            return Task::none();
        }
        while !self.close_queue.is_empty() {
            let id = self.close_queue.remove(0);
            match self.tabs.get(id).map(|t| t.is_dirty()) {
                None => {}
                Some(false) => {
                    self.tabs.close(id);
                    self.touch();
                }
                Some(true) => {
                    self.tabs.activate(id);
                    self.dialog = Some(Dialog::CloseDirty(id));
                    break;
                }
            }
        }
        Task::none()
    }

    /// Right-click menu of a tab.
    fn tab_entries(&self, id: TabId) -> Vec<Entry> {
        let Some(tab) = self.tabs.get(id) else { return Vec::new() };
        let pos = self.tabs.iter().position(|t| t.id == id).unwrap_or(0);
        let n = self.tabs.iter().count();
        let mut e = vec![
            Entry::item("Close", "Ctrl+W", Message::CloseTab(id)),
            Entry::item("Close Others", "", Message::CloseOthers(id)).when(n > 1),
            Entry::item("Close Tabs to the Right", "", Message::CloseRight(id)).when(pos + 1 < n),
            Entry::item("Close All", "", Message::CloseAll),
        ];
        if matches!(tab.doc, Doc::Text(_)) {
            let dir = tab.split.as_ref().map(|s| s.dir);
            e.push(Entry::Sep);
            e.push(Entry::toggle("Split Right", dir == Some(SplitDir::Right), Message::SplitTab(id, SplitDir::Right)));
            e.push(Entry::toggle("Split Down", dir == Some(SplitDir::Down), Message::SplitTab(id, SplitDir::Down)));
            if dir.is_some() {
                e.push(Entry::item("Close Split", "", Message::CloseSplit(id)));
            }
        }
        if let Some(path) = &tab.path {
            e.push(Entry::Sep);
            e.push(Entry::item("Copy File Path", "", Message::CopyText(path.to_string_lossy().into_owned())));
            e.push(Entry::item("Reveal in File Manager", "", Message::Reveal(path.clone())));
            e.push(Entry::item("Reload from Disk", "", Message::ReloadFromDisk(id)));
        }
        e
    }

    // ----------------------------------------------------------------- session

    fn flush_session(&mut self) -> Task<Message> {
        if !self.restored || !self.session_dirty {
            return Task::none();
        }
        self.session_dirty = false;
        let snapshot = self.snapshot();
        Task::perform(
            async move {
                if let Err(e) = snapshot.write().await {
                    eprintln!("veloedit: could not save session: {e}");
                }
            },
            |_| Message::Noop,
        )
    }

    /// Captures state; only drafts edited since the last flush are re-read.
    fn snapshot(&mut self) -> Snapshot {
        let mut drafts = Vec::new();
        let mut records = Vec::new();
        for tab in self.tabs.iter_mut() {
            let (kind, draft) = match &mut tab.doc {
                Doc::Text(doc) => {
                    let draft = (doc.dirty() || tab.path.is_none()).then(|| {
                        let name = session::draft_name(tab.id);
                        if doc.version() != doc.flushed_version {
                            drafts.push((name.clone(), doc.text()));
                            doc.flushed_version = doc.version();
                        }
                        name
                    });
                    (TabKind::Text, draft)
                }
                Doc::Hex(_) => (TabKind::Hex, None),
                Doc::Image(_) => (TabKind::Image, None),
            };
            let hex_edits = match &tab.doc {
                Doc::Hex(h) => h.edits().iter().map(|(&o, &b)| (o, b)).collect(),
                _ => Vec::new(),
            };
            records.push(TabRecord { kind, path: tab.path.clone(), draft, hex_edits });
        }
        Snapshot {
            file: SessionFile {
                sidebar_visible: self.sidebar_visible,
                window: self.window_state,
                sidebar_width: self.sidebar_width,
                preview_frac: self.preview_frac,
                last_dir: self.last_dir.clone(),
                roots: self.ws.root_paths(),
                expanded: self.ws.expanded_paths(),
                tabs: records,
                active: self.tabs.active_index(),
                ..SessionFile::default()
            },
            drafts,
        }
    }

    /// "3 of 12" / "No results" for the find bar.
    fn find_status(&self) -> String {
        if self.find.query.text.is_empty() {
            return String::new();
        }
        let Some((_, _, matches)) = self.live_matches() else { return "...".to_owned() };
        let Some(Doc::Text(doc)) = self.tabs.active().map(|t| &t.doc) else { return String::new() };
        match (matches.len(), find::current(matches, doc.state.selection().range())) {
            (0, _) => "No results".to_owned(),
            (n, Some(i)) => format!("{} of {n}", i + 1),
            (n, None) if n >= search::MAX_MATCHES => format!("{n}+ results"),
            (n, None) => format!("{n} results"),
        }
    }

    // -------------------------------------------------------------------- view

    pub fn view(&self) -> Element<'_, Message> {
        if !self.first_view_logged.replace(true) && std::env::var_os("VELOEDIT_TIMING").is_some() {
            if let Some(t0) = START.get() {
                eprintln!("[timing] first view() after {:?}", t0.elapsed());
            }
        }

        let c = self.settings.colors();
        let modal = self.modal_open() || self.popup.is_some();
        let active = self.tabs.active();
        let content: Element<'_, Message> = match active.map(|t| (t.id, &t.doc)) {
            Some((id, Doc::Text(doc))) => {
                let matches = self.find.open.then(|| self.find.valid_matches(id, doc.version())).flatten();
                let md_split = self.settings.markdown_preview && doc.hl.lang == Some(Lang::Markdown);
                // The source half of a Markdown split is narrow and its lines (table rows!) are
                // long: always wrap it.
                let wrap = self.settings.word_wrap || md_split;
                let tab = active.expect("a text document is shown by an active tab");
                let editor = match &tab.split {
                    None => {
                        let pick = text_view::Pick { state: &doc.state, pane: None, minimap: self.settings.minimap };
                        text_view::view(c, self.settings.font_size(), modal, wrap, id, doc, pick, matches)
                    }
                    Some(s) => {
                        let pane = |pane: Pane| {
                            let state = tab.pane_state(pane).expect("a split tab is a text tab");
                            let header = tab_bar::pane_header(c, tab, pane, s.focus == pane);
                            // Two minimaps would eat half of a side-by-side layout.
                            let minimap = self.settings.minimap && (pane == Pane::First || s.dir == SplitDir::Down);
                            let pick = text_view::Pick { state, pane: Some(pane), minimap };
                            let editor = text_view::view(c, self.settings.font_size(), modal, wrap, id, doc, pick, matches);
                            column![header, editor].into()
                        };
                        split_panes(c, s.dir, s.frac, pane(Pane::First), pane(Pane::Second))
                    }
                };
                if md_split {
                    // Source on the left, rendered document on the right; the divider is draggable.
                    let share = |f: f32| Length::FillPortion((f * 1000.0).round().max(1.0) as u16);
                    let line = container(space().width(1.0).height(Length::Fill)).style(theme::panel(c, c.border));
                    let handle = mouse_area(
                        container(line)
                            .padding(iced::Padding { left: 2.0, right: 2.0, ..iced::Padding::ZERO })
                            .width(5.0)
                            .height(Length::Fill),
                    )
                    .on_press(Message::PreviewResizeStart)
                    .interaction(mouse::Interaction::ResizingHorizontally);
                    row![
                        container(editor).width(share(1.0 - self.preview_frac)).height(Length::Fill),
                        handle,
                        container(markdown_view::view(c, self.settings.font_size(), doc.preview.as_ref(), self.preview_width()))
                            .width(share(self.preview_frac))
                            .height(Length::Fill),
                    ]
                    .into()
                } else {
                    editor
                }
            }
            Some((id, Doc::Hex(doc))) => hex_view::HexView::new(doc, c, self.settings.font_size(), move |a| Message::Hex(id, a))
                .id(hex_view::HEX_ID)
                .modal(modal)
                .into(),
            Some((_, Doc::Image(doc))) => image_view::view(c, doc),
            None => welcome(c),
        };
        let mut main = column![tab_bar::view(c, &self.tabs, self.tab_drag.as_ref().filter(|d| d.live).map(|d| tab_bar::Drag { id: d.id, slot: d.slot }))].width(Length::Fill).height(Length::Fill);
        if self.find.open && matches!(active.map(|t| &t.doc), Some(Doc::Text(_))) {
            main = main.push(find_bar::view(c, &self.find, self.find_status()));
        }
        let main = main.push(content);

        let body: Element<'_, Message> = if self.sidebar_visible {
            let active_path = active.and_then(|t| t.path.as_deref());
            row![sidebar::view(c, &self.ws, active_path, self.sidebar_scroll, self.sidebar_width), main].into()
        } else {
            main.into()
        };

        let mut page_col = column![];
        if self.settings.menu_bar {
            let open = match self.popup {
                Some(Popup::Menu(i)) => Some(i),
                _ => None,
            };
            page_col = page_col.push(menu_bar::view(c, open));
        }
        let page = container(page_col.push(body).push(status_bar::view(c, active, self.notice.as_deref())))
            .style(theme::panel(c, c.bg));

        // One flat stack: the page is always layer 0 at the same depth. Nesting a new
        // `stack!` around it for every overlay changes the root widget's type, and iced
        // then throws away the state of everything below (tab strip and sidebar scroll
        // offsets, editor focus) each time a menu opens or closes.
        let mut layers: Vec<Element<'_, Message>> = vec![page.into()];
        match self.dialog {
            Some(Dialog::CloseDirty(id)) => {
                if let Some(tab) = self.tabs.get(id) {
                    layers.push(close_dialog(c, tab.id, tab.label(LONG_LABEL).into_owned()));
                }
            }
            Some(Dialog::Conflict { id, then_close }) => {
                if let Some(tab) = self.tabs.get(id) {
                    layers.push(conflict_dialog(c, tab.id, then_close, &tab.title));
                }
            }
            Some(Dialog::About) => layers.push(about_dialog(c)),
            None => {}
        }
        if self.settings_open {
            layers.push(settings_panel::view(c, &self.settings));
        }
        if let Some(p) = &self.prompt {
            layers.push(prompt_dialog(c, p));
        }
        match &self.palette {
            Some(p) if p.mode == Mode::Search => layers.push(search_panel::view(c, p, self.ws.is_empty())),
            Some(p) => layers.push(palette_view::view(c, p, self.index_building, self.ws.is_empty())),
            None => {}
        }
        match &self.popup {
            Some(Popup::Context { at, entries, width }) => layers.push(menu::popup(c, entries.clone(), *at, 0.0, *width)),
            Some(Popup::Menu(i)) if self.settings.menu_bar => layers.push(menu::popup(
                c,
                self.menu_entries(*i),
                Point::new(menu_bar::x_of(*i), menu_bar::HEIGHT),
                menu_bar::HEIGHT,
                menu::WIDTH,
            )),
            _ => {}
        }
        stack(layers).into()
    }
}

// ------------------------------------------------------------------- helpers

fn command_message(cmd: Command) -> Message {
    match cmd {
        Command::GoToFile => Message::OpenPalette(Mode::Files),
        Command::GoToLine => Message::OpenGoto,
        Command::FindInFiles => Message::OpenPalette(Mode::Search),
        Command::ToggleSidebar => Message::ToggleSidebar,
        Command::ToggleMarkdownPreview => Message::ToggleSetting(Toggle::MarkdownPreview),
        Command::ToggleTrimOnSave => Message::ToggleSetting(Toggle::TrimOnSave),
        Command::Fold => Message::EditAction(Action::Fold),
        Command::Unfold => Message::EditAction(Action::Unfold),
        Command::FoldAll => Message::EditAction(Action::FoldAll),
        Command::UnfoldAll => Message::EditAction(Action::UnfoldAll),
        Command::FoldLevel(n) => Message::EditAction(Action::FoldLevel(n)),
        Command::NewFile => Message::NewFile,
        Command::Save => Message::SaveActive,
        Command::CloseTab => Message::CloseActive,
        Command::OpenFile => Message::PickFiles,
        Command::AddFolder => Message::PickFolder,
        Command::ReloadFromDisk => Message::ReloadActive,
        Command::NextTab => Message::NextTab,
        Command::PrevTab => Message::PrevTab,
        Command::SplitRight => Message::SplitActive(SplitDir::Right),
        Command::SplitDown => Message::SplitActive(SplitDir::Down),
        Command::CloseSplit => Message::CloseSplitActive,
        Command::FontBigger => Message::ChangeFontSize(1),
        Command::FontSmaller => Message::ChangeFontSize(-1),
        Command::FontReset => Message::ResetFontSize,
        Command::OpenSettings => Message::OpenSettings,
        Command::SettingsPanel => Message::OpenSettingsPanel,
        Command::RefreshGit => Message::RefreshGit,
        Command::Find => Message::OpenFind { replace: false },
        Command::Replace => Message::OpenFind { replace: true },
        Command::FindNext => Message::FindNext,
        Command::FindPrev => Message::FindPrev,
        Command::SetTheme(name) => Message::SetTheme(name.to_owned()),
    }
}

fn persist_setting(key: &'static str, value: serde_json::Value) -> Task<Message> {
    Task::perform(async move { settings::set(key, value) }, Message::SettingsWritten)
}

/// While the find bar is open Esc closes it even if its text input consumed the key.
fn find_key(event: Event, _status: event::Status, _window: window::Id) -> Option<Message> {
    match event {
        Event::Keyboard(keyboard::Event::KeyPressed { key: keyboard::Key::Named(keyboard::key::Named::Escape), .. }) => {
            Some(Message::Escape)
        }
        _ => None,
    }
}

/// Keys while a modal (palette / dialog) is open. Subscribed with
/// `listen_with`, so it also sees keys a focused text input consumed.
fn modal_key(event: Event, _status: event::Status, _window: window::Id) -> Option<Message> {
    use keyboard::{Key, key::Named};
    let Event::Keyboard(keyboard::Event::KeyPressed { key, modified_key, physical_key, modifiers, .. }) = event else {
        return None;
    };
    match key.as_ref() {
        Key::Named(Named::Escape) => Some(Message::Escape),
        Key::Named(Named::Tab) => Some(Message::SearchTab(modifiers.shift())),
        Key::Named(Named::ArrowDown) => Some(Message::PaletteMove(1)),
        Key::Named(Named::ArrowUp) => Some(Message::PaletteMove(-1)),
        Key::Named(Named::PageDown) => Some(Message::PaletteMove(palette::VISIBLE as isize)),
        Key::Named(Named::PageUp) => Some(Message::PaletteMove(-(palette::VISIBLE as isize))),
        _ => match shortcut(&key, &modified_key, physical_key, modifiers) {
            Some(m @ Message::OpenPalette(_)) => Some(m),
            _ => None,
        },
    }
}

/// The program that opens a path or URL the way the desktop does.
#[cfg(not(windows))]
const DESKTOP_OPEN: &str = "xdg-open";
/// (Explorer opens a folder in a window, a file in its default program, a URL in the browser;
/// it exits with 1 even then, which nobody looks at.)
#[cfg(windows)]
const DESKTOP_OPEN: &str = "explorer.exe";

/// Hands a path or URL to the desktop (`xdg-open`, Explorer) without blocking the UI.
fn open_with_desktop(target: std::ffi::OsString) {
    std::thread::spawn(move || {
        let child = std::process::Command::new(DESKTOP_OPEN)
            .arg(&target)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            let _ = child.wait();
        }
    });
}

/// Two clicks within this time are a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(450);

fn open_task(path: PathBuf) -> Task<Message> {
    Task::perform(loader::open_off(path), |r| Message::Opened(r.map(Handoff::new)))
}

fn reload_task(id: TabId, path: PathBuf, force: bool) -> Task<Message> {
    Task::perform(loader::reload(path), move |result| Message::DiskChanged {
        id,
        force,
        result: result.map(Handoff::new),
    })
}

fn scan(path: PathBuf) -> Task<Message> {
    Task::perform(
        async move {
            let dir = path.clone();
            let entries = fsx::off_thread(move || workspace::read_dir(&dir)).await.unwrap_or_default();
            (path, entries)
        },
        |(path, entries)| Message::DirScanned { path, entries },
    )
}

fn write_task(id: TabId, path: PathBuf, text: String, version: u64, then_close: bool) -> Task<Message> {
    Task::perform(
        {
            let path = path.clone();
            async move {
                let shown = path.clone();
                fsx::off_thread(move || {
                    let hash = crate::editor::text::hash_bytes(text.as_bytes());
                    session::atomic_write(&path, text.as_bytes())
                        .map(|()| hash)
                        .map_err(|e| format!("{}: {e}", path.display()))
                })
                .await
                .unwrap_or_else(|| Err(format!("{}: saving failed", shown.display())))
            }
        },
        move |result| Message::Saved { id, path: path.clone(), version, then_close, result },
    )
}

fn scroll_to(id: &'static str, y: f32) -> Task<Message> {
    widget::operation::scroll_to(
        widget::Id::new(id),
        scrollable::AbsoluteOffset { x: Some(0.0), y: Some(y) },
    )
}

fn scroll_x_to(id: &'static str, x: f32) -> Task<Message> {
    widget::operation::scroll_to(widget::Id::new(id), scrollable::AbsoluteOffset { x: Some(x), y: Some(0.0) })
}

/// Starts a file dialog in `dir` when there is one.
fn with_dir(dialog: rfd::AsyncFileDialog, dir: Option<PathBuf>) -> rfd::AsyncFileDialog {
    match dir {
        Some(d) => dialog.set_directory(d),
        None => dialog,
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// App-wide keyboard shortcuts (Ctrl on Linux/Windows). Layout independent:
/// uses the Latin character of the physical key.
pub fn shortcut(
    key: &keyboard::Key,
    // The key with Shift applied. With Ctrl held, `key` is the *unshifted* character
    // (on a Turkish Q keyboard "+" arrives as Ctrl+Shift+"4"), so symbols are read from here.
    modified: &keyboard::Key,
    physical: keyboard::key::Physical,
    mods: keyboard::Modifiers,
) -> Option<Message> {
    use keyboard::{Key, key::Named};

    if let Key::Named(Named::F3) = key.as_ref() {
        return Some(if mods.shift() { Message::FindPrev } else { Message::FindNext });
    }
    if !mods.command() || mods.alt() {
        return None;
    }
    // The numpad's zoom keys go by the physical key: what they *type* depends on NumLock
    // (KP_0 is "0" with it on and Insert with it off) and on the layout.
    if let keyboard::key::Physical::Code(code) = physical {
        use keyboard::key::Code;
        match code {
            Code::Numpad0 => return Some(Message::ResetFontSize),
            Code::NumpadAdd => return Some(Message::ChangeFontSize(1)),
            Code::NumpadSubtract => return Some(Message::ChangeFontSize(-1)),
            _ => {}
        }
    }
    if let Key::Named(named) = key.as_ref() {
        return match named {
            Named::Tab if mods.shift() => Some(Message::PrevTab),
            Named::Tab | Named::PageDown => Some(Message::NextTab),
            Named::PageUp => Some(Message::PrevTab),
            _ => None,
        };
    }
    if let Key::Character(c) = modified.as_ref() {
        match c {
            "+" | "=" => return Some(Message::ChangeFontSize(1)),
            "-" | "_" => return Some(Message::ChangeFontSize(-1)),
            _ => {}
        }
    }
    match (key.to_latin(physical)?.to_ascii_lowercase(), mods.shift()) {
        ('b', false) => Some(Message::ToggleSidebar),
        ('+' | '=', _) => Some(Message::ChangeFontSize(1)),
        ('-' | '_', _) => Some(Message::ChangeFontSize(-1)),
        ('0', false) => Some(Message::ResetFontSize),
        ('s', false) => Some(Message::SaveActive),
        ('w', false) => Some(Message::CloseActive),
        ('n', false) => Some(Message::NewFile),
        ('q', false) => Some(Message::Quit),
        ('o', false) => Some(Message::PickFiles),
        ('o', true) => Some(Message::PickFolder),
        ('f', false) => Some(Message::OpenFind { replace: false }),
        ('f', true) => Some(Message::OpenPalette(Mode::Search)),
        ('g', false) => Some(Message::OpenGoto),
        (',', false) => Some(Message::OpenSettingsPanel),
        ('h', false) => Some(Message::OpenFind { replace: true }),
        ('p', false) => Some(Message::OpenPalette(Mode::Files)),
        ('p', true) => Some(Message::OpenPalette(Mode::Commands)),
        _ => None,
    }
}

/// The two panes of a split tab with a draggable divider between them.
fn split_panes<'a>(c: theme::C, dir: SplitDir, frac: f32, first: Element<'a, Message>, second: Element<'a, Message>) -> Element<'a, Message> {
    let share = |f: f32| Length::FillPortion((f * 1000.0).round().max(1.0) as u16);
    let rule = |w: Length, h: Length| container(space().width(w).height(h)).style(theme::panel(c, c.border));
    match dir {
        SplitDir::Right => {
            let handle = mouse_area(
                container(rule(Length::Fixed(1.0), Length::Fill))
                    .padding(iced::Padding { left: 2.0, right: 2.0, ..iced::Padding::ZERO })
                    .width(5.0)
                    .height(Length::Fill),
            )
            .on_press(Message::SplitResizeStart)
            .interaction(mouse::Interaction::ResizingHorizontally);
            row![
                container(first).width(share(frac)).height(Length::Fill),
                handle,
                container(second).width(share(1.0 - frac)).height(Length::Fill),
            ]
            .into()
        }
        SplitDir::Down => {
            let handle = mouse_area(
                container(rule(Length::Fill, Length::Fixed(1.0)))
                    .padding(iced::Padding { top: 2.0, bottom: 2.0, ..iced::Padding::ZERO })
                    .width(Length::Fill)
                    .height(5.0),
            )
            .on_press(Message::SplitResizeStart)
            .interaction(mouse::Interaction::ResizingVertically);
            column![
                container(first).width(Length::Fill).height(share(frac)),
                handle,
                container(second).width(Length::Fill).height(share(1.0 - frac)),
            ]
            .into()
        }
    }
}

fn welcome<'a>(c: theme::C) -> Element<'a, Message> {
    let hint = |keys: &'static str, what: &'static str| {
        row![
            text(keys).size(12).color(c.accent).width(120),
            text(what).size(12).color(c.dim),
        ]
    };
    container(
        column![
            text("VeloEdit").size(28).color(c.text_bright),
            space().height(12.0),
            hint("Ctrl+B", "Toggle sidebar"),
            hint("Ctrl+O", "Open file"),
            hint("Ctrl+Shift+O", "Add folder to workspace"),
            hint("Ctrl+P", "Go to file  \u{00B7}  Ctrl+Shift+P: commands"),
            hint("Ctrl+Shift+F", "Find in files"),
            hint("Ctrl+N", "New file"),
            hint("Ctrl+S", "Save"),
            hint("Ctrl+W", "Close tab"),
        ]
        .spacing(6)
        .align_x(Alignment::Start),
    )
    .center(Length::Fill)
    .style(theme::panel(c, c.bg))
    .into()
}

fn conflict_dialog<'a>(c: theme::C, id: TabId, then_close: bool, title: &'a str) -> Element<'a, Message> {
    let choice = move |k| Message::ConflictChoice(id, then_close, k);
    dialog(
        c,
        format!("\"{title}\" changed on disk"),
        "Someone else modified this file after you opened it. Saving now replaces their version.",
        row![
            button(text("Reload (discard my edits)").size(13))
                .padding([6, 14])
                .style(theme::secondary_button(c))
                .on_press(choice(ConflictChoice::Reload)),
            space::horizontal(),
            button(text("Cancel").size(13))
                .padding([6, 14])
                .style(theme::secondary_button(c))
                .on_press(choice(ConflictChoice::Cancel)),
            button(text("Overwrite").size(13))
                .padding([6, 18])
                .style(theme::primary_button(c))
                .on_press(choice(ConflictChoice::Overwrite)),
        ]
        .spacing(8)
        .into(),
    )
}

fn about_dialog<'a>(c: theme::C) -> Element<'a, Message> {
    let body = format!(
        "A millisecond-start, GPU-accelerated text editor written in Rust.\n\n\
         Version        {}  \u{00B7}  {} {}\n\
         Built with     iced (wgpu / tiny-skia), tree-sitter, ropey\n\
         Settings       {}\n\
         Session        {}\n\n\
         Ctrl+Shift+P opens the command palette; every command lists its shortcut.",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        settings::path().display(),
        session::state_dir().display(),
    );
    dialog(
        c,
        "VeloEdit".to_owned(),
        body,
        row![
            space::horizontal(),
            button(text("Close").size(13))
                .padding([6, 22])
                .style(theme::primary_button(c))
                .on_press(Message::Escape),
        ]
        .into(),
    )
}

fn close_dialog<'a>(c: theme::C, id: TabId, title: String) -> Element<'a, Message> {
    dialog(
        c,
        format!("Save changes to \"{title}\"?"),
        "Your changes will be lost if you don't save them.",
        row![
            button(text("Don't Save").size(13))
                .padding([6, 14])
                .style(theme::secondary_button(c))
                .on_press(Message::CloseChoice(id, CloseChoice::Discard)),
            space::horizontal(),
            button(text("Cancel").size(13))
                .padding([6, 14])
                .style(theme::secondary_button(c))
                .on_press(Message::CloseChoice(id, CloseChoice::Cancel)),
            button(text("Save").size(13))
                .padding([6, 18])
                .style(theme::primary_button(c))
                .on_press(Message::CloseChoice(id, CloseChoice::Save)),
        ]
        .spacing(8)
        .into(),
    )
}

fn prompt_dialog<'a>(c: theme::C, p: &'a Prompt) -> Element<'a, Message> {
    let (title, action) = match &p.kind {
        PromptKind::NewFile(dir) => (format!("New file in {}", display_name(dir)), "Create"),
        PromptKind::NewFolder(dir) => (format!("New folder in {}", display_name(dir)), "Create"),
        PromptKind::Rename(path) => (format!("Rename {}", display_name(path)), "Rename"),
        PromptKind::Trash(path) => (format!("Move \"{}\" to the Trash?", display_name(path)), "Move to Trash"),
    };
    let mut body = column![text(title).size(15).color(c.text_bright)].spacing(12);
    match &p.kind {
        PromptKind::Trash(_) => {
            body = body.push(text("You can restore it from the Trash later.").size(12).color(c.dim));
        }
        kind => {
            let hint = if matches!(kind, PromptKind::NewFile(_)) { "name.ext  (folders/name.ext works too)" } else { "name" };
            body = body.push(
                widget::text_input(hint, &p.text)
                    .id(PROMPT_INPUT)
                    .on_input(Message::PromptInput)
                    .on_submit(Message::PromptSubmit)
                    .size(14)
                    .padding([8, 12])
                    .style(theme::input(c)),
            );
        }
    }
    if let Some(e) = &p.error {
        body = body.push(text(e.as_str()).size(12).color(c.danger));
    }
    body = body.push(
        row![
            space::horizontal(),
            button(text("Cancel").size(13))
                .padding([6, 14])
                .style(theme::secondary_button(c))
                .on_press(Message::Escape),
            button(text(action).size(13))
                .padding([6, 18])
                .style(theme::primary_button(c))
                .on_press(Message::PromptSubmit),
        ]
        .spacing(8),
    );
    let card = container(body).padding(20).width(460).style(move |_| container::Style {
        background: Some(Background::Color(c.sidebar)),
        border: Border { color: c.border, width: 1.0, radius: 8.0.into() },
        text_color: Some(c.text),
        ..container::Style::default()
    });
    opaque(container(card).center(Length::Fill).style(|_| container::Style {
        background: Some(Background::Color(iced::Color { a: 0.55, ..iced::Color::BLACK })),
        ..container::Style::default()
    }))
}

/// Centered modal card over a dimmed, click-blocking backdrop.
fn dialog<'a>(
    c: theme::C,
    title: String,
    body: impl Into<String>,
    buttons: Element<'a, Message>,
) -> Element<'a, Message> {
    let card = container(
        column![
            text(title).size(15).color(c.text_bright),
            text(body.into()).size(12).color(c.dim),
            buttons,
        ]
        .spacing(12),
    )
    .padding(20)
    .width(460)
    .style(move |_| container::Style {
        background: Some(Background::Color(c.sidebar)),
        border: Border { color: c.border, width: 1.0, radius: 8.0.into() },
        text_color: Some(c.text),
        ..container::Style::default()
    });

    opaque(
        container(card)
            .center(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(iced::Color { a: 0.55, ..iced::Color::BLACK })),
                ..container::Style::default()
            }),
    )
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
