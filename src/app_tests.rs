//! Headless end-to-end tests: drive `App::update`, render `App::view` with the
//! `iced_test` simulator (no display needed) and write PNG snapshots to
//! `target/ui-snapshots/` (override with `VELOEDIT_SNAPSHOT_DIR`).

use std::{fs, path::PathBuf, sync::{Once, OnceLock}};

use iced::{
    futures::executor::block_on,
};

use super::*;
use crate::{
    editor::{
        hex::{self, HexAction},
        state::{Action, Motion, Selection},
    },
    git::Base,
    message::{FindFlag, Toggle},
    palette::Mode,
    project_search,
    search,
    syntax::Group,
};

/// Top of the editor area in a window: menu bar + tab bar.
const TOP: f32 = crate::ui::menu_bar::HEIGHT + 33.0;

fn out_dir() -> PathBuf {
    let dir = std::env::var_os("VELOEDIT_SNAPSHOT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-snapshots"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Renders the current view and writes `<name>-<renderer>.png`.
fn shot(app: &App, name: &str) {
    let dir = out_dir();
    for e in fs::read_dir(&dir).unwrap().flatten() {
        if e.file_name().to_string_lossy().starts_with(&format!("{name}-")) {
            fs::remove_file(e.path()).unwrap();
        }
    }
    let mut ui = iced_test::simulator(app.view());
    let snap = ui.snapshot(&app.theme()).expect("snapshot renders");
    assert!(snap.matches_image(dir.join(format!("{name}.png"))).unwrap());
}

fn scratch() -> PathBuf {
    static INIT: Once = Once::new();
    let dir = std::env::temp_dir().join(format!("veloedit-ui-test-{}", std::process::id()));
    INIT.call_once(|| {
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("state")).unwrap();
        // SAFETY: executed once, before any test thread reads the variable.
        unsafe {
            std::env::set_var("VELOEDIT_STATE_DIR", dir.join("state"));
            std::env::set_var("VELOEDIT_CONFIG_DIR", dir.join("config"));
        }
    });
    dir
}

/// Tests that write the (process-wide) session directory take turns.
fn session_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn png_fixture(path: &Path) {
    let (w, h) = (320u32, 200u32);
    let mut img = image::RgbaImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = image::Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, 160, 255]);
    }
    img.save(path).unwrap();
}

/// Fixture project, created exactly once even when tests run in parallel
/// (`get_or_init` blocks concurrent callers until it is complete).
fn project() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(create_project).clone()
}

fn create_project() -> PathBuf {
    let root = scratch().join("proj");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::create_dir_all(root.join("node_modules")).unwrap();
    fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
    fs::write(root.join("node_modules/x.js"), "ignored").unwrap();
    fs::write(root.join("hello.rs"), "fn main() {\n    println!(\"merhaba velo\");\n}\n").unwrap();
    fs::write(root.join("sub/inner.md"), "# Inner\n").unwrap();
    fs::write(root.join("blob.bin"), (0..4096u32).map(|i| (i * 7 % 256) as u8).collect::<Vec<_>>()).unwrap();
    png_fixture(&root.join("pic.png"));
    // 1 GiB sparse file with a marker in the middle.
    let huge = fs::File::create(root.join("huge.bin")).unwrap();
    huge.set_len(1 << 30).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::FileExt::write_at(&huge, b"ABCDEFGHIJKLMNOP", 1 << 29).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::FileExt::seek_write(&huge, b"ABCDEFGHIJKLMNOP", 1 << 29).unwrap();
    root
}

fn booted(args: Vec<PathBuf>) -> App {
    scratch();
    let (mut app, _tasks) = App::boot(args);
    drop(app.update(Message::Restored(Handoff::new(vec![]), None)));
    app
}

fn open(app: &mut App, path: &Path) {
    let loaded = block_on(loader::open(path.to_path_buf())).expect("opens");
    drop(app.update(Message::Opened(Ok(Handoff::new(loaded)))));
}

fn scan_all(app: &mut App, dir: &Path) {
    drop(app.update(Message::DirScanned {
        path: dir.to_path_buf(),
        entries: workspace::read_dir(dir),
    }));
}

#[test]
fn every_view_kind_renders() {
    let root = project();
    let mut app = booted(vec![root.clone()]);
    scan_all(&mut app, &root);

    let names: Vec<_> = app.ws.rows().iter().map(|r| r.node.entry.name.clone()).collect();
    assert!(!names.contains(&"node_modules".to_owned()), ".gitignore is honored: {names:?}");
    assert!(names.contains(&"sub".to_owned()));

    open(&mut app, &root.join("hello.rs"));
    assert!(matches!(app.tabs.active().unwrap().doc, Doc::Text(_)));
    shot(&app, "01-text");

    open(&mut app, &root.join("blob.bin"));
    assert!(matches!(app.tabs.active().unwrap().doc, Doc::Hex(_)));
    shot(&app, "02-hex");

    open(&mut app, &root.join("pic.png"));
    assert!(matches!(app.tabs.active().unwrap().doc, Doc::Image(_)));
    shot(&app, "03-image");

    // Expand a directory, as the sidebar click would.
    drop(app.update(Message::TreeClicked { path: root.join("sub"), is_dir: true }));
    scan_all(&mut app, &root.join("sub"));
    assert!(app.ws.rows().iter().any(|r| r.node.entry.name == "inner.md" && r.depth == 2));
    shot(&app, "04-expanded-tree");

    // Tab switching + closing.
    drop(app.update(Message::PrevTab));
    assert!(matches!(app.tabs.active().unwrap().doc, Doc::Hex(_)));
    let id = app.tabs.active_id().unwrap();
    drop(app.update(Message::CloseTab(id)));
    assert_eq!(app.tabs.iter().count(), 2);

    // Sidebar toggle.
    drop(app.update(Message::ToggleSidebar));
    shot(&app, "05-sidebar-hidden");
}

#[test]
fn hex_view_of_a_gigabyte_file_is_lazy_and_exact() {
    let root = project();
    let mut app = booted(vec![]);
    open(&mut app, &root.join("huge.bin"));
    let id = app.tabs.active_id().unwrap();
    let hex = |app: &App| match &app.tabs.get(id).unwrap().doc { Doc::Hex(h) => (h.len, h.top_row(), h.byte(1 << 29), h.byte((1 << 29) + 15), h.cursor()), _ => panic!("huge.bin must open as hex") };
    assert_eq!(hex(&app).0, 1 << 30);

    // Jump to the marker in the middle of the file (`:0x20000000` in the palette).
    drop(app.update(Message::Hex(id, HexAction::GoTo(1 << 29))));
    let (_, top, first, last, cursor) = hex(&app);
    assert_eq!((first, last, cursor), (Some(b'A'), Some(b'P'), 1 << 29));
    assert!(top.abs_diff(1 << 25) < 64, "the cursor row is in view: {top}");
    shot(&app, "06-hex-gigabyte-middle");

    // Dragging the scrollbar to the end reaches the last rows.
    drop(app.update(Message::Hex(id, HexAction::ScrollTo { row: f64::MAX })));
    let Doc::Hex(h) = &app.tabs.get(id).unwrap().doc else { panic!() };
    assert!(h.top_row() > (1 << 26) - 64);
    assert!(h.byte((1 << 30) - 1).is_some(), "the last byte is reachable");
}

#[test]
fn hex_editing_saves_in_place_survives_restart_and_asks_before_closing() {
    let root = project();
    let file = root.join("patch-me.bin");
    fs::write(&file, (0..=255u8).collect::<Vec<_>>()).unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();
    let hex_dirty = |app: &App| app.tabs.get(id).map(|t| t.is_dirty());

    for a in [HexAction::GoTo(0x20), HexAction::Digit(0xA), HexAction::Digit(0xB), HexAction::Digit(0xC), HexAction::Digit(0xD)] {
        drop(app.update(Message::Hex(id, a)));
    }
    assert_eq!(hex_dirty(&app), Some(true));
    shot(&app, "25-hex-edit");
    assert_eq!(fs::read(&file).unwrap()[0x20], 0x20, "nothing is written before saving");

    // Closing asks first (the generic dialog), Cancel keeps the edits.
    drop(app.update(Message::CloseTab(id)));
    assert_eq!(app.dialog, Some(Dialog::CloseDirty(id)));
    drop(app.update(Message::CloseChoice(id, CloseChoice::Cancel)));
    assert_eq!(hex_dirty(&app), Some(true));

    // Hot exit: the pending edits are part of the session.
    let snap = app.snapshot();
    let rec = snap.file.tabs.iter().find(|t| t.path.as_deref() == Some(file.as_path())).unwrap().clone();
    assert_eq!(rec.hex_edits, vec![(0x20, 0xAB), (0x21, 0xCD)]);
    let (loaded, _) = block_on(loader::restore(vec![rec], Some(0)));
    let Body::Hex(h) = &loaded[0].body else { panic!() };
    assert_eq!((h.byte(0x20), h.byte(0x21), h.dirty()), (Some(0xAB), Some(0xCD), true));

    // Save: the worker writes the patch, the app clears the edits afterwards.
    let Doc::Hex(h) = &app.tabs.get(id).unwrap().doc else { panic!() };
    let saved: Vec<(u64, u8)> = h.edits().iter().map(|(&o, &b)| (o, b)).collect();
    hex::write_patches(&file, h.edits()).unwrap();
    drop(app.update(Message::HexSaved { id, saved: Arc::new(saved), then_close: false, result: Ok(()) }));
    assert_eq!(hex_dirty(&app), Some(false));
    let disk = fs::read(&file).unwrap();
    assert_eq!((disk.len(), disk[0x1F], disk[0x20], disk[0x21], disk[0x22]), (256, 0x1F, 0xAB, 0xCD, 0x22));
    // The watcher echo of our own write must not reset the tab.
    let echo = block_on(loader::open(file.clone())).map(Handoff::new);
    drop(app.update(Message::DiskChanged { id, force: false, result: echo }));
    let Doc::Hex(h) = &app.tabs.get(id).unwrap().doc else { panic!() };
    assert_eq!(h.cursor(), 0x22, "cursor kept: the echo was ignored");

    // Save-and-close from the dialog: edit again, choose Save.
    drop(app.update(Message::Hex(id, HexAction::Byte(b'z'))));
    drop(app.update(Message::CloseTab(id)));
    drop(app.update(Message::CloseChoice(id, CloseChoice::Save)));
    let Doc::Hex(h) = &app.tabs.get(id).unwrap().doc else { panic!() };
    hex::write_patches(&file, h.edits()).unwrap(); // what the spawned task does
    let saved: Vec<(u64, u8)> = h.edits().iter().map(|(&o, &b)| (o, b)).collect();
    drop(app.update(Message::HexSaved { id, saved: Arc::new(saved), then_close: true, result: Ok(()) }));
    assert!(app.tabs.get(id).is_none(), "saved, then closed");
    assert_eq!(fs::read(&file).unwrap()[0x22], b'z');
}

#[test]
fn unsaved_tabs_are_named_after_their_first_line() {
    let _turn = session_lock();
    let root = project();
    let file = root.join("named.txt");
    fs::write(&file, "text inside a saved file\n").unwrap();
    let mut app = booted(vec![]);
    let label = |app: &App, id: TabId| app.tabs.get(id).unwrap().label(crate::editor::TAB_LABEL).into_owned();
    let type_into = |app: &mut App, id: TabId, text: &str| {
        for c in text.chars() {
            drop(app.update(Message::Editor(id, Action::Insert(c.to_string()))));
        }
    };

    drop(app.update(Message::NewFile));
    let a = app.tabs.active_id().unwrap();
    drop(app.update(Message::NewFile));
    let b = app.tabs.active_id().unwrap();
    assert_eq!((label(&app, a).as_str(), label(&app, b).as_str()), ("untitled", "untitled 2"), "empty tabs keep their numbers");

    type_into(&mut app, a, "\n  SELECT   *  FROM users\nWHERE id = 1");
    assert_eq!(label(&app, a), "SELECT * FROM users", "blank lines and extra spaces are skipped");
    assert_eq!(label(&app, b), "untitled 2", "the other tab is untouched");
    assert_eq!(app.tabs.get(a).unwrap().title, "untitled", "the stable name (save dialog) is unchanged");

    // The window title follows the active tab; the tab list and the close dialog use the name too.
    app.tabs.activate(a);
    assert!(app.title().contains("SELECT * FROM users"), "{}", app.title());

    // Long first lines are cut with an ellipsis; the tab keeps a sane width.
    type_into(&mut app, b, "a very long first line that would never fit into one tab of the strip");
    let long = label(&app, b);
    assert_eq!(long.chars().count(), crate::editor::TAB_LABEL);
    assert!(long.ends_with('\u{2026}'), "{long}");
    assert!(crate::ui::tab_bar::span_of(&app.tabs, b).map(|(l, r)| r - l).unwrap() <= 240.0);

    // Editing the first line renames the tab; emptying the buffer gives the number back.
    drop(app.update(Message::Editor(a, Action::SelectAll)));
    drop(app.update(Message::Editor(a, Action::Insert("renamed\nbody".into()))));
    assert_eq!(label(&app, a), "renamed");
    drop(app.update(Message::Editor(a, Action::SelectAll)));
    drop(app.update(Message::Editor(a, Action::Backspace)));
    assert_eq!(label(&app, a), "untitled");

    // A file with a path always shows its file name, whatever it contains.
    open(&mut app, &file);
    let f = app.tabs.active_id().unwrap();
    assert_eq!(label(&app, f), "named.txt");
    shot(&app, "tabs-named-after-first-line");
}

/// Every message the widgets publish while `drive` runs against the current view.
fn messages(app: &App, drive: impl FnOnce(&mut iced_test::Simulator<'_, Message>)) -> Vec<Message> {
    let mut ui = iced_test::simulator(app.view());
    drive(&mut ui);
    ui.into_messages().collect()
}

#[test]
fn a_split_tab_shows_one_text_in_two_panes_with_their_own_places() {
    use crate::editor::{Pane, SplitDir};
    let _turn = session_lock();
    let root = project();
    let file = root.join("split-me.txt");
    fs::write(&file, (0..400).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();
    let text_of = |app: &App, pane: Pane| app.tabs.get(id).unwrap().pane_state(pane).unwrap().text();
    let state_of = |app: &App, pane: Pane| -> (f32, usize) {
        let s = app.tabs.get(id).unwrap().pane_state(pane).unwrap();
        (s.scroll_y, s.caret_line_col().0)
    };
    let focus = |app: &App| app.tabs.get(id).unwrap().split.as_ref().map(|s| s.focus);

    // The tab menu offers the split; nothing is split before.
    let entries = app.tab_entries(id);
    let labels = |e: &[Entry]| e.iter().filter_map(|e| match e { Entry::Item { label, .. } => Some(label.clone()), _ => None }).collect::<Vec<_>>();
    assert!(labels(&entries).iter().any(|l| l == "Split Right") && labels(&entries).iter().any(|l| l == "Split Down"));
    assert!(!labels(&entries).iter().any(|l| l == "Close Split"));
    assert!(app.tabs.get(id).unwrap().split.is_none());

    drop(app.update(Message::SplitTab(id, SplitDir::Right)));
    assert_eq!(focus(&app), Some(Pane::First));
    assert!(labels(&app.tab_entries(id)).iter().any(|l| l == "Close Split"));
    drop(messages(&app, |_| {})); // first layout measures both panes
    let m = app.tabs.get(id).unwrap().pane_state(Pane::Second).unwrap().metrics();
    assert!(m.char_w > 5.0, "the second pane was laid out too: {m:?}");

    // Pixels: the sidebar, then the first pane, a 5 px divider, the second pane.
    let area_left = sidebar::DEFAULT_WIDTH + 5.0;
    let pane2_left = area_left + (1024.0 - area_left - 5.0) / 2.0 + 5.0;
    let in_pane2 = |dx: f32, dy: f32| iced::Point::new(pane2_left + dx, TOP + dy);
    let in_pane1 = |dx: f32, dy: f32| iced::Point::new(area_left + dx, TOP + dy);

    // The wheel over the second pane scrolls only it; the first pane and the focus stay.
    let msgs = messages(&app, |ui| {
        ui.point_at(in_pane2(150.0, 100.0));
        ui.simulate([Event::Mouse(mouse::Event::WheelScrolled { delta: mouse::ScrollDelta::Lines { x: 0.0, y: -10.0 } })]);
    });
    assert!(matches!(msgs.as_slice(), [Message::EditorPane(i, Pane::Second, Action::Scroll { .. })] if *i == id), "{}", msgs.len());
    for m in msgs {
        drop(app.update(m));
    }
    assert_eq!(focus(&app), Some(Pane::First), "looking around does not take the focus");
    assert_eq!(state_of(&app, Pane::First).0, 0.0);
    assert!(state_of(&app, Pane::Second).0 > 100.0, "{:?}", state_of(&app, Pane::Second));

    // Clicking into the second pane makes it the working pane (its caret, its scroll).
    let msgs = messages(&app, |ui| {
        ui.point_at(in_pane2(150.0, 60.0));
        ui.simulate(iced_test::simulator::click());
    });
    assert!(matches!(msgs.as_slice(), [Message::EditorPane(i, Pane::Second, Action::Click { .. })] if *i == id));
    for m in msgs {
        drop(app.update(m));
    }
    assert_eq!(focus(&app), Some(Pane::Second));
    let (scroll2, caret2) = state_of(&app, Pane::Second);
    assert!(caret2 > 5 && scroll2 > 100.0, "caret on a line far down: {caret2}, scroll {scroll2}");
    assert_eq!(state_of(&app, Pane::First), (0.0, 0), "the first pane kept its caret and its place");

    // Typing there edits the one text; the other pane shows it at once and stays where it was.
    let before = text_of(&app, Pane::First);
    drop(app.update(Message::EditorPane(id, Pane::Second, Action::Insert("QQ".into()))));
    assert_eq!(text_of(&app, Pane::First), text_of(&app, Pane::Second));
    assert_ne!(text_of(&app, Pane::First), before);
    assert!(text_of(&app, Pane::First).contains("QQ"));
    assert_eq!(state_of(&app, Pane::First), (0.0, 0));
    // Typing at the very top of the first pane pushes the second pane's caret down one line.
    drop(app.update(Message::EditorPane(id, Pane::First, Action::Insert("top\n".into()))));
    assert_eq!(focus(&app), Some(Pane::First));
    assert_eq!(text_of(&app, Pane::First), text_of(&app, Pane::Second));
    assert_eq!(state_of(&app, Pane::Second).1, caret2 + 1);
    // One undo history for both panes: undo twice from whichever pane.
    drop(app.update(Message::EditorPane(id, Pane::Second, Action::Undo)));
    drop(app.update(Message::EditorPane(id, Pane::First, Action::Undo)));
    assert_eq!(text_of(&app, Pane::First), before);
    assert_eq!(text_of(&app, Pane::Second), before);
    assert!(!app.tabs.get(id).unwrap().is_dirty(), "back at the saved text");

    // Layout switches and the divider.
    drop(app.update(Message::SplitTab(id, SplitDir::Down)));
    assert_eq!(app.tabs.get(id).unwrap().split.as_ref().map(|s| s.dir), Some(SplitDir::Down));
    drop(app.update(Message::SplitResizeStart));
    let extent = app.split_extent();
    let from = pointer();
    drop(app.update(Message::SplitResizeTo(iced::Point::new(from.x, from.y + extent * 0.2))));
    assert!((app.tabs.get(id).unwrap().split.as_ref().unwrap().frac - 0.7).abs() < 0.001);
    drop(app.update(Message::SplitResizeTo(iced::Point::new(from.x, from.y - extent))));
    assert_eq!(app.tabs.get(id).unwrap().split.as_ref().unwrap().frac, crate::editor::SPLIT_MIN);
    drop(app.update(Message::SplitResizeEnd));
    drop(app.update(Message::SplitResizeTo(iced::Point::new(from.x, from.y + extent))));
    assert_eq!(app.tabs.get(id).unwrap().split.as_ref().unwrap().frac, crate::editor::SPLIT_MIN, "ignored unless dragging");
    app.tabs.get_mut(id).unwrap().split.as_mut().unwrap().frac = 0.5;
    drop(messages(&app, |_| {}));
    drop(app.update(Message::EditorPane(id, Pane::Second, Action::ScrollTo { y: 20.0 * 18.0 * 100.0 })));
    shot(&app, "split-down");
    drop(app.update(Message::SplitTab(id, SplitDir::Right)));
    drop(messages(&app, |_| {}));
    shot(&app, "split-right");

    // Closing keeps the pane the user worked in.
    let _ = (in_pane1(0.0, 0.0),);
    drop(app.update(Message::EditorPane(id, Pane::Second, Action::Click { line: 120, col: 0, extend: false, count: 1, add: false })));
    drop(app.update(Message::CloseSplit(id)));
    assert!(app.tabs.get(id).unwrap().split.is_none());
    let Doc::Text(d) = &app.tabs.get(id).unwrap().doc else { panic!() };
    assert_eq!(d.state.caret_line_col().0, 120, "the focused pane's caret stays");
    assert!(app.tabs.get(id).unwrap().pane_state(Pane::Second).is_some());
}

#[test]
fn file_dialogs_start_in_the_folder_of_the_last_picked_file() {
    let _turn = session_lock();
    let root = project().join("dialogs");
    let (far, gone) = (root.join("far"), root.join("gone"));
    for d in [&far, &gone] {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(far.join("a.txt"), "a").unwrap();
    fs::write(gone.join("b.txt"), "b").unwrap();
    fs::write(root.join("here.txt"), "h").unwrap();
    let mut app = booted(vec![]);
    app.last_dir = None;

    // Nothing known yet: the desktop decides (its "Recent" page).
    assert_eq!(app.dialog_dir(), None);
    // The active file's folder is the next best thing...
    open(&mut app, &root.join("here.txt"));
    assert_eq!(app.dialog_dir(), Some(root.clone()));
    // ...until a file has been picked in a dialog: that folder is remembered.
    drop(app.update(Message::FilesPicked(vec![far.join("a.txt")])));
    assert_eq!(app.dialog_dir(), Some(far.clone()));
    // Cancelling a dialog forgets nothing.
    drop(app.update(Message::FilesPicked(vec![])));
    assert_eq!(app.dialog_dir(), Some(far.clone()));
    // It survives a restart (the session file carries it).
    assert_eq!(app.snapshot().file.last_dir, Some(far.clone()));
    // Saving counts as a pick, too.
    let id = app.tabs.active_id().unwrap();
    drop(app.update(Message::SavePicked { id, path: Some(gone.join("saved.txt")), then_close: false }));
    assert_eq!(app.dialog_dir(), Some(gone.clone()));
    // A remembered folder that no longer exists is skipped.
    fs::remove_dir_all(&gone).unwrap();
    assert_eq!(app.dialog_dir(), Some(root));
}

#[test]
fn split_panes_have_name_bars_that_focus_and_close() {
    use crate::editor::{Pane, SplitDir};
    let _turn = session_lock();
    let root = project();
    let file = root.join("bars.txt");
    fs::write(&file, (0..300).map(|i| format!("row {i}\n")).collect::<String>()).unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();
    let focus = |app: &App| app.tabs.get(id).unwrap().split.as_ref().map(|s| s.focus);
    drop(app.update(Message::SplitTab(id, SplitDir::Right)));
    drop(messages(&app, |_| {}));

    // A click on the second pane's name bar (just under the tab strip) works that pane.
    let area_left = sidebar::DEFAULT_WIDTH + 5.0;
    let pane2_left = area_left + (1024.0 - area_left - 5.0) / 2.0 + 5.0;
    let msgs = messages(&app, |ui| {
        ui.point_at(iced::Point::new(pane2_left + 160.0, TOP + 10.0));
        ui.simulate(iced_test::simulator::click());
    });
    assert!(matches!(msgs.as_slice(), [Message::FocusPane(i, Pane::Second)] if *i == id), "{}", msgs.len());
    for m in msgs {
        drop(app.update(m));
    }
    assert_eq!(focus(&app), Some(Pane::Second));
    shot(&app, "split-name-bars");

    // The × of a bar closes that pane and keeps the other pane's place and caret.
    drop(app.update(Message::EditorPane(id, Pane::Second, Action::Click { line: 7, col: 1, extend: false, count: 1, add: false })));
    drop(app.update(Message::EditorPane(id, Pane::First, Action::Click { line: 40, col: 0, extend: false, count: 1, add: false })));
    drop(app.update(Message::ClosePane(id, Pane::First)));
    assert!(app.tabs.get(id).unwrap().split.is_none());
    let Doc::Text(d) = &app.tabs.get(id).unwrap().doc else { panic!() };
    assert_eq!(d.state.caret_line_col().0, 7, "closing the first pane leaves the second pane's view");

    drop(app.update(Message::SplitTab(id, SplitDir::Down)));
    drop(app.update(Message::EditorPane(id, Pane::Second, Action::Click { line: 90, col: 0, extend: false, count: 1, add: false })));
    drop(app.update(Message::ClosePane(id, Pane::Second)));
    let Doc::Text(d) = &app.tabs.get(id).unwrap().doc else { panic!() };
    assert_eq!(d.state.caret_line_col().0, 7, "closing the second pane leaves the first pane's view");
}

#[test]
fn tabs_can_be_dragged_into_a_new_order() {
    use std::sync::atomic::Ordering::Relaxed;
    let _turn = session_lock();
    let root = project().join("drag");
    fs::create_dir_all(&root).unwrap();
    let mut app = booted(vec![]);
    let mut ids = Vec::new();
    for n in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        fs::write(root.join(n), n).unwrap();
        open(&mut app, &root.join(n));
        ids.push(app.tabs.active_id().unwrap());
    }
    let order = |app: &App| app.tabs.iter().map(|t| t.title.clone()).collect::<Vec<_>>().join(" ");
    assert_eq!(order(&app), "a.txt b.txt c.txt d.txt");
    // Strip geometry: starts at the sidebar's edge, just under the menu bar; every tab is 100 px wide.
    let (left, y) = (sidebar::DEFAULT_WIDTH + 5.0, menu_bar::HEIGHT + 15.0);
    let at = |x: f32| iced::Point::new(left + x, y);
    let press = |app: &mut App, id: TabId| {
        super::LEFT_DOWN.store(true, Relaxed);
        drop(app.update(Message::TabPressed(id)));
    };

    // Press a: it becomes the active tab at once; a tiny move is still a click.
    press(&mut app, ids[0]);
    assert_eq!(app.tabs.active_id(), Some(ids[0]));
    drop(app.update(Message::TabDragTo(iced::Point::new(super::pointer().x + 2.0, super::pointer().y))));
    assert!(app.tab_drag.as_ref().is_some_and(|d| !d.live), "below the threshold");
    // Drag it between c and d (x = 330 is right of c's middle at 250 and left of d's at 350).
    drop(app.update(Message::TabDragTo(at(330.0))));
    let d = app.tab_drag.as_ref().unwrap();
    assert!(d.live && d.slot == Some(3), "{:?}", d.slot);
    shot(&app, "tab-drag"); // dimmed tab + insertion mark
    drop(app.update(Message::TabDragEnd));
    assert_eq!(order(&app), "b.txt c.txt a.txt d.txt");
    assert_eq!(app.tabs.active_id(), Some(ids[0]), "the dragged tab stays active");
    let saved: Vec<_> = app.snapshot().file.tabs.iter().filter_map(|t| t.path.as_ref()?.file_name()?.to_str().map(String::from)).collect();
    assert_eq!(saved, ["b.txt", "c.txt", "a.txt", "d.txt"], "the new order is part of the session");

    // Drag d all the way to the front, then back to where it was: nothing moves.
    press(&mut app, ids[3]);
    drop(app.update(Message::TabDragTo(at(5.0))));
    drop(app.update(Message::TabDragEnd));
    assert_eq!(order(&app), "d.txt b.txt c.txt a.txt");
    press(&mut app, ids[3]);
    drop(app.update(Message::TabDragTo(at(20.0))));
    drop(app.update(Message::TabDragTo(at(60.0))));
    drop(app.update(Message::TabDragEnd));
    assert_eq!(order(&app), "d.txt b.txt c.txt a.txt", "dropped where it was");

    // Pulled far away from the strip (or Esc, or a missed release): nothing moves.
    press(&mut app, ids[1]);
    drop(app.update(Message::TabDragTo(iced::Point::new(left + 300.0, 400.0))));
    assert_eq!(app.tab_drag.as_ref().unwrap().slot, None);
    drop(app.update(Message::TabDragEnd));
    assert_eq!(order(&app), "d.txt b.txt c.txt a.txt");
    press(&mut app, ids[1]);
    drop(app.update(Message::TabDragTo(at(380.0))));
    drop(app.update(Message::Escape));
    assert!(app.tab_drag.is_none());
    drop(app.update(Message::TabDragEnd));
    assert_eq!(order(&app), "d.txt b.txt c.txt a.txt");
    press(&mut app, ids[1]);
    super::LEFT_DOWN.store(false, Relaxed);
    drop(app.update(Message::TabDragTo(at(380.0))));
    assert!(app.tab_drag.is_none(), "button no longer down: the drag is dropped");
    assert_eq!(order(&app), "d.txt b.txt c.txt a.txt");
}

#[test]
fn a_single_sidebar_click_previews_and_a_second_click_or_an_edit_pins() {
    let _turn = session_lock();
    let root = project().join("preview");
    fs::create_dir_all(&root).unwrap();
    for n in ["a", "b", "c", "d", "e", "f", "g"] {
        fs::write(root.join(format!("{n}.txt")), n).unwrap();
    }
    let mut app = booted(vec![]);
    let path = |n: &str| root.join(format!("{n}.txt"));
    let names = |app: &App| app.tabs.iter().map(|t| format!("{}{}", t.title.trim_end_matches(".txt"), if t.preview { "*" } else { "" })).collect::<Vec<_>>().join(" ");
    // A sidebar click, with the file arriving from disk (the load is a task in the real app).
    let click = |app: &mut App, n: &str| {
        drop(app.update(Message::TreeClicked { path: path(n), is_dir: false }));
        if app.tabs.by_path(&path(n)).is_none() {
            let loaded = block_on(loader::open(path(n))).unwrap();
            drop(app.update(Message::OpenedPreview(Ok(Handoff::new(loaded)))));
        }
    };
    // Past the double-click time, so that the next click is a new click.
    let later = |app: &mut App| app.last_tree_click = None;

    click(&mut app, "a");
    assert_eq!(names(&app), "a*", "one click: a preview tab");
    later(&mut app);
    click(&mut app, "b");
    assert_eq!(names(&app), "b*", "the next click takes its place");
    assert_eq!(app.tabs.active().unwrap().title, "b.txt");

    // Files opened any other way are real tabs and are never replaced.
    open(&mut app, &path("c"));
    assert_eq!(names(&app), "b* c");
    later(&mut app);
    click(&mut app, "d");
    assert_eq!(names(&app), "d* c", "the preview keeps its position");
    later(&mut app);

    // Clicking a file that is open just shows it; a preview stays a preview, a tab a tab.
    click(&mut app, "c");
    assert_eq!((names(&app).as_str(), app.tabs.active().unwrap().title.as_str()), ("d* c", "c.txt"));
    later(&mut app);
    click(&mut app, "d");
    assert_eq!(names(&app), "d* c");
    later(&mut app);

    // Typing in the preview makes it a real tab; the next click opens a new preview.
    let d = app.tabs.active_id().unwrap();
    assert_eq!(app.tabs.get(d).unwrap().title, "d.txt");
    drop(app.update(Message::Editor(d, Action::Insert("y".into()))));
    assert_eq!(names(&app), "d c", "an edit pins");
    click(&mut app, "e");
    assert_eq!(names(&app), "d c e*", "work in a tab is never replaced");
    later(&mut app);

    // A second click on the file pins it - also when it arrives before the first load did.
    click(&mut app, "e");
    assert_eq!(names(&app), "d c e*", "an open preview clicked once stays a preview");
    click(&mut app, "e");
    assert_eq!(names(&app), "d c e", "double click");
    later(&mut app);
    drop(app.update(Message::TreeClicked { path: path("f"), is_dir: false }));
    drop(app.update(Message::TreeClicked { path: path("f"), is_dir: false })); // before the load finished
    for pinned in [false, true] {
        let loaded = block_on(loader::open(path("f"))).unwrap();
        let msg = if pinned { Message::Opened(Ok(Handoff::new(loaded))) } else { Message::OpenedPreview(Ok(Handoff::new(loaded))) };
        drop(app.update(msg));
    }
    assert_eq!(names(&app), "d c e f");

    // A double click on a tab pins it, too; so does the context menu's Open.
    later(&mut app);
    click(&mut app, "g");
    assert_eq!(names(&app), "d c e f g*");
    let g = app.tabs.active_id().unwrap();
    drop(app.update(Message::TabPressed(g)));
    assert!(app.tabs.get(g).unwrap().preview, "a single press does not pin");
    drop(app.update(Message::TabPressed(g)));
    assert!(!app.tabs.get(g).unwrap().preview);
    drop(app.update(Message::TabDragEnd));
    later(&mut app);
    click(&mut app, "a");
    assert_eq!(names(&app), "d c e f g a*");
    drop(app.update(Message::OpenPinned(path("a"))));
    assert_eq!(names(&app), "d c e f g a");
    // Closing the only preview and previewing again works from an empty slot.
    later(&mut app);
    click(&mut app, "b");
    assert_eq!(names(&app), "d c e f g a b*");
    shot(&app, "preview-tab");
}

#[test]
fn unsaved_work_survives_a_restart_and_close_prompts() {
    let _turn = session_lock();
    let mut app = booted(vec![]);
    drop(app.update(Message::NewFile));
    let id = app.tabs.active_id().unwrap();
    for c in "merhaba dunya".chars() {
        drop(app.update(Message::Editor(id, Action::Insert(c.to_string()))));
    }
    assert!(app.tabs.get(id).unwrap().is_dirty());

    // "Quit": write the snapshot exactly like CloseRequested does.
    let snapshot = app.snapshot();
    block_on(snapshot.write()).unwrap();

    // "Restart": a fresh App restores from the state dir.
    let session = session::load();
    assert_eq!(session.tabs.len(), 1);
    let (loaded, active) = block_on(loader::restore(session.tabs, session.active));
    assert_eq!(active, Some(0));
    assert!(loaded[0].dirty);
    assert!(matches!(&loaded[0].body, Body::Text(t) if t == "merhaba dunya"));

    let mut fresh = booted(vec![]);
    drop(fresh.update(Message::Restored(Handoff::new(loaded), active)));
    let Doc::Text(doc) = &fresh.tabs.active().unwrap().doc else { panic!() };
    assert_eq!(doc.text().trim_end(), "merhaba dunya");

    // Closing a dirty tab asks first; Don't Save closes it.
    let id = fresh.tabs.active_id().unwrap();
    drop(fresh.update(Message::CloseTab(id)));
    assert_eq!(fresh.dialog, Some(Dialog::CloseDirty(id)));
    shot(&fresh, "07-close-prompt");
    drop(fresh.update(Message::CloseChoice(id, CloseChoice::Discard)));
    assert!(fresh.tabs.is_empty());
    shot(&fresh, "08-welcome");
}

#[test]
fn save_writes_atomically_and_clears_dirty() {
    let root = project();
    let file = root.join("save-me.txt");
    fs::write(&file, "one\n").unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();
    drop(app.update(Message::Editor(id, Action::Insert('X'.to_string()))));
    assert!(app.tabs.get(id).unwrap().is_dirty());

    // What SaveTab schedules, executed synchronously.
    let Doc::Text(doc) = &app.tabs.get(id).unwrap().doc else { panic!() };
    let (text, version) = (doc.text(), doc.version());
    session::atomic_write(&file, text.as_bytes()).unwrap();
    drop(app.update(Message::Saved { id, path: file.clone(), version, then_close: false, result: Ok(crate::editor::text::hash_bytes(text.as_bytes())) }));

    assert_eq!(fs::read_to_string(&file).unwrap(), "Xone\n");
    assert!(!app.tabs.get(id).unwrap().is_dirty());
}

/// Feeds what the watcher would trigger: re-read the file and hand it to the app.
fn disk_event(app: &mut App, id: TabId, path: &Path) {
    let result = block_on(loader::reload(path.to_path_buf())).map(Handoff::new);
    drop(app.update(Message::DiskChanged { id, force: false, result }));
}

#[test]
fn external_edit_reloads_clean_buffers_but_never_clobbers_dirty_ones() {
    let root = project();
    let file = root.join("watched.txt");
    fs::write(&file, "v1\n").unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();
    let text = |app: &App| match &app.tabs.get(id).unwrap().doc {
        Doc::Text(d) => d.text(),
        _ => panic!(),
    };

    // Our own save echoes back through the watcher: must be a no-op.
    drop(app.update(Message::Editor(id, Action::Insert('!'.to_string()))));
    let Doc::Text(doc) = &app.tabs.get(id).unwrap().doc else { panic!() };
    let (saved, version) = (doc.text(), doc.version());
    session::atomic_write(&file, saved.as_bytes()).unwrap();
    drop(app.update(Message::Saved {
        id, path: file.clone(), version, then_close: false,
        result: Ok(crate::editor::text::hash_bytes(saved.as_bytes())),
    }));
    disk_event(&mut app, id, &file);
    assert!(!app.tabs.get(id).unwrap().is_dirty());
    assert!(!app.tabs.get(id).unwrap().has_conflict());
    assert_eq!(text(&app), "!v1\n");

    // Someone else edits a clean buffer: silently reloaded.
    fs::write(&file, "from outside\n").unwrap();
    disk_event(&mut app, id, &file);
    assert_eq!(text(&app), "from outside\n");
    assert!(app.notice.as_deref().unwrap().contains("Reloaded"));

    // Unsaved edits + external edit: kept, flagged, Save asks first.
    drop(app.update(Message::Editor(id, Action::Insert('M'.to_string()))));
    fs::write(&file, "outside again\n").unwrap();
    disk_event(&mut app, id, &file);
    assert!(app.tabs.get(id).unwrap().has_conflict());
    assert!(text(&app).contains('M') && !text(&app).contains("outside again"), "my edit survives, the disk text is not taken");
    shot(&app, "09-conflict-tab");

    drop(app.update(Message::SaveActive));
    assert_eq!(app.dialog, Some(Dialog::Conflict { id, then_close: false }));
    assert_eq!(fs::read_to_string(&file).unwrap(), "outside again\n", "nothing written yet");
    shot(&app, "10-conflict-dialog");

    // Reload discards my edits and takes the disk version.
    drop(app.update(Message::ConflictChoice(id, false, ConflictChoice::Cancel)));
    assert!(app.dialog.is_none());
    let result = block_on(loader::open(file.clone())).map(Handoff::new);
    drop(app.update(Message::DiskChanged { id, force: true, result }));
    assert_eq!(text(&app), "outside again\n");
    assert!(!app.tabs.get(id).unwrap().is_dirty() && !app.tabs.get(id).unwrap().has_conflict());
}

#[test]
fn deleted_file_is_reported_and_kept_open() {
    let root = project();
    let file = root.join("doomed.txt");
    fs::write(&file, "bye\n").unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();
    fs::remove_file(&file).unwrap();
    disk_event(&mut app, id, &file);
    assert!(app.notice.as_deref().unwrap().contains("doomed.txt was deleted on disk"), "{:?}", app.notice);
    assert_eq!(app.tabs.iter().count(), 1, "the tab (and its text) stays");
}

#[test]
fn sidebar_follows_the_disk() {
    let root = project().join("live");
    fs::create_dir_all(root.join("d")).unwrap();
    fs::write(root.join("d/one.txt"), "").unwrap();
    let mut app = booted(vec![root.clone()]);
    scan_all(&mut app, &root);
    drop(app.update(Message::TreeClicked { path: root.join("d"), is_dir: true }));
    scan_all(&mut app, &root.join("d"));
    let names = |app: &App| app.ws.rows().iter().map(|r| r.node.entry.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&app), vec!["live", "d", "one.txt"]);
    let watched = app.watch_set();
    assert_eq!(watched.len(), 2, "root + expanded dir, nothing else");

    fs::write(root.join("d/two.txt"), "").unwrap();
    fs::remove_file(root.join("d/one.txt")).unwrap();
    // Both ends of the pipeline: the changed paths select the dirs to rescan...
    drop(app.update(Message::FsChanged(vec![root.join("d/two.txt"), root.join("d/one.txt")])));
    // ...and the scan result is merged without collapsing anything.
    scan_all(&mut app, &root.join("d"));
    assert_eq!(names(&app), vec!["live", "d", "two.txt"]);
}

fn type_query(app: &mut App, q: &str) {
    drop(app.update(Message::PaletteQuery(q.to_owned())));
    // The ranking runs on a worker in the app; run the same function here.
    let seq = app.palette_seq;
    let hits = {
        let index = app.file_index.clone().unwrap();
        palette::rank_files(&index, q, palette::LIMIT)
    };
    drop(app.update(Message::PaletteResults { seq, hits }));
}

#[test]
fn quick_open_finds_opens_and_respects_gitignore() {
    let root = project();
    let mut app = booted(vec![root.clone()]);

    drop(app.update(Message::OpenPalette(Mode::Files)));
    assert!(app.index_building && app.palette.is_some());
    shot(&app, "11-palette-indexing");

    // The worker's result arrives.
    let index = Arc::new(palette::index::build(&[root.clone()]));
    assert!(!index.iter().any(|f| f.rel.contains("node_modules")), ".gitignore applies to the index");
    drop(app.update(Message::IndexBuilt(index)));
    assert!(!app.palette.as_ref().unwrap().items.is_empty(), "empty query lists files");

    type_query(&mut app, "hlo");
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.items[0].title, "hello.rs", "{:?}", p.items.iter().map(|i| &i.title).collect::<Vec<_>>());
    shot(&app, "12-palette-files");

    // Enter opens the selected file and closes the palette.
    drop(app.update(Message::PaletteSubmit));
    assert!(app.palette.is_none());
    open(&mut app, &root.join("hello.rs")); // what the spawned open task delivers
    assert_eq!(app.tabs.active().unwrap().title, "hello.rs");

    // Open tabs lead the list for an empty query.
    drop(app.update(Message::OpenPalette(Mode::Files)));
    drop(app.update(Message::PaletteQuery(String::new())));
    assert_eq!(app.palette.as_ref().unwrap().items[0].title, "hello.rs");
    // Same shortcut again toggles it closed; Escape closes too.
    drop(app.update(Message::OpenPalette(Mode::Files)));
    assert!(app.palette.is_none());
    drop(app.update(Message::OpenPalette(Mode::Commands)));
    drop(app.update(Message::Escape));
    assert!(app.palette.is_none());
}

#[test]
fn command_palette_switches_theme_and_modal_blocks_the_editor() {
    let root = project();
    let mut app = booted(vec![]);
    open(&mut app, &root.join("hello.rs"));

    drop(app.update(Message::OpenPalette(Mode::Commands)));
    drop(app.update(Message::PaletteQuery("light".into())));
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.items[0].title, "Theme: Light");
    shot(&app, "13-palette-commands");

    drop(app.update(Message::PaletteSubmit));
    assert_eq!(app.settings.colors().name, "light");
    shot(&app, "14-light-theme");

    // The setting is persisted as JSON, other keys untouched.
    let cfg = settings::path();
    fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    fs::write(&cfg, r#"{"theme":"dark","my_note":"keep me"}"#).unwrap();
    settings::set("theme", serde_json::json!("light")).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(saved["theme"], "light");
    assert_eq!(saved["my_note"], "keep me");
    assert_eq!(settings::load().unwrap().colors().name, "light");
    fs::write(&cfg, "{ not json").unwrap();
    assert!(settings::load().is_err() && settings::set("theme", "dark".into()).is_err());
    fs::remove_file(&cfg).unwrap();

    // With a dialog open, global shortcuts are ignored.
    drop(app.update(Message::NewFile));
    let id = app.tabs.active_id().unwrap();
    drop(app.update(Message::Editor(id, Action::Insert('x'.to_string()))));
    drop(app.update(Message::CloseTab(id)));
    assert!(app.dialog.is_some());
    let before = app.tabs.iter().count();
    drop(app.update(Message::Key(keyboard::Event::KeyPressed {
        key: keyboard::Key::Character("w".into()),
        modified_key: keyboard::Key::Character("w".into()),
        physical_key: keyboard::key::Physical::Code(keyboard::key::Code::KeyW),
        location: keyboard::Location::Standard,
        modifiers: keyboard::Modifiers::CTRL,
        text: None,
        repeat: false,
    })));
    assert_eq!(app.tabs.iter().count(), before);
    drop(app.update(Message::Escape));
    assert!(app.dialog.is_none(), "Escape cancels the dialog");
}

/// What the worker task does, run inline: parse the active buffer and deliver
/// the result as the app would receive it.
fn run_highlight_job(app: &mut App) {
    drop(app.start_analysis()); // no-op if the job was already started
    let id = app.tabs.active_id().unwrap();
    let Doc::Text(doc) = &app.tabs.get(id).unwrap().doc else { panic!() };
    assert!(doc.hl.in_flight, "a job was started");
    let (lang, text, prev) = (doc.hl.lang.unwrap(), doc.text(), doc.hl.parse.clone());
    let result = syntax::highlight(lang, prev.as_ref(), Arc::new(text)).map(|(p, s)| (p, Arc::new(s)));
    drop(app.update(Message::Highlighted { id, lang, result }));
}

#[test]
fn syntax_highlighting_follows_edits_and_themes() {
    let root = project();
    let code = root.join("sample.rs");
    fs::write(
        &code,
        "use std::collections::HashMap;\n\n/// A tiny demo.\n#[derive(Debug)]\nstruct Point { x: f32, y: f32 }\n\nfn main() {\n    let name = \"velo\\n\"; // greeting\n    let mut total: u32 = 42;\n    for i in 0..10 {\n        if i % 2 == 0 { total += i; }\n    }\n    println!(\"{name} {total}\");\n}\n",
    )
    .unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &code);

    let Doc::Text(doc) = &app.tabs.active().unwrap().doc else { panic!() };
    assert_eq!(doc.hl.lang, Some(Lang::Rust));
    assert!(doc.hl.spans.is_none(), "nothing parsed yet");
    assert!(doc.hl.in_flight, "opening a file starts the first parse immediately");

    run_highlight_job(&mut app);
    let Doc::Text(doc) = &app.tabs.active().unwrap().doc else { panic!() };
    let spans = doc.hl.spans.clone().expect("spans delivered");
    assert!(spans.line(7).iter().any(|s| s.group == Group::Comment), "line 8 has a comment");
    assert!(!app.wants_highlight(), "up to date -> tick subscription off");
    shot(&app, "15-syntax-dark");

    // Typing makes it stale again; the next job is incremental and updates spans.
    let id = app.tabs.active_id().unwrap();
    for c in "// ".chars() {
        drop(app.update(Message::Editor(id, Action::Insert(c.to_string()))));
    }
    assert!(app.wants_highlight());
    run_highlight_job(&mut app);
    let Doc::Text(doc) = &app.tabs.active().unwrap().doc else { panic!() };
    assert!(
        !Arc::ptr_eq(&spans, doc.hl.spans.as_ref().unwrap()),
        "new spans after an edit"
    );
    assert!(doc.hl.spans.as_ref().unwrap().line(0).iter().any(|s| s.group == Group::Comment));

    // Theme switch re-colors without re-parsing (colors are part of the settings).
    app.settings.theme = "light".into();
    shot(&app, "16-syntax-light");

    // Unknown extension: plain text, and no worker is ever started.
    let plain = root.join("plain.txt");
    fs::write(&plain, "fn not code\n").unwrap();
    open(&mut app, &plain);
    assert!(!app.wants_highlight());
    assert_eq!(app.tabs.active().unwrap().title, "plain.txt");

    // Save As to a Rust name turns highlighting on.
    drop(app.update(Message::NewFile));
    let id = app.tabs.active_id().unwrap();
    drop(app.update(Message::Saved { id, path: root.join("fresh.rs"), version: 1, then_close: false, result: Ok(0) }));
    assert!(app.wants_highlight() || matches!(&app.tabs.get(id).unwrap().doc, Doc::Text(d) if d.hl.lang == Some(Lang::Rust)));
}

fn ctrl_key(c: &str) -> iced::Event {
    iced::Event::Keyboard(keyboard::Event::KeyPressed {
        key: keyboard::Key::Character(c.into()),
        modified_key: keyboard::Key::Character(c.into()),
        physical_key: keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified),
        location: keyboard::Location::Standard,
        modifiers: keyboard::Modifiers::CTRL,
        text: None,
        repeat: false,
    })
}

/// Drives the real widget with simulated input; returns the actions it emitted.
fn editor_actions(app: &App, drive: impl FnOnce(&mut iced_test::Simulator<'_, Message>)) -> Vec<Action> {
    let mut ui = iced_test::simulator(app.view());
    drive(&mut ui);
    ui.into_messages()
        .filter_map(|m| match m {
            Message::Editor(_, a) => Some(a),
            _ => None,
        })
        .collect()
}

/// A numpad key press as the system reports it: `key` is read without modifiers, so it is
/// always the NumLock-off key (KP_1 = End), while `modified_key` and `text` say what the key
/// does right now.
fn numpad_key(code: keyboard::key::Code, key: keyboard::key::Named, logical: keyboard::Key, text: Option<&str>) -> iced::Event {
    iced::Event::Keyboard(keyboard::Event::KeyPressed {
        key: keyboard::Key::Named(key),
        modified_key: logical,
        physical_key: keyboard::key::Physical::Code(code),
        location: keyboard::Location::Numpad,
        modifiers: keyboard::Modifiers::empty(),
        text: text.map(Into::into),
        repeat: false,
    })
}

#[test]
fn numpad_digits_are_typed_with_numlock_on_and_move_the_caret_with_it_off() {
    use keyboard::key::{Code, Named};
    let root = project();
    let file = root.join("numpad.txt");
    fs::write(&file, "abc\ndef\n").unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    drop(editor_actions(&app, |_| {})); // first layout measures the text
    let m = match &app.tabs.active().unwrap().doc { Doc::Text(d) => d.state.metrics(), _ => panic!() };
    let click = iced::Point::new(sidebar::DEFAULT_WIDTH + 5.0 + 120.0, TOP + 0.5 * m.line_h);
    let digit = |s: &str| keyboard::Key::Character(s.into());

    // NumLock on: KP_1 / KP_4 / KP_2 / KP_0 arrive with the End / Left / Down / Insert key
    // and the digit as text - they have to type.
    let acts = editor_actions(&app, |ui| {
        ui.point_at(click);
        ui.simulate(iced_test::simulator::click());
        ui.simulate([
            numpad_key(Code::Numpad1, Named::End, digit("1"), Some("1")),
            numpad_key(Code::Numpad4, Named::ArrowLeft, digit("4"), Some("4")),
            numpad_key(Code::Numpad2, Named::ArrowDown, digit("2"), Some("2")),
            numpad_key(Code::Numpad0, Named::Insert, digit("0"), Some("0")),
            numpad_key(Code::NumpadDecimal, Named::Delete, digit("."), Some(".")),
        ]);
    });
    let typed: Vec<&str> = acts.iter().filter_map(|a| if let Action::Insert(t) = a { Some(t.as_str()) } else { None }).collect();
    assert_eq!(typed, ["1", "4", "2", "0", "."], "{acts:?}");
    assert!(!acts.iter().any(|a| matches!(a, Action::Move { .. } | Action::Delete)), "no key acted as navigation: {acts:?}");

    // NumLock off: the same keys are the navigation keys.
    let acts = editor_actions(&app, |ui| {
        ui.point_at(click);
        ui.simulate(iced_test::simulator::click());
        ui.simulate([
            numpad_key(Code::Numpad1, Named::End, keyboard::Key::Named(Named::End), None),
            numpad_key(Code::Numpad4, Named::ArrowLeft, keyboard::Key::Named(Named::ArrowLeft), None),
        ]);
    });
    assert!(
        matches!(&acts[1..], [Action::Move { motion: Motion::LineEnd, .. }, Action::Move { motion: Motion::Left, .. }]),
        "{acts:?}"
    );
}

fn apply_all(app: &mut App, id: TabId, actions: Vec<Action>) {
    for a in actions {
        drop(app.update(Message::Editor(id, a)));
    }
}

#[test]
fn the_widget_turns_mouse_and_keyboard_into_edits() {
    let root = project();
    let file = root.join("typing.txt");
    fs::write(&file, "hello world\nsecond line\n").unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();
    let state = |app: &App| match &app.tabs.get(id).unwrap().doc {
        Doc::Text(d) => (d.text(), d.dirty(), d.state.selection()),
        _ => panic!(),
    };
    let m = match &app.tabs.get(id).unwrap().doc { Doc::Text(d) => d.state.metrics(), _ => panic!() };
    assert_eq!(m.char_w, 0.0, "metrics are measured during the first layout");
    drop(editor_actions(&app, |_| {}));
    let m = match &app.tabs.get(id).unwrap().doc { Doc::Text(d) => d.state.metrics(), _ => panic!() };
    assert!(m.char_w > 5.0 && m.line_h >= 18.0 && m.text_w > 300.0, "{m:?}");

    // Editor origin: 261 px sidebar, ~33 px tab bar; gutter = 5 cells, then 6 px padding.
    let (ox, oy) = (sidebar::DEFAULT_WIDTH + 5.0, TOP);
    let at = |col: f32, row: f32| iced::Point::new(ox + (6.2 + col) * m.char_w + 6.0, oy + (row + 0.5) * m.line_h);

    // Click between "hello" and " world" -> caret at column 5.
    let acts = editor_actions(&app, |ui| {
        ui.point_at(at(5.0, 0.0));
        ui.simulate(iced_test::simulator::click());
    });
    assert!(
        matches!(acts.as_slice(), [Action::Click { line: 0, col: 5, extend: false, count: 1, add: false }]),
        "{acts:?}"
    );
    apply_all(&mut app, id, acts);

    // The click focused the widget, so typing works (same simulator session needed
    // for focus: do it in one go).
    let acts = editor_actions(&app, |ui| {
        ui.point_at(at(5.0, 0.0));
        ui.simulate(iced_test::simulator::click());
        ui.typewrite("\u{11f}x"); // ğx
        ui.simulate([ctrl_key("z")]);
    });
    assert!(matches!(&acts[1], Action::Insert(t) if t == "\u{11f}"), "{acts:?}");
    assert!(matches!(&acts[2], Action::Insert(t) if t == "x"));
    assert!(matches!(acts.last(), Some(Action::Undo)));
    apply_all(&mut app, id, acts);
    assert_eq!(state(&app).0, "hello world\nsecond line\n", "typed two chars then undid them in one step");
    assert!(!state(&app).1, "back at the saved state -> clean");

    // Double click selects a word; drag extends.
    let acts = editor_actions(&app, |ui| {
        ui.point_at(at(8.0, 0.0));
        ui.simulate(iced_test::simulator::click());
        ui.simulate(iced_test::simulator::click());
    });
    assert!(matches!(acts.last(), Some(Action::Click { count: 2, .. })), "{acts:?}");
    apply_all(&mut app, id, acts);
    let Doc::Text(d) = &app.tabs.get(id).unwrap().doc else { panic!() };
    assert_eq!(d.state.selected_text().as_deref(), Some("world"));

    // Wheel scrolls; Shift+wheel scrolls sideways.
    let acts = editor_actions(&app, |ui| {
        ui.point_at(at(3.0, 2.0));
        ui.simulate([iced::Event::Mouse(iced::mouse::Event::WheelScrolled {
            delta: iced::mouse::ScrollDelta::Lines { x: 0.0, y: -2.0 },
        })]);
    });
    assert!(matches!(acts.as_slice(), [Action::Scroll { dx, dy }] if *dx == 0.0 && *dy > 0.0), "{acts:?}");
}

#[test]
fn minimap_click_scrolls_and_long_documents_render() {
    let root = project();
    let file = root.join("long.rs");
    let body: String = (0..3000).map(|i| format!("fn f{i}() {{ let x = {i}; // line {i}\n}}\n")).collect();
    fs::write(&file, &body).unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    run_highlight_job(&mut app);
    let id = app.tabs.active_id().unwrap();
    drop(editor_actions(&app, |_| {}));
    let m = match &app.tabs.get(id).unwrap().doc { Doc::Text(d) => d.state.metrics(), _ => panic!() };

    // Click the middle of the minimap (right edge of the 1024 px window).
    let acts = editor_actions(&app, |ui| {
        ui.point_at(iced::Point::new(1024.0 - 50.0, TOP + 384.0));
        ui.simulate(iced_test::simulator::click());
    });
    let Some(Action::ScrollTo { y }) = acts.first().cloned() else { panic!("{acts:?}") };
    // At scroll 0 the minimap shows the first ~380 lines; its middle is line ~190.
    let expected = (384.0 / 2.0) * m.line_h - m.view_h / 2.0;
    assert!((y - expected).abs() < 2.0 * m.line_h, "minimap row -> file line: {y} vs {expected}");
    apply_all(&mut app, id, acts);
    // Now drag the minimap's viewport towards the bottom: the minimap itself
    // scrolls proportionally, so the lower part of the file becomes reachable.
    let acts = editor_actions(&app, |ui| {
        ui.point_at(iced::Point::new(1024.0 - 50.0, TOP + 700.0));
        ui.simulate(iced_test::simulator::click());
    });
    apply_all(&mut app, id, acts);
    shot(&app, "17-minimap-middle");

    // Jump around: end of file, back to the start; caret follows.
    apply_all(&mut app, id, vec![Action::Move { motion: Motion::DocEnd, extend: false }]);
    let Doc::Text(d) = &app.tabs.get(id).unwrap().doc else { panic!() };
    assert!(d.state.scroll_y > 5000.0 * m.line_h);
    shot(&app, "18-end-of-file");
}

#[test]
fn scrollbars_follow_the_scroll_position_and_can_be_dragged() {
    let root = project();
    let file = root.join("scrollbars.rs");
    let wide = "x".repeat(400);
    let body: String = (0..400).map(|i| if i == 5 { format!("let wide = \"{wide}\";\n") } else { format!("let v{i} = {i};\n") }).collect();
    fs::write(&file, &body).unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    run_highlight_job(&mut app);
    let id = app.tabs.active_id().unwrap();
    drop(editor_actions(&app, |_| {})); // layout pass: metrics
    let state = |app: &App| match &app.tabs.get(id).unwrap().doc {
        Doc::Text(d) => (d.state.scroll_x, d.state.scroll_y, d.state.max_scroll_y(), d.state.metrics()),
        _ => panic!(),
    };
    let (_, _, max_y, m) = state(&app);
    assert!(max_y > 1000.0, "long document: {max_y}");

    // Clicking the middle of the vertical scrollbar centers the thumb there.
    let acts = editor_actions(&app, |ui| {
        ui.point_at(iced::Point::new(1024.0 - 6.0, TOP + m.view_h / 2.0));
        ui.simulate(iced_test::simulator::click());
    });
    let Some(Action::ScrollTo { y }) = acts.first().cloned() else { panic!("{acts:?}") };
    assert!((y - max_y / 2.0).abs() < max_y * 0.05, "{y} vs {}", max_y / 2.0);
    apply_all(&mut app, id, acts);
    shot(&app, "29-scrollbars");

    // The horizontal bar appears for the long line and scrolls sideways.
    apply_all(&mut app, id, vec![Action::ScrollTo { y: 0.0 }]);
    let acts = editor_actions(&app, |ui| {
        ui.point_at(iced::Point::new(700.0, TOP + m.view_h - 6.0));
        ui.simulate(iced_test::simulator::click());
    });
    let Some(Action::ScrollToX { x }) = acts.first().cloned() else { panic!("{acts:?}") };
    assert!(x > 0.0);
    apply_all(&mut app, id, acts);
    assert!(state(&app).0 > 0.0, "scrolled right");
    shot(&app, "30-scrollbar-horizontal");
}

#[test]
fn right_click_menu_menu_bar_and_file_operations() {
    use iced::{Event, mouse};
    let root = scratch().join("ctxmenu");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.php"), "<?php echo 1;\n").unwrap();
    let mut app = booted(vec![root.clone()]);
    scan_all(&mut app, &root);

    // A real right click on the first row asks for the context menu.
    let msgs: Vec<Message> = {
        let mut ui = iced_test::simulator(app.view());
        ui.point_at(iced::Point::new(120.0, menu_bar::HEIGHT + 33.0 + 12.0));
        ui.simulate([Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right))]);
        ui.into_messages().collect()
    };
    assert!(
        msgs.iter().any(|m| matches!(m, Message::TreeContext { is_root: true, path, .. } if *path == root)),
        "right click on the root row -> context menu request"
    );

    // Folder menu.
    drop(app.update(Message::TreeContext { path: root.join("src"), is_dir: true, is_root: false }));
    assert!(matches!(&app.popup, Some(Popup::Context { entries, .. }) if entries.len() >= 8));
    shot(&app, "31-context-menu");
    // Picking an entry closes the menu and runs it.
    drop(app.update(Message::MenuPick(Box::new(Message::NewFileIn(root.join("src"))))));
    assert!(app.popup.is_none() && app.prompt.is_some());
    shot(&app, "32-new-file-prompt");

    // Creating: a bad name stays in the prompt with an error, a good one opens the file.
    drop(app.update(Message::PromptInput("../evil.php".into())));
    let r = fileops::create_file(&root.join("src"), &app.prompt.as_ref().unwrap().text);
    drop(app.update(Message::FileOp(r)));
    assert!(app.prompt.as_ref().unwrap().error.is_some() && !root.join("evil.php").exists());
    drop(app.update(Message::PromptInput("b/new.php".into())));
    let r = fileops::create_file(&root.join("src"), &app.prompt.as_ref().unwrap().text);
    drop(app.update(Message::FileOp(r)));
    assert!(app.prompt.is_none() && root.join("src/b/new.php").is_file());

    // Renaming a file that is open in a tab retargets the tab (and its language).
    let a = root.join("src/a.php");
    open(&mut app, &a);
    let id = app.tabs.by_path(&a).unwrap();
    drop(app.update(Message::RenameEntry(a.clone())));
    drop(app.update(Message::PromptInput("renamed.txt".into())));
    let r = fileops::rename(&a, &app.prompt.as_ref().unwrap().text);
    drop(app.update(Message::FileOp(r)));
    let tab = app.tabs.get(id).unwrap();
    assert_eq!(tab.path.as_deref(), Some(root.join("src/renamed.txt").as_path()));
    assert_eq!(tab.title, "renamed.txt");
    assert!(root.join("src/renamed.txt").is_file() && !a.exists());

    // Menu bar: drop-downs open, hover switches between them, a pick closes it.
    drop(app.update(Message::OpenMenu(2)));
    shot(&app, "33-menu-bar-view");
    drop(app.update(Message::MenuHover(0)));
    assert!(matches!(app.popup, Some(Popup::Menu(0))));
    let before = app.settings.word_wrap;
    drop(app.update(Message::OpenMenu(0)));
    assert!(app.popup.is_none(), "clicking the open menu closes it");
    drop(app.update(Message::OpenMenu(2)));
    drop(app.update(Message::MenuPick(Box::new(Message::ToggleSetting(Toggle::WordWrap)))));
    assert!(app.popup.is_none());
    assert_eq!(app.settings.word_wrap, !before);
    // Esc closes a menu, then the next Esc has nothing left to close.
    drop(app.update(Message::OpenMenu(1)));
    drop(app.update(Message::Escape));
    assert!(app.popup.is_none());
    for i in 0..menu_bar::MENUS.len() {
        assert!(!app.menu_entries(i).is_empty());
    }
}

#[test]
fn editor_right_click_menu_sidebar_resize_and_markdown_preview() {
    use iced::{Event, mouse};
    let root = scratch().join("m7");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    for i in 0..60 {
        fs::write(root.join(format!("file{i:02}.txt")), "x").unwrap();
    }
    let page = root.join("page.html.twig");
    fs::write(&page, "<ul>\n{% for u in users %}\n  <li>{{ u.name|upper }}</li>\n{% endfor %}\n</ul>\n{# done #}\n").unwrap();
    let readme = root.join("README.md");
    fs::write(&readme, "# Title\n\nSome *emphasis*, `code` and a [link](https://example.org).\n\n- one\n- two\n\n```rust\nfn main() {}\n```\n").unwrap();
    let mut app = booted(vec![root.clone()]);
    scan_all(&mut app, &root);
    shot(&app, "34-sidebar-scrollbar-gap");

    // --- editor right click: caret moves under the pointer and the menu is requested.
    open(&mut app, &page);
    run_highlight_job(&mut app);
    drop(editor_actions(&app, |_| {}));
    let m = match &app.tabs.active().unwrap().doc { Doc::Text(d) => d.state.metrics(), _ => panic!() };
    let sidebar = sidebar::DEFAULT_WIDTH + 5.0;
    let (msgs, acts) = {
        let mut ui = iced_test::simulator(app.view());
        ui.point_at(iced::Point::new(sidebar + 90.0, TOP + 2.5 * m.line_h));
        ui.simulate([Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right))]);
        let msgs: Vec<Message> = ui.into_messages().collect();
        let acts: Vec<Action> = msgs.iter().filter_map(|m| match m { Message::Editor(_, a) => Some(a.clone()), _ => None }).collect();
        (msgs, acts)
    };
    assert!(msgs.iter().any(|m| matches!(m, Message::EditorContext)), "menu requested");
    assert!(matches!(acts.first(), Some(Action::Click { line: 2, extend: false, .. })), "caret goes to the clicked line: {acts:?}");
    let id = app.tabs.active_id().unwrap();
    apply_all(&mut app, id, acts);
    drop(app.update(Message::EditorContext));
    let Some(Popup::Context { entries, .. }) = &app.popup else { panic!("menu open") };
    let enabled = |label: &str| entries.iter().any(|e| matches!(e, Entry::Item { label: l, enabled: true, .. } if l == label));
    assert!(enabled("Paste") && enabled("Select All") && enabled("Find..."), "basic entries");
    assert!(!enabled("Copy") && !enabled("Cut"), "nothing selected -> copy/cut disabled");
    assert!(entries.iter().any(|e| matches!(e, Entry::Item { label, .. } if label == "Reveal in File Manager")), "file entries");
    // With a selection they come alive; the click inside a selection keeps it.
    drop(app.update(Message::CloseMenu));
    drop(app.update(Message::EditAction(Action::SelectAll)));
    drop(app.update(Message::EditorContext));
    let Some(Popup::Context { entries, .. }) = &app.popup else { panic!() };
    assert!(entries.iter().any(|e| matches!(e, Entry::Item { label, enabled: true, .. } if label == "Copy")));
    drop(app.update(Message::CloseMenu));
    {
        let Doc::Text(d) = &app.tabs.active().unwrap().doc else { panic!() };
        assert!(d.state.point_in_selection(1, 4) && !d.state.point_in_selection(99, 0));
    }
    // Twig is colored on top of the HTML.
    let Doc::Text(d) = &app.tabs.active().unwrap().doc else { panic!() };
    assert_eq!(d.hl.lang, Some(Lang::Twig));
    shot(&app, "35-twig");

    // --- sidebar resize: only while the divider is held; clamped; remembered.
    drop(app.update(Message::SidebarResizeTo(500.0)));
    assert_eq!(app.sidebar_width, sidebar::DEFAULT_WIDTH, "ignored without a drag");
    drop(app.update(Message::SidebarResizeStart));
    drop(app.update(Message::SidebarResizeTo(420.0)));
    assert_eq!(app.sidebar_width, 420.0);
    drop(app.update(Message::SidebarResizeTo(5.0)));
    assert_eq!(app.sidebar_width, sidebar::MIN_WIDTH);
    drop(app.update(Message::SidebarResizeTo(9000.0)));
    assert_eq!(app.sidebar_width, sidebar::MAX_WIDTH);
    drop(app.update(Message::SidebarResizeTo(300.0)));
    drop(app.update(Message::SidebarResizeEnd));
    assert!(!app.resizing_sidebar && app.session_dirty);
    assert_eq!(app.snapshot().file.sidebar_width, 300.0, "saved with the session");
    // The divider itself starts the drag.
    app.sidebar_width = sidebar::DEFAULT_WIDTH;
    let msgs: Vec<Message> = {
        let mut ui = iced_test::simulator(app.view());
        ui.point_at(iced::Point::new(sidebar::DEFAULT_WIDTH + 2.5, 400.0));
        ui.simulate(iced_test::simulator::click());
        ui.into_messages().collect()
    };
    assert!(msgs.iter().any(|m| matches!(m, Message::SidebarResizeStart)), "{}", msgs.len());

    // --- Markdown: source + rendered preview side by side.
    open(&mut app, &readme);
    let id = app.tabs.active_id().unwrap();
    {
        let Doc::Text(d) = &app.tabs.get(id).unwrap().doc else { panic!() };
        assert_eq!(d.hl.lang, Some(Lang::Markdown));
        let p = d.preview.as_ref().expect("preview built right after opening");
        assert!(p.weight() > 0 && p.version == d.version() && p.window.is_none());
    }
    shot(&app, "36-markdown-preview");
    // Typing re-renders; the preview never lags behind the text.
    apply_all(&mut app, id, vec![Action::Insert("INTRO\n\n".into())]);
    {
        let Doc::Text(d) = &app.tabs.get(id).unwrap().doc else { panic!() };
        assert_eq!(d.preview.as_ref().unwrap().version, d.version());
    }
    // Switching it off removes the pane; the menu entry reflects the setting.
    drop(app.update(Message::ToggleSetting(Toggle::MarkdownPreview)));
    assert!(!app.settings.markdown_preview && !app.preview_active());
    shot(&app, "37-markdown-source-only");
    drop(app.update(Message::ToggleSetting(Toggle::MarkdownPreview)));
    assert!(app.preview_active());
    // Links: web links go to the desktop, relative ones open the file next to the document.
    fs::write(root.join("OTHER.md"), "# other\n").unwrap();
    drop(app.update(Message::OpenLink("OTHER.md#top".into())));
    open(&mut app, &root.join("OTHER.md"));
    assert!(app.tabs.by_path(&root.join("OTHER.md")).is_some());
    drop(app.update(Message::OpenLink("nope.md".into())));
    assert!(app.notice.as_deref().is_some_and(|n| n.contains("nope.md")));
}

#[test]
fn tab_strip_scrolls_lists_all_tabs_and_keeps_the_active_one_visible() {
    use iced::{Event, mouse};
    let root = scratch().join("manytabs");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let mut app = booted(vec![]);
    let files: Vec<PathBuf> = (0..30).map(|i| root.join(format!("module_number_{i:02}.php"))).collect();
    for f in &files {
        fs::write(f, "<?php\n").unwrap();
        open(&mut app, f);
    }
    let last = app.tabs.active_id().unwrap();
    let first = app.tabs.by_path(&files[0]).unwrap();
    let (_, end) = tab_bar::span_of(&app.tabs, last).unwrap();
    assert!(end > 3000.0, "30 tabs are far wider than the window: {end}");
    assert_eq!(tab_bar::span_of(&app.tabs, first).unwrap().0, 0.0);

    // The strip reports its size; the active (last) tab is scrolled into view.
    drop(app.update(Message::TabsViewport { x: 0.0, width: 800.0, content: end }));
    assert!(app.tab_x > 0.0 && app.tab_x + 800.0 >= end - 1.0, "last tab revealed: x={}", app.tab_x);
    // Activating the first tab scrolls back; one in view does not move the strip.
    drop(app.update(Message::ActivateTab(first)));
    assert_eq!(app.tab_x, 0.0);
    let mid = app.tabs.by_path(&files[2]).unwrap();
    drop(app.update(Message::ActivateTab(mid)));
    assert_eq!(app.tab_x, 0.0, "already visible");

    // Wheel: down = towards later tabs, clamped at both ends.
    drop(app.update(Message::TabsWheel(120.0)));
    assert_eq!(app.tab_x, 120.0);
    drop(app.update(Message::TabsWheel(-9999.0)));
    assert_eq!(app.tab_x, 0.0);
    drop(app.update(Message::TabsWheel(1e9)));
    assert_eq!(app.tab_x, end - 800.0);
    // A real wheel event over the strip becomes that message.
    let msgs: Vec<Message> = {
        let mut ui = iced_test::simulator(app.view());
        ui.point_at(iced::Point::new(400.0, menu_bar::HEIGHT + 16.0));
        ui.simulate([Event::Mouse(mouse::Event::WheelScrolled { delta: mouse::ScrollDelta::Lines { x: 0.0, y: -2.0 } })]);
        ui.into_messages().collect()
    };
    let tags: Vec<String> = msgs.iter().map(|m| match m { Message::TabsWheel(px) => format!("wheel{px}"), Message::TabsViewport { x, width, content } => format!("vp{x}/{width}/{content}"), _ => "other".into() }).collect();
    assert!(msgs.iter().any(|m| matches!(m, Message::TabsWheel(px) if *px > 0.0)), "{tags:?}");
    assert!(tab_bar::wheel_px(mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 }) < 0.0, "wheel up scrolls back");

    // The ▾ list shows every tab (scrolls when taller than the window), marks the active one.
    drop(app.update(Message::OpenTabList));
    let Some(Popup::Context { entries, width, .. }) = &app.popup else { panic!("list open") };
    assert_eq!(entries.len(), 30);
    assert!(*width > menu::WIDTH);
    assert!(matches!(&entries[2], Entry::Item { label, checked: Some(true), .. } if label == "module_number_02.php"));
    assert!(matches!(&entries[0], Entry::Item { hint, .. } if hint.ends_with("manytabs")), "folder shown as hint");
    shot(&app, "38-tab-list");
    let pick = match &entries[27] { Entry::Item { msg, .. } => msg.clone(), _ => panic!() };
    drop(app.update(Message::MenuPick(Box::new(pick))));
    assert_eq!(app.tabs.active().unwrap().title, "module_number_27.php");
    assert!(app.popup.is_none());
    shot(&app, "39-tab-strip");

    // Help > About is a real dialog.
    drop(app.update(Message::OpenMenu(5)));
    drop(app.update(Message::MenuPick(Box::new(Message::About))));
    assert!(matches!(app.dialog, Some(Dialog::About)));
    shot(&app, "40-about");
    drop(app.update(Message::Escape));
    assert!(app.dialog.is_none());
}

#[test]
fn sidebar_files_carry_colored_type_badges() {
    let root = scratch().join("badges");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src")).unwrap();
    for f in ["index.php", "app.js", "style.css", "README.md", "base.html.twig", "data.json", "Dockerfile", ".env", "notes.txt", "photo.png", "Makefile", "main.rs", "weird.xyz"] {
        fs::write(root.join(f), "x").unwrap();
    }
    let mut app = booted(vec![root.clone()]);
    scan_all(&mut app, &root);
    shot(&app, "41-sidebar-badges");
    drop(app.update(Message::ToggleSetting(Toggle::MenuBar)));
    drop(app.update(Message::SetTheme("light".into())));
    shot(&app, "42-sidebar-badges-light");
}

#[test]
fn markdown_tables_wrap_in_columns_and_the_preview_divider_drags() {
    let root = scratch().join("mdtable");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let long = "gerekçe metni ".repeat(30);
    let md = root.join("T.md");
    fs::write(&md, format!("# Kararlar\n\n| ID | Karar | Gerekçe | Durum |\n|---|---|---|---|\n| T-01 | **PHP 8.4** | {long} | \u{2705} |\n| T-02 | `x` ~~eski~~ | {long} | \u{1F7E1} |\n\nson\n")).unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &md);
    let Doc::Text(d) = &app.tabs.active().unwrap().doc else { panic!() };
    let p = d.preview.as_ref().unwrap();
    let Some(crate::editor::preview::Block::Table(t)) = p.blocks.iter().find(|b| matches!(b, crate::editor::preview::Block::Table(_))) else { panic!("table block") };
    assert_eq!((t.header.len(), t.rows.len()), (4, 2));
    shot(&app, "44-markdown-table");

    // The preview/editor divider: dragging moves the split, clamped, and it is saved.
    drop(app.update(Message::WindowResized(iced::Size::new(1200.0, 800.0))));
    drop(app.update(Message::PreviewResizeTo(300.0)));
    assert_eq!(app.preview_frac, 0.5, "ignored unless dragging");
    drop(app.update(Message::PreviewResizeStart));
    let left = sidebar::DEFAULT_WIDTH + 5.0;
    drop(app.update(Message::PreviewResizeTo(left + (1200.0 - left) * 0.3)));
    assert!((app.preview_frac - 0.7).abs() < 0.001, "{}", app.preview_frac);
    drop(app.update(Message::PreviewResizeTo(1190.0)));
    assert_eq!(app.preview_frac, 0.2, "never smaller than a fifth");
    drop(app.update(Message::PreviewResizeTo(0.0)));
    assert_eq!(app.preview_frac, 0.85);
    drop(app.update(Message::PreviewResizeTo(left + (1200.0 - left) * 0.4)));
    drop(app.update(Message::PreviewResizeEnd));
    assert!(!app.resizing_preview && (app.snapshot().file.preview_frac - 0.6).abs() < 0.001);
    shot(&app, "45-markdown-wide-preview");
    // The divider itself starts the drag.
    app.preview_frac = 0.5;
    let msgs: Vec<Message> = {
        let mut ui = iced_test::simulator(app.view());
        // 1024 px window: the divider sits at the middle of the area right of the sidebar.
        let x = left + (1024.0 - left) / 2.0;
        ui.point_at(iced::Point::new(x, 400.0));
        ui.simulate(iced_test::simulator::click());
        ui.into_messages().collect()
    };
    assert!(msgs.iter().any(|m| matches!(m, Message::PreviewResizeStart)), "{}", msgs.len());
}

#[test]
fn code_folding_by_gutter_arrow_keys_and_commands() {
    let root = scratch().join("folding");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let file = root.join("Service.php");
    let src = "<?php\nclass Service\n{\n    public function run($x)\n    {\n        if ($x) {\n            return 1;\n        }\n\n        return 2;\n    }\n\n    public function stop()\n    {\n        return 3;\n    }\n}\n";
    fs::write(&file, src).unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    run_highlight_job(&mut app);
    let id = app.tabs.active_id().unwrap();
    drop(editor_actions(&app, |_| {})); // layout: metrics
    let (m, left) = match &app.tabs.get(id).unwrap().doc {
        Doc::Text(d) => (d.state.metrics(), sidebar::DEFAULT_WIDTH + 5.0),
        _ => panic!(),
    };
    let state = |app: &App| match &app.tabs.get(id).unwrap().doc {
        Doc::Text(d) => (d.state.has_folds(), d.state.total_rows(), d.state.line_count()),
        _ => panic!(),
    };
    assert_eq!(state(&app).0, false);
    // The fold arrow sits in the gutter's right margin; a click there folds the block under it.
    let gutter_w = 6.2 * m.char_w;
    let arrow = |line: usize| iced::Point::new(left + gutter_w - m.char_w * 0.5, TOP + (line as f32 + 0.5) * m.line_h);
    let acts = editor_actions(&app, |ui| {
        ui.point_at(arrow(4));
        ui.simulate(iced_test::simulator::click());
    });
    assert!(matches!(acts.first(), Some(Action::ToggleFold { line: 4 })), "{acts:?}");
    apply_all(&mut app, id, acts);
    let (folded, rows, lines) = state(&app);
    assert!(folded && rows == lines - 6, "{rows} rows of {lines} lines");
    shot(&app, "46-code-folded");
    // Clicking a line number far from the arrow still selects the line instead.
    let acts = editor_actions(&app, |ui| {
        ui.point_at(iced::Point::new(left + 8.0, TOP + 0.5 * m.line_h));
        ui.simulate(iced_test::simulator::click());
    });
    assert!(matches!(acts.first(), Some(Action::Click { count: 3, .. })), "{acts:?}");
    // Rows after the fold are clicked at their new place: row 5 is source line 11.
    let acts = editor_actions(&app, |ui| {
        ui.point_at(iced::Point::new(left + gutter_w + 30.0, TOP + 5.5 * m.line_h));
        ui.simulate(iced_test::simulator::click());
    });
    assert!(matches!(acts.first(), Some(Action::Click { line: 11, .. })), "{acts:?}");
    // Unfold with the toggle, then with keys / commands.
    apply_all(&mut app, id, vec![Action::ToggleFold { line: 4 }]);
    assert_eq!(state(&app).0, false);
    apply_all(&mut app, id, vec![Action::FoldAll]);
    assert!(state(&app).0 && state(&app).1 < 8);
    apply_all(&mut app, id, vec![Action::UnfoldAll]);
    assert_eq!(state(&app).0, false);
    drop(app.update(Message::EditAction(Action::FoldLevel(2))));
    assert!(state(&app).0);
    shot(&app, "47-code-fold-level-2");
    // Every fold command is in the palette.
    for cmd in [Command::Fold, Command::Unfold, Command::FoldAll, Command::UnfoldAll, Command::FoldLevel(1)] {
        assert!(palette::rank_commands("fold").contains(&cmd), "{cmd:?}");
    }
}

#[test]
fn tab_menu_closes_in_batches_asking_about_unsaved_ones_and_other_launches_join_this_window() {
    let root = scratch().join("tabmenu");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let mut app = booted(vec![]);
    let files: Vec<PathBuf> = (0..5).map(|i| root.join(format!("f{i}.txt"))).collect();
    for f in &files {
        fs::write(f, "x").unwrap();
        open(&mut app, f);
    }
    let id = |app: &App, i: usize| app.tabs.by_path(&files[i]).unwrap();
    let titles = |app: &App| app.tabs.iter().map(|t| t.title.clone()).collect::<Vec<_>>();

    // The menu: Close Others / to the Right are disabled where they make no sense.
    drop(app.update(Message::TabContext(id(&app, 4))));
    let Some(Popup::Context { entries, .. }) = &app.popup else { panic!("menu") };
    let enabled = |label: &str| entries.iter().any(|e| matches!(e, Entry::Item { label: l, enabled: true, .. } if l == label));
    assert!(enabled("Close") && enabled("Close Others") && enabled("Close All") && !enabled("Close Tabs to the Right"));
    drop(app.update(Message::CloseMenu));

    // Tab 1 has unsaved changes. "Close Others" of tab 2 closes the clean ones and asks about tab 1.
    let one = id(&app, 1);
    drop(app.update(Message::ActivateTab(one)));
    apply_all(&mut app, one, vec![Action::Insert("edit".into())]);
    drop(app.update(Message::CloseOthers(id(&app, 2))));
    assert_eq!(titles(&app), vec!["f1.txt", "f2.txt", "f3.txt", "f4.txt"], "f0 closed; the unsaved f1 asks, f3/f4 wait behind it");
    assert!(matches!(app.dialog, Some(Dialog::CloseDirty(d)) if d == one));
    // Cancel keeps it and drops the rest of the batch.
    drop(app.update(Message::CloseChoice(one, CloseChoice::Cancel)));
    assert_eq!(titles(&app), vec!["f1.txt", "f2.txt", "f3.txt", "f4.txt"], "cancelling drops the rest of the batch too");
    assert!(app.close_queue.is_empty() && app.dialog.is_none());
    // Close All: the clean one goes, discarding the unsaved one finishes the batch.
    drop(app.update(Message::CloseAll));
    assert!(matches!(app.dialog, Some(Dialog::CloseDirty(d)) if d == one));
    drop(app.update(Message::CloseChoice(one, CloseChoice::Discard)));
    assert!(app.tabs.is_empty() && app.dialog.is_none());
    // To the right.
    for f in &files {
        open(&mut app, f);
    }
    drop(app.update(Message::CloseRight(id(&app, 1))));
    assert_eq!(titles(&app), vec!["f0.txt", "f1.txt"]);

    // Another launch hands us a folder: it joins the workspace (files open as tabs).
    let extra = root.join("extra");
    fs::create_dir_all(&extra).unwrap();
    drop(app.update(Message::External(vec![extra.clone()])));
    assert!(app.ws.root_paths().contains(&extra));
}

#[test]
fn ctrl_click_collapses_a_folder_tree_zoom_works_and_the_window_is_remembered() {
    use iced::{Event, keyboard, mouse};
    let root = scratch().join("ctrlzoom");
    let _ = fs::remove_dir_all(&root);
    for d in ["a/b/c", "a/d", "z"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(root.join("a/b/c/deep.txt"), "x").unwrap();
    let mut app = booted(vec![root.clone()]);
    for d in [&root, &root.join("a")] {
        scan_all(&mut app, d);
    }
    let click = |app: &mut App, p: &Path| drop(app.update(Message::TreeClicked { path: p.to_path_buf(), is_dir: true }));
    let ctrl = |app: &mut App, on: bool| {
        let m = if on { keyboard::Modifiers::CTRL } else { keyboard::Modifiers::empty() };
        drop(app.update(Message::Key(keyboard::Event::ModifiersChanged(m))));
    };
    // Open a/, a/b/ and a/b/c/ the normal way.
    click(&mut app, &root.join("a"));
    scan_all(&mut app, &root.join("a"));
    click(&mut app, &root.join("a/b"));
    scan_all(&mut app, &root.join("a/b"));
    click(&mut app, &root.join("a/b/c"));
    let expanded = |app: &mut App| (app.ws.is_expanded(&root.join("a")), app.ws.is_expanded(&root.join("a/b")), app.ws.is_expanded(&root.join("a/b/c")));
    assert_eq!(expanded(&mut app), (true, true, true));
    // Plain click collapses only that folder; its children keep their state.
    click(&mut app, &root.join("a"));
    assert_eq!(expanded(&mut app), (false, true, true));
    click(&mut app, &root.join("a"));
    assert_eq!(expanded(&mut app), (true, true, true));
    // Ctrl+click on an OPEN folder collapses everything under it...
    ctrl(&mut app, true);
    click(&mut app, &root.join("a"));
    assert_eq!(expanded(&mut app), (false, false, false));
    // ...but on a CLOSED folder it opens just that one (the children stay closed).
    click(&mut app, &root.join("a"));
    assert_eq!(expanded(&mut app), (true, false, false));
    ctrl(&mut app, false);

    // ---- zoom
    let default = Settings::default().font_size;
    assert_eq!(app.settings.font_size(), default);
    let unknown = keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
    let ch = |c: &str| keyboard::Key::Character(c.into());
    // US layout: Ctrl+= and Ctrl+Shift+= ("+"), Ctrl+-.
    for (key, modified, mods, want) in [
        ("=", "=", keyboard::Modifiers::CTRL, 1),
        ("=", "+", keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT, 1),
        ("-", "-", keyboard::Modifiers::CTRL, -1),
        // Turkish Q layout: "+" is Shift+4 and Ctrl reports the unshifted "4" as the key.
        ("4", "+", keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT, 1),
        ("0", "=", keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT, 1),
    ] {
        let m = shortcut(&ch(key), &ch(modified), unknown, mods);
        assert!(matches!(m, Some(Message::ChangeFontSize(d)) if d == want), "{key}/{modified}");
    }
    let zero = shortcut(&ch("0"), &ch("0"), unknown, keyboard::Modifiers::CTRL);
    assert!(matches!(zero, Some(Message::ResetFontSize)));
    assert!(shortcut(&ch("0"), &ch("="), unknown, keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT).is_some_and(|m| matches!(m, Message::ChangeFontSize(1))), "Ctrl+Shift+0 is '=' there, not reset");
    assert!(shortcut(&ch("="), &ch("="), unknown, keyboard::Modifiers::empty()).is_none(), "no Ctrl, no zoom: typing '=' must work");
    // Numpad keys: found by the physical key, whatever NumLock makes them type
    // ("0" with it on, Insert with it off, "+"/"-" or nothing recognisable).
    {
        use keyboard::key::{Code, Named, Physical};
        let pad = |c| Physical::Code(c);
        let insert = keyboard::Key::Named(Named::Insert);
        for key in [ch("0"), insert.clone(), keyboard::Key::Unidentified] {
            let m = shortcut(&key, &key, pad(Code::Numpad0), keyboard::Modifiers::CTRL);
            assert!(matches!(m, Some(Message::ResetFontSize)), "Ctrl+Numpad0 as {key:?}");
        }
        assert!(matches!(shortcut(&ch("+"), &ch("+"), pad(Code::NumpadAdd), keyboard::Modifiers::CTRL), Some(Message::ChangeFontSize(1))));
        assert!(matches!(shortcut(&ch("-"), &ch("-"), pad(Code::NumpadSubtract), keyboard::Modifiers::CTRL), Some(Message::ChangeFontSize(-1))));
        assert!(shortcut(&ch("0"), &ch("0"), pad(Code::Numpad0), keyboard::Modifiers::empty()).is_none(), "plain Numpad0 types a 0");
        assert!(shortcut(&insert, &insert, pad(Code::Numpad0), keyboard::Modifiers::empty()).is_none());
    }
    let file = root.join("zoom.txt");
    fs::write(&file, (0..200).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();
    drop(editor_actions(&app, |_| {}));
    apply_all(&mut app, id, vec![Action::ScrollTo { y: 40.0 * 20.0 }]);
    let top_row = |app: &App| match &app.tabs.get(id).unwrap().doc { Doc::Text(d) => d.state.scroll_y / (d.state.metrics().line_h), _ => panic!() };
    let before = top_row(&app);
    drop(app.update(Message::ChangeFontSize(4)));
    assert_eq!(app.settings.font_size(), default + 4.0);
    drop(editor_actions(&app, |_| {}));
    assert!((top_row(&app) - before).abs() < 1.0, "same lines stay on top: {before} -> {}", top_row(&app));
    assert!(app.font_dirty, "written once the zoom settles");
    drop(app.update(Message::ChangeFontSize(1000)));
    assert_eq!(app.settings.font_size(), 40.0, "clamped");
    drop(app.update(Message::ResetFontSize));
    assert_eq!(app.settings.font_size(), default);
    // Ctrl + wheel over the editor zooms instead of scrolling.
    let msgs: Vec<Message> = {
        let mut ui = iced_test::simulator(app.view());
        ui.point_at(iced::Point::new(sidebar::DEFAULT_WIDTH + 200.0, TOP + 100.0));
        ui.simulate([
            Event::Keyboard(keyboard::Event::ModifiersChanged(keyboard::Modifiers::CTRL)),
            Event::Mouse(mouse::Event::WheelScrolled { delta: mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 } }),
        ]);
        ui.into_messages().collect()
    };
    assert!(msgs.iter().any(|m| matches!(m, Message::Editor(_, Action::Zoom(1)))), "wheel up zooms in");
    assert!(!msgs.iter().any(|m| matches!(m, Message::Editor(_, Action::Scroll { .. }))), "and does not scroll");
    drop(app.update(Message::Editor(id, Action::Zoom(-2))));
    assert_eq!(app.settings.font_size(), default - 2.0);

    // ---- the window comes back as it was closed
    let size = iced::Size::new(1400.0, 900.0);
    drop(app.update(Message::WindowMaximized { size, maximized: false }));
    drop(app.update(Message::WindowMoved(iced::Point::new(120.0, 80.0))));
    let w = app.snapshot().file.window;
    assert_eq!((w.width, w.height, w.x, w.y, w.maximized), (1400.0, 900.0, Some(120.0), Some(80.0), false));
    // Maximizing keeps the *normal* size and position for the next start.
    drop(app.update(Message::WindowMaximized { size: iced::Size::new(2560.0, 1400.0), maximized: true }));
    drop(app.update(Message::WindowMoved(iced::Point::new(0.0, 0.0))));
    let w = app.snapshot().file.window;
    assert_eq!((w.width, w.height, w.x, w.y, w.maximized), (1400.0, 900.0, Some(120.0), Some(80.0), true));
}

#[test]
fn long_sidebar_names_stay_on_one_row() {
    let root = scratch().join("longnames");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    for n in ["a_really_quite_long_file_name_that_does_not_fit.php", "another_really_long_file_name_for_the_same_reason.js"] {
        fs::write(root.join(n), "x").unwrap();
    }
    let mut app = booted(vec![root.clone()]);
    scan_all(&mut app, &root);
    app.sidebar_width = sidebar::MIN_WIDTH;
    shot(&app, "48-sidebar-long-names");
}

#[test]
fn git_gutter_marks_changes_against_head_and_follows_edits() {
    use std::process::{Command, Stdio};
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let dir = project().join("repo");
    fs::create_dir_all(&dir).unwrap();
    let git = |args: &[&str]| {
        assert!(Command::new("git")
            .arg("-C").arg(&dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .stdout(Stdio::null()).stderr(Stdio::null())
            .status().unwrap().success());
    };
    let file = dir.join("tracked.rs");
    fs::write(&file, "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\nfn e() {}\n").unwrap();
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-qm", "init"]);
    // Working tree: b changed, d removed, two functions appended.
    fs::write(&file, "fn a() {}\nfn b2() {}\nfn c() {}\nfn e() {}\nfn f() {}\nfn g() {}\n").unwrap();

    let mut app = booted(vec![]);
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();
    let git_of = |app: &App| match &app.tabs.get(id).unwrap().doc {
        Doc::Text(d) => (d.git.marks.clone(), matches!(d.git.base, Base::Ready(_)), d.git.in_flight),
        _ => panic!(),
    };
    assert!(matches!(&app.tabs.get(id).unwrap().doc, Doc::Text(d) if matches!(d.git.base, Base::Loading)),
        "opening a file starts fetching its HEAD version");

    // The worker's results, delivered inline.
    let base = git::base_text(&file).map(Arc::new);
    assert!(base.is_some());
    drop(app.update(Message::GitBase { id, path: file.clone(), base: base.clone() }));
    assert!(git_of(&app).1 && git_of(&app).2, "base ready -> diff job started");
    let Doc::Text(d) = &app.tabs.get(id).unwrap().doc else { panic!() };
    let marks = Arc::new(git::diff(base.as_ref().unwrap(), &d.text()));
    drop(app.update(Message::GitMarksReady { id, marks }));

    let marks = git_of(&app).0.expect("marks delivered");
    assert_eq!(marks.get(1), Some(git::Mark::Modified), "b -> b2");
    assert_eq!(marks.get(3), Some(git::Mark::Removed), "d was removed above e");
    assert_eq!((marks.get(4), marks.get(5)), (Some(git::Mark::Added), Some(git::Mark::Added)));
    assert_eq!(marks.get(0), None);
    shot(&app, "19-git-gutter");

    // Editing makes the diff stale again.
    assert!(!app.wants_highlight() || app.wants_highlight());
    drop(app.update(Message::Editor(id, Action::Insert("x".into()))));
    assert!(app.wants_highlight(), "edit -> background refresh is wanted");

    // An untracked file (outside any repo) just has no marks.
    let loose = project().join("hello.rs");
    open(&mut app, &loose);
    let id2 = app.tabs.active_id().unwrap();
    drop(app.update(Message::GitBase { id: id2, path: loose.clone(), base: git::base_text(&loose).map(Arc::new) }));
    let Doc::Text(d) = &app.tabs.get(id2).unwrap().doc else { panic!() };
    assert!(matches!(d.git.base, Base::Absent) && d.git.marks.is_none());
}

#[test]
fn drawing_a_huge_document_costs_only_the_visible_lines() {
    let root = project();
    let file = root.join("huge.rs");
    let line = "let value_with_a_long_name = compute(argument_one, argument_two); // comment\n";
    fs::write(&file, line.repeat(210_000)).unwrap(); // ~16 MB: opens as text, far above the highlight cap
    let mut app = booted(vec![]);
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();
    let Doc::Text(d) = &app.tabs.get(id).unwrap().doc else { panic!() };
    assert!(d.hl.spans.is_none());
    assert!(!d.hl.in_flight, "analysis of a 22 MB buffer is skipped, without copying it");

    apply_all(&mut app, id, vec![Action::Scroll { dx: 0.0, dy: 2_000_000.0 }]);
    let t = std::time::Instant::now();
    let mut ui = iced_test::simulator(app.view());
    let layout = t.elapsed();
    let t = std::time::Instant::now();
    for _ in 0..10 {
        ui.snapshot(&app.theme()).unwrap();
    }
    let per_frame = t.elapsed() / 10;
    println!("layout {layout:?}, draw+readback per frame {per_frame:?}");
    assert!(layout.as_millis() < 3000, "{layout:?}"); // includes headless renderer start-up; loaded CI is slow
    assert!(per_frame.as_millis() < 1000, "{per_frame:?}");
}

/// What the search worker does, run inline.
fn deliver_find_results(app: &mut App) {
    let key = app.find.requested.clone().expect("a search was started");
    let Doc::Text(doc) = &app.tabs.active().unwrap().doc else { panic!() };
    let (matches, error) = match app.find.query.compile() {
        Ok(Some(re)) => (search::find_all(&doc.text(), &re), None),
        Ok(None) => (vec![], None),
        Err(e) => (vec![], Some(e)),
    };
    drop(app.update(Message::FindResults { key, matches: Arc::new(matches), error }));
}

fn active_text(app: &App) -> (String, Selection) {
    let Doc::Text(d) = &app.tabs.active().unwrap().doc else { panic!() };
    (d.text(), d.state.selection())
}

#[test]
fn find_selects_matches_steps_through_them_and_reports_progress() {
    let root = project();
    let file = root.join("findme.txt");
    fs::write(&file, "alpha beta\nBeta gamma\nbeta beta\n").unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);

    drop(app.update(Message::OpenFind { replace: false }));
    assert!(app.find.open);
    drop(app.update(Message::FindQuery("beta".into())));
    deliver_find_results(&mut app);
    assert_eq!(app.find.matches.len(), 4, "case-insensitive by default");
    assert_eq!(app.find_status(), "1 of 4", "incremental search selected the first match");
    assert_eq!(active_text(&app).1.range(), (6, 10));
    shot(&app, "20-find-bar");

    drop(app.update(Message::FindNext));
    assert_eq!(active_text(&app).1.range(), (11, 15));
    assert_eq!(app.find_status(), "2 of 4");
    drop(app.update(Message::FindPrev));
    drop(app.update(Message::FindPrev));
    assert_eq!(active_text(&app).1.range(), (27, 31), "wraps backwards to the last match");
    drop(app.update(Message::FindNext));
    assert_eq!(app.find_status(), "1 of 4", "and forwards to the first");

    // Case sensitive: only the lower-case ones.
    drop(app.update(Message::FindToggle(FindFlag::Case)));
    deliver_find_results(&mut app);
    assert_eq!(app.find.matches.len(), 3);
    // Invalid regex: message, no crash, no matches.
    drop(app.update(Message::FindToggle(FindFlag::Regex)));
    drop(app.update(Message::FindQuery("(".into())));
    deliver_find_results(&mut app);
    assert!(app.find.error.is_some() && app.find.matches.is_empty());
    assert!(app.find_status().len() > 0);
    // Editing the buffer invalidates the results until the next search lands.
    drop(app.update(Message::FindQuery("beta".into())));
    deliver_find_results(&mut app);
    let id = app.tabs.active_id().unwrap();
    drop(app.update(Message::Editor(id, Action::Insert("zz".into()))));
    assert!(app.live_matches().is_none(), "stale after an edit");
    assert!(app.wants_highlight(), "the tick will re-run the search");

    // Esc closes the bar and gives the editor focus back.
    drop(app.update(Message::Escape));
    assert!(!app.find.open);
}

#[test]
fn replace_one_and_replace_all_are_single_undo_steps() {
    let root = project();
    let file = root.join("replace.txt");
    fs::write(&file, "cat dog cat\nconcat cat\n").unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();

    drop(app.update(Message::OpenFind { replace: true }));
    drop(app.update(Message::FindToggle(FindFlag::Word)));
    drop(app.update(Message::FindQuery("cat".into())));
    deliver_find_results(&mut app);
    assert_eq!(app.find.matches.len(), 3, "whole words only: 'concat' is skipped");
    drop(app.update(Message::FindReplacement("fox".into())));
    shot(&app, "21-replace-bar");

    // Replace the selected match, as the worker would compute it.
    let run_replace = |app: &mut App, all: bool| {
        let (text, sel) = active_text(app);
        let re = app.find.query.compile().unwrap().unwrap();
        let version = match &app.tabs.get(id).unwrap().doc { Doc::Text(d) => d.version(), _ => panic!() };
        let (edit, count) = if all {
            search::replace_all(&text, &re, "fox", true).map_or((None, 0), |(e, n)| (Some(e), n))
        } else {
            (search::replace_at(&text, &re, sel.range().0, "fox", true), 1)
        };
        drop(app.update(Message::Replaced { id, version, edit, count, then_next: !all }));
    };
    drop(app.update(Message::ReplaceOne)); // selection is the first match -> job started
    run_replace(&mut app, false);
    assert_eq!(active_text(&app).0, "fox dog cat\nconcat cat\n");
    deliver_find_results(&mut app);
    assert_eq!(active_text(&app).1.range(), (8, 11), "moved on to the next match");

    drop(app.update(Message::ReplaceAll));
    run_replace(&mut app, true);
    assert_eq!(active_text(&app).0, "fox dog fox\nconcat fox\n");
    assert!(app.notice.as_deref().unwrap().contains("Replaced 2"));

    // Replace All = one undo step; the earlier single replace is another.
    drop(app.update(Message::Editor(id, Action::Undo)));
    assert_eq!(active_text(&app).0, "fox dog cat\nconcat cat\n");
    drop(app.update(Message::Editor(id, Action::Undo)));
    assert_eq!(active_text(&app).0, "cat dog cat\nconcat cat\n");
}

#[test]
fn ctrl_click_and_ctrl_d_give_multiple_cursors_that_edit_together() {
    let root = project();
    let file = root.join("multi.txt");
    fs::write(&file, "foo bar\nfoo baz\nfoo qux\n").unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();
    drop(editor_actions(&app, |_| {}));
    let m = match &app.tabs.get(id).unwrap().doc { Doc::Text(d) => d.state.metrics(), _ => panic!() };
    let (ox, oy) = (sidebar::DEFAULT_WIDTH + 5.0, TOP);
    let at = |col: f32, row: f32| iced::Point::new(ox + (6.2 + col) * m.char_w + 6.0, oy + (row + 0.5) * m.line_h);

    // Ctrl+click adds a cursor (modifier state comes from ModifiersChanged).
    let acts = editor_actions(&app, |ui| {
        ui.point_at(at(3.0, 0.0));
        ui.simulate(iced_test::simulator::click());
        ui.simulate([iced::Event::Keyboard(keyboard::Event::ModifiersChanged(keyboard::Modifiers::CTRL))]);
        ui.point_at(at(3.0, 1.0));
        ui.simulate(iced_test::simulator::click());
    });
    assert!(matches!(acts.last(), Some(Action::Click { add: true, line: 1, col: 3, .. })), "{acts:?}");
    apply_all(&mut app, id, acts);
    let Doc::Text(d) = &app.tabs.get(id).unwrap().doc else { panic!() };
    assert_eq!(d.state.cursor_count(), 2);

    apply_all(&mut app, id, vec![Action::Insert("!".into())]);
    assert_eq!(active_text(&app).0, "foo! bar\nfoo! baz\nfoo qux\n");
    shot(&app, "22-multi-cursor");

    // Ctrl+D from a caret: selects the word, then each next occurrence.
    apply_all(&mut app, id, vec![Action::Undo, Action::Collapse]);
    apply_all(&mut app, id, vec![
        Action::Click { line: 0, col: 1, extend: false, count: 1, add: false },
        Action::SelectNextOccurrence,
        Action::SelectNextOccurrence,
        Action::SelectNextOccurrence,
        Action::Insert("X".into()),
    ]);
    assert_eq!(active_text(&app).0, "X bar\nX baz\nX qux\n");
    // Esc with several cursors collapses them (before it would close the find bar).
    apply_all(&mut app, id, vec![Action::Undo, Action::SelectNextOccurrence]);
    let Doc::Text(d) = &app.tabs.get(id).unwrap().doc else { panic!() };
    assert!(d.state.cursor_count() >= 1);
}

#[test]
fn a_restored_draft_identical_to_the_file_is_not_dirty() {
    let _turn = session_lock();
    use crate::session::{SessionFile, Snapshot, TabKind, TabRecord};
    let root = project();
    let same = root.join("draft-same.txt");
    let changed = root.join("draft-changed.txt");
    fs::write(&same, "on disk\n").unwrap();
    fs::write(&changed, "on disk\n").unwrap();
    let records = vec![
        TabRecord { kind: TabKind::Text, path: Some(same.clone()), draft: Some("draft-same.txt".into()), hex_edits: vec![] },
        TabRecord { kind: TabKind::Text, path: Some(changed.clone()), draft: Some("draft-changed.txt".into()), hex_edits: vec![] },
    ];
    let snapshot = Snapshot {
        file: SessionFile { tabs: records.clone(), ..SessionFile::default() },
        drafts: vec![
            ("draft-same.txt".into(), "on disk\n".into()),
            ("draft-changed.txt".into(), "on disk\nplus an edit\n".into()),
        ],
    };
    scratch();
    block_on(snapshot.write()).unwrap();
    let (loaded, _) = block_on(loader::restore(records, None));
    assert_eq!(loaded.len(), 2);
    assert!(!loaded[0].dirty, "identical draft -> clean tab");
    assert!(loaded[1].dirty && matches!(&loaded[1].body, Body::Text(t) if t.ends_with("plus an edit\n")));
}

/// Runs the search the palette would have started (tasks don't run in tests).
fn finish_search(app: &mut App, roots: &[PathBuf]) {
    let seq = app.palette_seq;
    let p = app.palette.as_ref().unwrap();
    let re = p.search_query().compile().unwrap().unwrap();
    let roots = match &p.scope {
        Some(s) => vec![s.clone()],
        None => roots.to_vec(),
    };
    let outcome = project_search::run(&roots, &p.filter.clone(), &re, || false);
    drop(app.update(Message::SearchResults { seq, outcome: Arc::new(outcome) }));
}

#[test]
fn find_in_files_groups_hits_per_file_with_filter_scope_and_keyboard_scrolling() {
    use crate::palette::SearchRow;
    let root = scratch().join("phpsearch");
    let _ = fs::remove_dir_all(&root);
    for d in ["app/Http", "app/Models", "vendor/lib", "resources/views"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    let long = format!("<?php // {} $user->login(); {}\n", "padding ".repeat(30), "tail ".repeat(40));
    fs::write(root.join("app/Http/UserController.php"), format!("<?php\n$user = 1;\nif ($user) {{ return $user; }}\n{long}")).unwrap();
    fs::write(root.join("app/Models/User.php"), "<?php\nclass User {\n    public $user;\n}\n").unwrap();
    fs::write(root.join("vendor/lib/x.php"), "<?php\n$user = 'vendor';\n").unwrap();
    fs::write(root.join("resources/views/home.blade.php"), "<p>{{ $user }}</p>\n").unwrap();
    // Many hits in one file to exercise the scrolling list.
    fs::write(root.join("app/Http/Many.php"), "$user;\n".repeat(80)).unwrap();
    let mut app = booted(vec![root.clone()]);

    drop(app.update(Message::OpenPalette(Mode::Search)));
    drop(app.update(Message::PaletteQuery("user".into())));
    finish_search(&mut app, &[root.clone()]);
    let p = app.palette.as_ref().unwrap();
    let files = p.rows.iter().filter(|r| matches!(r, SearchRow::File(..))).count();
    assert_eq!(files, 5, "one header per file: {:?}", &p.rows[..4]);
    assert!(matches!(p.rows[0], SearchRow::File(0, n) if n > 0));
    assert_eq!(p.rows.len(), p.items.len() + files);
    assert_eq!(p.selected_row(), Some(1), "first hit sits under the first header");
    shot(&app, "27-find-in-files");

    // Moving past the visible rows scrolls the list (the header stays in view for the first hit).
    drop(app.update(Message::SearchScrolled { y: 0.0, height: 240.0 }));
    for _ in 0..30 {
        drop(app.update(Message::PaletteMove(1)));
    }
    let p = app.palette.as_ref().unwrap();
    let row_y = p.selected_row().unwrap() as f32 * search_panel::ROW_H;
    assert!(p.scroll_y > 0.0 && row_y >= p.scroll_y && row_y + search_panel::ROW_H <= p.scroll_y + 240.0 + 0.5, "{} {}", p.scroll_y, row_y);

    // The filter narrows to blade templates / drops vendor.
    drop(app.update(Message::SearchFilter("*.blade.php".into())));
    finish_search(&mut app, &[root.clone()]);
    assert_eq!(app.palette.as_ref().unwrap().items.len(), 1);
    drop(app.update(Message::SearchFilter("*.php, !vendor".into())));
    finish_search(&mut app, &[root.clone()]);
    let p = app.palette.as_ref().unwrap();
    assert!(p.items.iter().all(|i| !i.detail.contains("vendor")) && p.items.len() > 80, "{}", p.items.len());
    assert_eq!(app.search_filter, "*.php, !vendor", "remembered for the next time");

    // Find in Folder: only below that folder, labels relative to it.
    drop(app.update(Message::SearchFilter(String::new())));
    drop(app.update(Message::FindInFolder(root.join("app/Models"))));
    finish_search(&mut app, &[root.clone()]);
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.scope.as_deref(), Some(root.join("app/Models").as_path()));
    assert_eq!(p.items.len(), 2, "`class User` and `$user`");
    assert!(p.items[0].detail.starts_with("User.php:2") && p.items[1].detail.starts_with("User.php:3"), "{}", p.items[0].detail);
    shot(&app, "28-find-in-folder");
    drop(app.update(Message::SearchScope(None)));
    assert!(app.palette.as_ref().unwrap().scope.is_none());
}

#[test]
fn project_search_lists_hits_and_opens_the_file_at_the_match() {
    let root = project().join("psearch");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), "first line\nsecond needle line\nthird\n").unwrap();
    fs::write(root.join("sub/b.rs"), "fn main() {\n    let x = \"NEEDLE\";\n}\n").unwrap();
    let mut app = booted(vec![root.clone()]);

    drop(app.update(Message::OpenPalette(Mode::Search)));
    drop(app.update(Message::PaletteQuery("needle".into())));
    let seq = app.palette_seq;
    let re = app.palette.as_ref().unwrap().search_query().compile().unwrap().unwrap();
    let outcome = project_search::run(&[root.clone()], "", &re, || false);
    drop(app.update(Message::SearchResults { seq, outcome: Arc::new(outcome) }));
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.items.len(), 2, "case-insensitive by default");
    assert_eq!(p.status, "2 results in 2 files");
    assert!(p.items.iter().any(|i| i.detail == "sub/b.rs:2" && i.title == "let x = \"NEEDLE\";"));
    shot(&app, "23-project-search");

    // A late answer for an older query is ignored.
    drop(app.update(Message::SearchResults { seq: seq - 1, outcome: Arc::new(Default::default()) }));
    assert_eq!(app.palette.as_ref().unwrap().items.len(), 2);

    // Options re-run the query: case-sensitive keeps only the lower-case hit.
    drop(app.update(Message::PaletteToggle(FindFlag::Case)));
    let seq = app.palette_seq;
    let re = app.palette.as_ref().unwrap().search_query().compile().unwrap().unwrap();
    drop(app.update(Message::SearchResults { seq, outcome: Arc::new(project_search::run(&[root.clone()], "", &re, || false)) }));
    assert_eq!(app.palette.as_ref().unwrap().items.len(), 1);

    // Enter opens the file with the match selected.
    drop(app.update(Message::PaletteSubmit));
    assert!(app.palette.is_none());
    assert!(app.goto.is_some(), "waiting for the file to load");
    open(&mut app, &root.join("a.txt"));
    assert!(app.goto.is_none());
    let Doc::Text(d) = &app.tabs.active().unwrap().doc else { panic!() };
    assert_eq!(d.state.selected_text().as_deref(), Some("needle"));
    assert_eq!(d.state.caret_line_col().0, 1);

    // Hits in an already open tab just activate it and select.
    drop(app.update(Message::OpenAt { path: root.join("a.txt"), line: 0, col: 6, len: 4 }));
    let Doc::Text(d) = &app.tabs.active().unwrap().doc else { panic!() };
    assert_eq!(d.state.selected_text().as_deref(), Some("line"));
}

#[test]
fn go_to_line_jumps_and_empty_or_invalid_input_offers_nothing() {
    let root = project();
    let file = root.join("goto.txt");
    fs::write(&file, (1..=50).map(|i| format!("row {i}\n")).collect::<String>()).unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    drop(app.update(Message::OpenGoto));
    assert_eq!(app.palette.as_ref().unwrap().query, ":");
    assert!(app.palette.as_ref().unwrap().items.is_empty());
    drop(app.update(Message::PaletteQuery(":abc".into())));
    assert!(app.palette.as_ref().unwrap().items.is_empty());
    drop(app.update(Message::PaletteQuery(":37".into())));
    assert_eq!(app.palette.as_ref().unwrap().items[0].title, "Go to line 37");
    drop(app.update(Message::PaletteSubmit));
    let Doc::Text(d) = &app.tabs.active().unwrap().doc else { panic!() };
    assert_eq!(d.state.caret_line_col(), (36, 0));
    // Beyond the end: clamped to the last line.
    drop(app.update(Message::GotoLine(9999)));
    let Doc::Text(d) = &app.tabs.active().unwrap().doc else { panic!() };
    assert_eq!(d.state.caret_line_col().0, 50);
}

#[test]
fn settings_panel_changes_apply_immediately_and_block_the_editor() {
    let root = project();
    let mut app = booted(vec![]);
    open(&mut app, &root.join("hello.rs"));

    drop(app.update(Message::OpenSettingsPanel));
    assert!(app.settings_open);
    shot(&app, "24-settings-panel");

    drop(app.update(Message::ChangeFontSize(2)));
    drop(app.update(Message::ChangeTabSize(-2)));
    drop(app.update(Message::ToggleSetting(Toggle::Minimap)));
    drop(app.update(Message::ToggleSetting(Toggle::WordWrap)));
    drop(app.update(Message::SetTheme("light".into())));
    assert_eq!(app.settings.font_size(), 16.0);
    assert_eq!(app.settings.tab_size(), 2);
    assert!(!app.settings.minimap && app.settings.word_wrap);
    assert_eq!(app.settings.colors().name, "light");
    for _ in 0..30 {
        drop(app.update(Message::ChangeTabSize(-1)));
    }
    assert_eq!(app.settings.tab_size(), 1, "clamped");

    // The editor ignores typing while the panel is open ...
    let acts = editor_actions(&app, |ui| {
        ui.typewrite("zzz");
    });
    assert!(acts.is_empty(), "{acts:?}");
    // ... and Esc closes it.
    drop(app.update(Message::Escape));
    assert!(!app.settings_open);
}

#[test]
fn word_wrap_draws_rows_maps_clicks_and_follows_the_setting() {
    let root = project();
    let file = root.join("wrapme.txt");
    let para = "Merhaba dunya, bu cok uzun bir satir ve pencere genisligine gore alt satirlara bolunmeli. ".repeat(6);
    fs::write(&file, format!("{para}\nkisa satir\n")).unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();
    let state = |app: &App| match &app.tabs.get(id).unwrap().doc {
        Doc::Text(d) => (d.state.total_rows(), d.state.wrapping(), d.state.metrics()),
        _ => panic!(),
    };

    drop(editor_actions(&app, |_| {}));
    assert_eq!(state(&app).0, 3, "no wrapping by default: one row per line (+ the empty last line)");

    app.settings.word_wrap = true;
    drop(editor_actions(&app, |_| {}));
    let (rows, wrapping, m) = state(&app);
    assert!(wrapping && rows > 6, "the long paragraph now spans several rows: {rows}");
    shot(&app, "26-word-wrap");

    // Clicking in the second visual row lands inside the paragraph, not on its first row.
    let (ox, oy) = (sidebar::DEFAULT_WIDTH + 5.0, TOP);
    let at = |col: f32, row: f32| iced::Point::new(ox + (6.2 + col) * m.char_w + 6.0, oy + (row + 0.5) * m.line_h);
    let acts = editor_actions(&app, |ui| {
        ui.point_at(at(3.0, 1.0));
        ui.simulate(iced_test::simulator::click());
    });
    let Some(Action::Click { line: 0, col, .. }) = acts.first().cloned() else { panic!("{acts:?}") };
    assert!(col > 40, "column is measured from the line start, not the row: {col}");
    apply_all(&mut app, id, acts);
    let Doc::Text(d) = &app.tabs.get(id).unwrap().doc else { panic!() };
    assert_eq!(d.state.row_of(d.state.selection().head), (1, 3));

    // Down moves one visual row; typing near the edge re-wraps the paragraph.
    apply_all(&mut app, id, vec![Action::Move { motion: Motion::Down, extend: false }]);
    let Doc::Text(d) = &app.tabs.get(id).unwrap().doc else { panic!() };
    assert_eq!(d.state.row_of(d.state.selection().head).0, 2);
    let before = state(&app).0;
    apply_all(&mut app, id, vec![Action::Insert("Z".repeat(80))]);
    assert!(state(&app).0 > before, "more text -> more rows");

    // Turning the setting off restores one row per line.
    app.settings.word_wrap = false;
    drop(editor_actions(&app, |_| {}));
    assert!(!state(&app).1);
}

#[test]
fn huge_markdown_files_get_a_following_preview_instead_of_none() {
    let root = scratch().join("bigmd");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let file = root.join("BIG.md");
    let body: String = (0..30_000)
        .map(|i| format!("## Section {i}\n\nText of section {i} with `code` and a [link](https://example.org/{i}).\n\n- one\n- two\n\n"))
        .collect();
    assert!(body.len() > 2_000_000);
    fs::write(&file, &body).unwrap();
    let mut app = booted(vec![]);
    let t = std::time::Instant::now();
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();
    let opened = t.elapsed();
    drop(editor_actions(&app, |_| {})); // layout: metrics
    let top = |app: &App| match &app.tabs.get(id).unwrap().doc {
        Doc::Text(d) => (d.state.line_of_byte(d.state.top_byte()) + 1, d.preview.as_ref().and_then(|p| p.window), d.preview.as_ref().map_or(0, |p| p.weight())),
        _ => panic!(),
    };
    let (line, window, items) = top(&app);
    let w = window.expect("a 2 MB document is shown in a window");
    assert!(w.first_line <= line && line <= w.last_line && items < 2500, "{w:?} {items}");
    assert!(opened < std::time::Duration::from_secs(3), "opening + first preview took {opened:?}");

    // Scrolling the editor far down moves the window along with it, quickly.
    let t = std::time::Instant::now();
    apply_all(&mut app, id, vec![Action::ScrollTo { y: 1.0e9 }]);
    let moved = t.elapsed();
    let (line, window, _) = top(&app);
    let w2 = window.unwrap();
    assert!(w2.first_line > w.last_line && w2.first_line <= line && line <= w2.last_line, "{w2:?} top {line}");
    assert!(moved < std::time::Duration::from_millis(1500), "re-windowing took {moved:?}");
    shot(&app, "43-markdown-large-window");
    // Typing keeps the preview in step with the text.
    apply_all(&mut app, id, vec![Action::Insert("x".into())]);
    let Doc::Text(d) = &app.tabs.get(id).unwrap().doc else { panic!() };
    assert_eq!(d.preview.as_ref().unwrap().version, d.version());
}

/// Renders any file through the real view for eyeballing:
/// `VELOEDIT_PREVIEW_FILE=/path/x.md cargo test --release render_file_for_eyeballing -- --ignored`
#[test]
#[ignore = "manual: needs VELOEDIT_PREVIEW_FILE"]
fn render_file_for_eyeballing() {
    let Some(path) = std::env::var_os("VELOEDIT_PREVIEW_FILE").map(PathBuf::from) else { return };
    let mut app = booted(vec![]);
    let t = std::time::Instant::now();
    open(&mut app, &path);
    let opened = t.elapsed();
    let id = app.tabs.active_id().unwrap();
    drop(editor_actions(&app, |_| {}));
    let skip: usize = std::env::var("VELOEDIT_SCROLL_ROWS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    if skip > 0 {
        let lh = match &app.tabs.get(id).unwrap().doc { Doc::Text(d) => d.state.metrics().line_h, _ => 20.0 };
        apply_all(&mut app, id, vec![Action::ScrollTo { y: skip as f32 * lh }]);
    }
    let t = std::time::Instant::now();
    let mut ui = iced_test::simulator(app.view());
    let laid_out = t.elapsed();
    let snap = ui.snapshot(&app.theme()).expect("renders");
    for e in fs::read_dir(out_dir()).unwrap().flatten() {
        if e.file_name().to_string_lossy().starts_with("eyeball") {
            fs::remove_file(e.path()).unwrap();
        }
    }
    let _ = snap.matches_image(out_dir().join("eyeball.png"));
    eprintln!("opened in {opened:?}, view+layout {laid_out:?}");
}

#[test]
#[ignore = "manual: needs VELOEDIT_PREVIEW_FILE"]
fn render_preview_only_at_width() {
    let Some(path) = std::env::var_os("VELOEDIT_PREVIEW_FILE").map(PathBuf::from) else { return };
    let width: f32 = std::env::var("VELOEDIT_PREVIEW_WIDTH").ok().and_then(|v| v.parse().ok()).unwrap_or(380.0);
    let mut app = booted(vec![]);
    open(&mut app, &path);
    let Doc::Text(d) = &app.tabs.active().unwrap().doc else { panic!() };
    let el: Element<'_, Message> = iced::widget::container(crate::ui::markdown_view::view(&crate::ui::theme::DARK, 14.0, d.preview.as_ref(), width))
        .width(width)
        .into();
    let mut ui = iced_test::simulator(el);
    let snap = ui.snapshot(&app.theme()).expect("renders");
    for e in fs::read_dir(out_dir()).unwrap().flatten() {
        if e.file_name().to_string_lossy().starts_with("eyeball") {
            fs::remove_file(e.path()).unwrap();
        }
    }
    let _ = snap.matches_image(out_dir().join("eyeball.png"));
}

#[test]
fn trim_on_save_is_a_setting_and_cuts_the_buffer_before_it_is_written() {
    let root = project();
    let file = root.join("trim-me.txt");
    let messy = "keep  inner\t\n   \nend  ";
    fs::write(&file, messy).unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();
    let state = |app: &App| match &app.tabs.get(id).unwrap().doc {
        Doc::Text(d) => (d.text(), d.dirty()),
        _ => panic!(),
    };
    // What the write task does, run inline.
    let write = |app: &mut App, text: String, version: u64| {
        session::atomic_write(&file, text.as_bytes()).unwrap();
        let result = Ok(crate::editor::text::hash_bytes(text.as_bytes()));
        drop(app.update(Message::Saved { id, path: file.clone(), version, then_close: false, result }));
    };

    // Off (the default): the file is written exactly as it is.
    assert!(!app.settings.trim_trailing_whitespace);
    drop(app.update(Message::Editor(id, Action::Insert("x".into()))));
    let (text, version) = app.text_to_save(id, &file).unwrap();
    assert_eq!(text, format!("x{messy}"));
    write(&mut app, text, version);
    assert_eq!(fs::read_to_string(&file).unwrap(), format!("x{messy}"));
    assert_eq!(app.notice.as_deref(), Some("Saved trim-me.txt"));

    // On: Ctrl+S cuts the blanks off the buffer, then writes it.
    drop(app.update(Message::ToggleSetting(Toggle::TrimOnSave)));
    assert!(app.settings.trim_trailing_whitespace);
    drop(app.update(Message::Editor(id, Action::Insert("y".into()))));
    drop(app.update(Message::SaveActive));
    assert_eq!(state(&app).0, "xykeep  inner\n\nend", "the buffer itself is trimmed");
    let (text, version) = app.text_to_save(id, &file).unwrap();
    assert_eq!(text, "xykeep  inner\n\nend");
    write(&mut app, text, version);
    assert_eq!(fs::read_to_string(&file).unwrap(), "xykeep  inner\n\nend");
    assert!(!state(&app).1, "saved, not dirty");
    assert!(app.notice.as_deref().is_some_and(|n| n.contains("trimmed 3 lines")), "{:?}", app.notice);

    // Saving again changes nothing and says nothing about trimming.
    let (text, version) = app.text_to_save(id, &file).unwrap();
    write(&mut app, text, version);
    assert_eq!(app.notice.as_deref(), Some("Saved trim-me.txt"));

    // One undo brings the blanks back (and the tab is unsaved again).
    drop(app.update(Message::Editor(id, Action::Undo)));
    assert_eq!(state(&app), (format!("xykeep  inner\t\n   \nend  "), true));

    // Off again: files stay as they are.
    drop(app.update(Message::ToggleSetting(Toggle::TrimOnSave)));
    assert!(!app.settings.trim_trailing_whitespace);
    assert_eq!(app.text_to_save(id, &file).unwrap().0, "xykeep  inner\t\n   \nend  ");
}

#[test]
fn trim_on_save_is_offered_in_the_settings_panel_the_menu_and_the_palette() {
    let root = project();
    let mut app = booted(vec![]);
    open(&mut app, &root.join("hello.rs"));
    drop(app.update(Message::OpenSettingsPanel));
    shot(&app, "24b-settings-trim");
    drop(app.update(Message::Escape));

    // The Preferences menu shows it with a check mark, and picking it flips the setting.
    let entry = app.menu_entries(4).into_iter().find_map(|e| match e {
        crate::ui::menu::Entry::Item { label, msg, checked, .. } if label == "Trim Trailing Spaces on Save" => Some((msg, checked)),
        _ => None,
    });
    let (msg, checked) = entry.expect("the Preferences menu has the toggle");
    assert_eq!(checked, Some(false));
    drop(app.update(msg));
    assert!(app.settings.trim_trailing_whitespace);
    // ... and so does the command palette.
    assert!(Command::all().contains(&Command::ToggleTrimOnSave));
    drop(app.update(command_message(Command::ToggleTrimOnSave)));
    assert!(!app.settings.trim_trailing_whitespace);
}

#[test]
fn trim_on_save_keeps_the_hard_line_breaks_of_markdown_files() {
    let root = project();
    let notes = root.join("notes.md");
    let mut app = booted(vec![]);
    drop(app.update(Message::ToggleSetting(Toggle::TrimOnSave)));
    assert!(app.settings.trim_trailing_whitespace);
    let text = "first line  \nsecond line\t\n\nend of paragraph  \n  \nlast  ";

    // An existing .md file.
    fs::write(&notes, text).unwrap();
    open(&mut app, &notes);
    let id = app.tabs.active_id().unwrap();
    let (saved, _) = app.text_to_save(id, &notes).unwrap();
    assert_eq!(saved, "first line  \nsecond line\n\nend of paragraph\n\nlast", "the break after \"first line\" stays");

    // A new buffer is judged by the name it is saved under.
    drop(app.update(Message::NewFile));
    let id = app.tabs.active_id().unwrap();
    drop(app.update(Message::Editor(id, Action::Insert("one  \ntwo  \n".into()))));
    let (as_text, _) = app.text_to_save(id, &root.join("fresh.txt")).unwrap();
    assert_eq!(as_text, "one\ntwo\n", "a text file loses them");
    drop(app.update(Message::Editor(id, Action::Undo)));
    let (as_md, _) = app.text_to_save(id, &root.join("fresh.md")).unwrap();
    assert_eq!(as_md, "one  \ntwo\n", "a Markdown file keeps the break before more text");
}

#[test]
fn a_reload_from_disk_can_be_undone_and_a_file_that_reads_empty_is_not_taken_over() {
    let root = project();
    let file = root.join("flaky.txt");
    fs::write(&file, "line one\nline two\n").unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();
    let text = |app: &App| match &app.tabs.get(id).unwrap().doc {
        Doc::Text(d) => d.text(),
        _ => panic!(),
    };

    // Someone changes the file: the clean buffer follows - as an edit Ctrl+Z can take back.
    fs::write(&file, "line one\nline TWO\nline three\n").unwrap();
    disk_event(&mut app, id, &file);
    assert_eq!(text(&app), "line one\nline TWO\nline three\n");
    assert!(!app.tabs.get(id).unwrap().is_dirty());
    assert!(app.notice.as_deref().unwrap().contains("Ctrl+Z"), "{:?}", app.notice);
    drop(app.update(Message::Editor(id, Action::Undo)));
    assert_eq!(text(&app), "line one\nline two\n", "the reload is undone");
    assert!(app.tabs.get(id).unwrap().is_dirty(), "and the buffer differs from the disk again");
    drop(app.update(Message::Editor(id, Action::Redo)));
    assert_eq!(text(&app), "line one\nline TWO\nline three\n");

    // The file suddenly reads as empty (a save cut short, a mount that lost its connection):
    // the text in the buffer may be the only copy, so it is kept and offered for saving.
    fs::write(&file, "").unwrap();
    disk_event(&mut app, id, &file);
    assert_eq!(text(&app), "line one\nline TWO\nline three\n", "the buffer is not emptied");
    assert!(app.tabs.get(id).unwrap().is_dirty() && app.tabs.get(id).unwrap().has_conflict());
    assert!(app.notice.as_deref().unwrap().contains("empty on disk"), "{:?}", app.notice);
    // Ctrl+S asks first (the disk changed), and Overwrite writes the text back.
    drop(app.update(Message::SaveActive));
    assert_eq!(app.dialog, Some(Dialog::Conflict { id, then_close: false }));
    drop(app.update(Message::ConflictChoice(id, false, ConflictChoice::Overwrite)));
    assert!(app.dialog.is_none());
    let (restored, version) = app.text_to_save(id, &file).unwrap();
    assert_eq!(restored, "line one\nline TWO\nline three\n");
    session::atomic_write(&file, restored.as_bytes()).unwrap();
    let result = Ok(crate::editor::text::hash_bytes(restored.as_bytes()));
    drop(app.update(Message::Saved { id, path: file.clone(), version, then_close: false, result }));
    assert!(!app.tabs.get(id).unwrap().is_dirty() && !app.tabs.get(id).unwrap().has_conflict());

    // "Reload from Disk" is the user's own wish: an empty file is taken - and still undoable.
    fs::write(&file, "").unwrap();
    let result = block_on(loader::reload(file.clone())).map(Handoff::new);
    drop(app.update(Message::DiskChanged { id, force: true, result }));
    assert_eq!(text(&app), "");
    drop(app.update(Message::Editor(id, Action::Undo)));
    assert_eq!(text(&app), "line one\nline TWO\nline three\n");
}

#[test]
fn the_echo_of_our_own_save_is_not_read_back_from_the_disk() {
    let root = project();
    let file = root.join("echo.txt");
    fs::write(&file, "x\n").unwrap();
    let mut app = booted(vec![]);
    open(&mut app, &file);
    let id = app.tabs.active_id().unwrap();
    let just_saved = |app: &App| match &app.tabs.get(id).unwrap().doc {
        Doc::Text(d) => d.just_saved(),
        _ => panic!(),
    };
    assert!(!just_saved(&app));
    drop(app.update(Message::Editor(id, Action::Insert("y".into()))));
    drop(app.update(Message::SaveActive));
    assert!(just_saved(&app), "while the write is under way the watcher's events are our own");
    let (text, version) = app.text_to_save(id, &file).unwrap();
    session::atomic_write(&file, text.as_bytes()).unwrap();
    let result = Ok(crate::editor::text::hash_bytes(text.as_bytes()));
    drop(app.update(Message::Saved { id, path: file.clone(), version, then_close: false, result }));
    assert!(just_saved(&app), "and for a moment afterwards");
    // Later changes are real changes again.
    if let Doc::Text(d) = &mut app.tabs.get_mut(id).unwrap().doc {
        d.saved_at = Some(Instant::now() - Duration::from_secs(10));
    }
    assert!(!just_saved(&app));
}

#[test]
fn the_window_title_shows_the_whole_path_of_a_saved_file() {
    let root = project();
    let file = root.join("sub/inner.md");
    let mut app = booted(vec![]);
    assert_eq!(app.title(), "VeloEdit");
    open(&mut app, &file);
    assert_eq!(app.title(), format!("{} - VeloEdit", file.display()));
    let id = app.tabs.active_id().unwrap();
    drop(app.update(Message::Editor(id, Action::Insert("x".into()))));
    assert_eq!(app.title(), format!("\u{2022} {} - VeloEdit", file.display()), "unsaved: marked");
    // A buffer without a file keeps being named after its text.
    drop(app.update(Message::NewFile));
    let id = app.tabs.active_id().unwrap();
    drop(app.update(Message::Editor(id, Action::Insert("scratch notes".into()))));
    assert!(app.title().contains("scratch notes") && !app.title().contains('/'), "{}", app.title());
}

// A FIFO (`mkfifo`) is what stands in for the dead mount: Unix only.
#[cfg(unix)]
#[test]
fn a_draft_comes_back_even_when_its_file_sits_on_a_mount_that_never_answers() {
    // A FIFO stands in for the dead mount: opening it blocks until somebody writes to it.
    let _turn = session_lock();
    let dir = scratch().join("deadmount");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("state-drafts")).unwrap();
    let fifo = dir.join("remote.txt");
    assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let drafts = session::state_dir().join("drafts");
    fs::create_dir_all(&drafts).unwrap();
    fs::write(drafts.join("draft-4242.txt"), "my unsaved work").unwrap();

    let ok = dir.join("fine.txt");
    fs::write(&ok, "reachable").unwrap();
    let rec = |path: &Path, draft: Option<&str>| session::TabRecord {
        kind: session::TabKind::Text,
        path: Some(path.to_path_buf()),
        draft: draft.map(str::to_owned),
        hex_edits: Vec::new(),
    };
    let records = vec![rec(&fifo, Some("draft-4242.txt")), rec(&fifo, None), rec(&ok, None)];
    let started = Instant::now();
    let (loaded, active) = block_on(loader::restore_within(records, Some(2), Duration::from_millis(300)));
    assert!(started.elapsed() < Duration::from_secs(5), "restoring did not wait for the dead mount: {:?}", started.elapsed());
    assert_eq!(loaded.len(), 2, "the draft and the reachable file; the unreachable file without work is left out");
    assert!(loaded[0].dirty && matches!(&loaded[0].body, Body::Text(t) if t == "my unsaved work"), "the unsaved text survives");
    assert!(matches!(&loaded[1].body, Body::Text(t) if t == "reachable"));
    assert_eq!(active, Some(1), "the active tab index follows");
    // Let the stuck reader threads go (a read+write open never blocks; while it is held the
    // blocked opens return, and dropping it ends their reads).
    for _ in 0..6 {
        let held = fs::OpenOptions::new().read(true).write(true).open(&fifo);
        std::thread::sleep(Duration::from_millis(100));
        drop(held);
    }
    fs::remove_file(drafts.join("draft-4242.txt")).ok();
}
