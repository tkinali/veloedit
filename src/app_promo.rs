//! The frames of the promo video: a scripted tour of the real UI, drawn headlessly.
//!
//! Not part of the normal test run. Generate the frames with
//!
//! ```text
//! ICED_TEST_BACKEND=tiny-skia cargo test --release promo_frames -- --ignored --nocapture
//! ```
//!
//! (they land in `target/promo/frames`, `VELOEDIT_PROMO_DIR` moves them) and let
//! `scripts/make-promo.py` compose the video: backdrop, captions, key hints, the mouse
//! pointer, the title and closing cards. The script here only drives the app (typing, menus,
//! palette) and records, per scene, which frame is shown for how long.

use std::{
    fs,
    path::{Path, PathBuf},
};

use iced::{Point, Size};
use serde_json::json;

use iced::futures::executor::block_on;

use super::{
    tests::{booted, deliver_find_results, deliver_git_status, finish_search, open, scan_all, scratch, session_lock, type_query},
    *,
};

/// The window of the video, in logical pixels (drawn at twice that).
const W: f32 = 1000.0;
const H: f32 = 740.0;

struct Scene {
    name: String,
    en: String,
    tr: String,
    /// `(file, milliseconds)`: identical consecutive frames are one entry.
    frames: Vec<(String, u32)>,
    last: Vec<u8>,
    pointer: Vec<serde_json::Value>,
    clicks: Vec<u32>,
    keys: Vec<serde_json::Value>,
    /// Moments the window is closed and reopened (the video darkens it meanwhile).
    cuts: Vec<serde_json::Value>,
    /// Scene time so far.
    t: u32,
}

struct Reel {
    app: App,
    dir: PathBuf,
    root: PathBuf,
    done: Vec<serde_json::Value>,
    scene: Option<Scene>,
    seq: usize,
}

impl Reel {
    fn new(app: App, dir: PathBuf, root: PathBuf) -> Self {
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self { app, dir, root, done: Vec::new(), scene: None, seq: 0 }
    }

    /// Closes every tab and every popup: each scene starts from a tidy stage.
    fn tidy(&mut self) {
        let ids: Vec<TabId> = self.app.tabs.iter().map(|t| t.id).collect();
        for id in ids {
            self.app.tabs.close(id);
        }
        self.app.palette = None;
        self.app.popup = None;
        self.app.dialog = None;
        self.app.settings_open = false;
        self.app.notice = None;
        self.app.find.open = false;
    }

    fn begin(&mut self, name: &str, en: &str, tr: &str) {
        self.end();
        self.tidy();
        fs::create_dir_all(self.dir.join(name)).unwrap();
        self.scene = Some(Scene {
            name: name.into(),
            en: en.into(),
            tr: tr.into(),
            frames: Vec::new(),
            last: Vec::new(),
            pointer: Vec::new(),
            clicks: Vec::new(),
            keys: Vec::new(),
            cuts: Vec::new(),
            t: 0,
        });
    }

    fn end(&mut self) {
        if let Some(s) = self.scene.take() {
            self.done.push(json!({
                "name": s.name, "en": s.en, "tr": s.tr, "frames": s.frames, "pointer": s.pointer,
                "clicks": s.clicks, "keys": s.keys, "cuts": s.cuts, "length": s.t,
            }));
        }
    }

    fn finish(mut self) {
        self.end();
        let manifest = json!({ "window": [W, H], "scenes": self.done });
        fs::write(self.dir.join("manifest.json"), serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
        println!("promo: {} scenes, {} distinct frames -> {}", self.done.len(), self.seq, self.dir.display());
    }

    /// Draws the current view and holds it for `ms`. A view identical to the last one only
    /// makes that frame last longer.
    fn snap(&mut self, ms: u32) {
        let theme = self.app.theme();
        let mut ui = iced_test::simulator::Simulator::with_size(iced::Settings::default(), Size::new(W, H), self.app.view());
        let snapshot = ui.snapshot(&theme).expect("the view renders");
        let tmp = self.dir.join("tmp.png");
        for e in fs::read_dir(&self.dir).unwrap().flatten() {
            if e.file_name().to_string_lossy().starts_with("tmp-") {
                fs::remove_file(e.path()).unwrap();
            }
        }
        snapshot.matches_image(&tmp).expect("the frame is written");
        let written = fs::read_dir(&self.dir)
            .unwrap()
            .flatten()
            .find(|e| e.file_name().to_string_lossy().starts_with("tmp-"))
            .expect("a frame was written")
            .path();
        let bytes = fs::read(&written).unwrap();
        let scene = self.scene.as_mut().expect("inside a scene");
        if bytes == scene.last && !scene.frames.is_empty() {
            scene.frames.last_mut().unwrap().1 += ms;
            fs::remove_file(written).unwrap();
        } else {
            self.seq += 1;
            let file = format!("{}/{:05}.png", scene.name, self.seq);
            fs::rename(written, self.dir.join(&file)).unwrap();
            scene.frames.push((file, ms));
            scene.last = bytes;
        }
        scene.t += ms;
    }

    fn msg(&mut self, m: Message) {
        drop(self.app.update(m));
    }

    /// Centre of the first widget that shows `text`, in window coordinates.
    fn at(&self, text: &str) -> Point {
        let mut ui = iced_test::simulator::Simulator::with_size(iced::Settings::default(), Size::new(W, H), self.app.view());
        match ui.find(text) {
            Ok(target) => target.bounds().center(),
            Err(e) => panic!("no widget shows `{text}`: {e:?}"),
        }
    }

    /// The mouse pointer glides to `(x, y)` in `ms` (the view stays as it is).
    fn glide(&mut self, to: Point, ms: u32) {
        let scene = self.scene.as_mut().unwrap();
        scene.pointer.push(json!({ "t0": scene.t, "t1": scene.t + ms, "x": to.x, "y": to.y }));
        self.snap(ms);
    }

    fn point_at(&mut self, text: &str, ms: u32) {
        let to = self.at(text);
        self.glide(to, ms);
    }

    fn click(&mut self) {
        let scene = self.scene.as_mut().unwrap();
        scene.clicks.push(scene.t);
    }

    fn keys(&mut self, label: &str, ms: u32) {
        let scene = self.scene.as_mut().unwrap();
        scene.keys.push(json!({ "t": scene.t, "label": label, "dur": ms }));
    }

    /// Quits and starts the editor again: the session is written exactly like a real quit does,
    /// and a fresh app restores it (open folder, unsaved tabs). The video darkens the window for
    /// `ms` meanwhile.
    fn restart(&mut self, ms: u32) {
        let scene = self.scene.as_mut().unwrap();
        scene.cuts.push(json!({ "t0": scene.t, "t1": scene.t + ms }));
        block_on(self.app.snapshot().write()).unwrap();
        let session = session::load();
        let (loaded, active) = block_on(loader::restore(session.tabs, session.active));
        let mut fresh = booted(vec![]);
        drop(fresh.update(Message::WindowResized(Size::new(W, H))));
        fresh.sidebar_width = 190.0;
        fresh.settings.font_size = 15.0;
        drop(fresh.update(Message::Restored(Handoff::new(loaded), active)));
        for _ in 0..2 {
            for d in ["", "src", "app", "web", "assets"] {
                scan_all(&mut fresh, &self.root.join(d));
            }
        }
        self.app = fresh;
        self.git_status();
        self.snap(ms);
    }

    /// Delivers what the git workers would answer for the project (the sidebar's colors).
    fn git_status(&mut self) {
        deliver_git_status(&mut self.app, &self.root);
    }

    /// What `Ctrl+S` does, without the worker: the file is written and the app told so.
    fn save(&mut self) {
        let id = self.active();
        let path = self.app.tabs.get(id).and_then(|t| t.path.clone()).expect("a saved file");
        let (text, version) = self.app.text_to_save(id, &path).expect("a text tab");
        fs::write(&path, &text).unwrap();
        let result = Ok(crate::editor::text::hash_bytes(text.as_bytes()));
        self.msg(Message::Saved { id, path, version, then_close: false, result });
    }

    fn active(&self) -> TabId {
        self.app.tabs.active_id().expect("a tab is open")
    }

    /// Types `text` into the active tab, `ms` per character.
    fn type_text(&mut self, text: &str, ms: u32) {
        let id = self.active();
        for ch in text.chars() {
            let action = if ch == '\n' { Action::Enter } else { Action::Insert(ch.to_string()) };
            self.msg(Message::Editor(id, action));
            self.color();
            self.snap(if ch == ' ' { ms + 20 } else { ms });
        }
    }

    /// Runs the highlighting job the app would have started on a worker.
    fn color(&mut self) {
        drop(self.app.start_analysis());
        let id = self.active();
        let Doc::Text(doc) = &self.app.tabs.get(id).unwrap().doc else { return };
        if !doc.hl.in_flight {
            return;
        }
        let (lang, text, prev) = (doc.hl.lang.unwrap(), doc.text(), doc.hl.parse.clone());
        let result = syntax::highlight(lang, prev.as_ref(), Arc::new(text)).map(|(p, s)| (p, Arc::new(s)));
        self.msg(Message::Highlighted { id, lang, result });
    }

    /// Delivers the git base and the change marks of the active file (a worker's job).
    fn git_marks(&mut self) {
        let id = self.active();
        let Some(path) = self.app.tabs.get(id).and_then(|t| t.path.clone()) else { return };
        let base = git::base_text(&path).map(Arc::new);
        self.msg(Message::GitBase { id, path, base: base.clone() });
        let Doc::Text(d) = &self.app.tabs.get(id).unwrap().doc else { return };
        if let Some(base) = base {
            let marks = Arc::new(git::diff(&base, &d.text()));
            self.msg(Message::GitMarksReady { id, marks });
        }
    }

    fn open_file(&mut self, rel: &str) {
        let path = self.root.join(rel);
        open(&mut self.app, &path);
        self.color();
        self.git_marks();
    }

    /// Where `needle` starts in the active text: `(line, column)`, 0-based.
    fn find_pos(&self, needle: &str) -> (usize, usize) {
        let Some(Doc::Text(d)) = self.app.tabs.active().map(|t| &t.doc) else { panic!("a text tab") };
        let text = d.text();
        let at = text.find(needle).unwrap_or_else(|| panic!("`{needle}` is not in the text"));
        let line = text[..at].matches('\n').count();
        let col = text[..at].rsplit('\n').next().unwrap().chars().count();
        (line, col)
    }

    fn click_in_text(&mut self, line: usize, col: usize) {
        let id = self.active();
        self.msg(Message::Editor(id, Action::Click { line, col, extend: false, count: 1, add: false }));
    }
}

// ------------------------------------------------------------------- the demo project

const MAIN_RS: &str = r#"//! velo-demo: a tiny routing table.

use std::collections::HashMap;

type Handler = fn(&Request) -> String;

#[derive(Debug, Clone)]
struct Request {
    method: &'static str,
    path: String,
}

struct Router {
    routes: HashMap<String, Handler>,
}

impl Router {
    fn new() -> Self {
        Router { routes: HashMap::new() }
    }

    // TODO: support path parameters like /users/:id
    fn add(&mut self, path: &str, handler: Handler) {
        self.routes.insert(path.to_owned(), handler);
    }

    fn handle(&self, request: &Request) -> String {
        match self.routes.get(&request.path) {
            Some(handler) => handler(request),
            None => format!("404 {}", request.path),
        }
    }
}

fn home(request: &Request) -> String {
    format!("Hello from {}", request.path)
}

fn health(_request: &Request) -> String {
    "ok".to_owned()
}

fn main() {
    let mut router = Router::new();
    router.add("/", home);
    router.add("/health", health);

    // TODO: read the port from the environment
    let request = Request {
        method: "GET",
        path: "/health".into(),
    };
    println!("{}", router.handle(&request));
}
"#;

/// What git has of `main.rs`: a few lines differ from the working copy (the gutter marks).
const MAIN_RS_COMMITTED: &str = r#"//! velo-demo: a tiny routing table.

use std::collections::HashMap;

type Handler = fn(&Request) -> String;

#[derive(Debug, Clone)]
struct Request {
    method: &'static str,
    path: String,
}

struct Router {
    routes: HashMap<String, Handler>,
}

impl Router {
    fn new() -> Self {
        Router { routes: HashMap::new() }
    }

    fn add(&mut self, path: &str, handler: Handler) {
        self.routes.insert(path.to_owned(), handler);
    }

    fn handle(&self, request: &Request) -> String {
        match self.routes.get(&request.path) {
            Some(handler) => handler(request),
            None => format!("404 {}", request.path),
        }
    }
}

fn home(request: &Request) -> String {
    format!("Hello from {}", request.path)
}

fn health(_request: &Request) -> String {
    "up".to_owned()
}

fn main() {
    let mut router = Router::new();
    router.add("/", home);
    router.add("/health", health);

    let request = Request {
        method: "GET",
        path: "/health".into(),
    };
    println!("{}", router.handle(&request));
}
"#;

const ROUTER_PHP: &str = r#"<?php

namespace App\Http;

final class Router
{
    /** @var array<string, callable> */
    private array $routes = [];

    public function add(string $path, callable $h): void
    {
        $this->routes[$path] = $h;
    }

    public function dispatch(string $path): string
    {
        $handler = $this->routes[$path] ?? null;

        if ($handler === null) {
            return '404 Not Found';
        }

        return (string) $handler($path);
    }
}
"#;

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>velo-demo</title>
  <link rel="stylesheet" href="style.css">
</head>
<body>
  <h1 class="title">Hello from velo-demo</h1>
  <p>Served by a router of <strong>twenty lines</strong>.</p>
  <script src="legacy.js"></script>
</body>
</html>
"#;

const STYLE_CSS: &str = r#":root {
  --accent: #4aa3ff;
}

body {
  font-family: system-ui, sans-serif;
  margin: 2rem auto;
  max-width: 40rem;
}

.title {
  color: var(--accent);
}
"#;

const CONFIG_YAML: &str = "server:\n  host: 0.0.0.0\n  port: 8080\nroutes:\n  - path: /\n    handler: home\n  - path: /health\n    handler: health\n";

/// Indented with tabs, the way old projects are.
const LEGACY_JS: &str = "// TODO: replace this with a bundler\nfunction greet(name) {\n\tvar message = 'Hello, ' + name;\n\tif (name) {\n\t\tconsole.log(message);\n\t} else {\n\t\tconsole.log('Hello, stranger');\n\t}\n\treturn message;\n}\n\ngreet('world');\n";

const README_MD: &str = r#"# velo-demo

A tiny router, written to show what **VeloEdit** can do.

| Path      | Handler  | Answer     |
|-----------|----------|------------|
| `/`       | `home`   | a greeting |
| `/health` | `health` | `ok`       |
| anything  | -        | `404`      |

- [x] routing table
- [ ] path parameters (TODO)
"#;

/// A page as copied from a browser's "view source".
const PAGE_HTML: &str = r#"<!DOCTYPE html>
<html lang="tr">
<head>
  <meta charset="utf-8">
  <title>Merhaba dunya</title>
  <style>
    body { font-family: sans-serif; background: #f4f4f4; }
    .card { padding: 1rem; border-radius: 8px; }
  </style>
</head>
<body>
  <div class="card">
    <h1>Merhaba!</h1>
    <a href="/iletisim">Iletisim</a>
  </div>
</body>
</html>
"#;

fn make_project(root: &Path) {
    let _ = fs::remove_dir_all(root);
    for d in ["src", "app", "web", "assets"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    let write = |rel: &str, text: &str| fs::write(root.join(rel), text).unwrap();
    write("src/main.rs", MAIN_RS_COMMITTED);
    write("app/Router.php", ROUTER_PHP);
    write("web/index.html", INDEX_HTML);
    write("web/style.css", STYLE_CSS);
    write("web/legacy.js", LEGACY_JS);
    write("config.yaml", CONFIG_YAML);
    write("README.md", README_MD);
    let bytes: Vec<u8> = (0u32..768)
        .map(|i| match i % 96 {
            0..=15 => b"VELO\x01\x00\x00\x02velo-demo "[(i % 16) as usize],
            16..=47 => b"The quick brown fox jumps over t"[(i % 32) as usize],
            _ => ((i * 7 + 13) % 251) as u8,
        })
        .collect();
    fs::write(root.join("assets/data.bin"), bytes).unwrap();
    fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/icon-256.png"), root.join("assets/logo.png")).unwrap();

    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .args(["-c", "user.name=demo", "-c", "user.email=demo@example.com", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success());
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-qm", "init"]);
    // The working copy is a little ahead of the commit.
    write("src/main.rs", MAIN_RS);
}

// ------------------------------------------------------------------------ the tour

#[test]
#[ignore = "renders the promo video's frames; see the module documentation"]
fn promo_frames() {
    let _turn = session_lock();
    let root = scratch().join("velo-demo");
    make_project(&root);
    let mut app = booted(vec![root.clone()]);
    drop(app.update(Message::WindowResized(Size::new(W, H))));
    app.sidebar_width = 190.0;
    app.settings.font_size = 15.0;
    scan_all(&mut app, &root);
    let out = std::env::var_os("VELOEDIT_PROMO_DIR").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/promo/frames"));
    let mut r = Reel::new(app, out, root.clone());
    for d in ["src", "app", "web", "assets"] {
        r.msg(Message::TreeClicked { path: root.join(d), is_dir: true });
        scan_all(&mut r.app, &root.join(d));
    }
    r.git_status();

    // 1 -------------------------------------------------------------- open a file
    r.begin(
        "open",
        "Opens in well under 100 ms: 50+ languages, minimap, git marks in the gutter",
        "100 ms'den kısa sürede açılır: 50+ dil, mini harita, satır kenarında git işaretleri",
    );
    r.snap(900);
    r.point_at("main.rs", 700);
    r.click();
    r.open_file("src/main.rs");
    r.snap(2200);
    // The caret walks down a little, the way a reader's would.
    for line in [6usize, 10, 16, 20] {
        r.click_in_text(line, 8);
        r.snap(260);
    }
    r.snap(1200);

    // 1b ------------------------------------------------------------- git colors in the sidebar
    r.begin(
        "git",
        "Git colors: changed files turn yellow, new ones green, in the sidebar",
        "Git renkleri: değişen dosya sarı, yeni dosya yeşil, kenar çubuğunda",
    );
    r.open_file("src/main.rs");
    r.snap(1400);
    // An edit that is saved: the file lights up, and the folder says "something in here changed".
    r.point_at("style.css", 700);
    r.click();
    r.open_file("web/style.css");
    r.snap(600);
    let id = r.active();
    r.msg(Message::Editor(id, Action::Move { motion: crate::editor::state::Motion::DocEnd, extend: false }));
    r.type_text("\n.card { border-radius: 8px; }", 35);
    r.snap(600);
    r.keys("Ctrl+S", 1200);
    r.save();
    r.git_status();
    r.snap(1500);
    // A file that appears from somewhere else (a build, a checkout, a script): new, so green.
    fs::write(r.root.join("app/Cache.php"), "<?php\n\nclass Cache {}\n").unwrap();
    let app_dir = r.root.join("app");
    scan_all(&mut r.app, &app_dir);
    r.git_status();
    r.snap(1800);
    // Back to how the project was, for the scenes that follow.
    fs::write(r.root.join("web/style.css"), STYLE_CSS).unwrap();
    fs::remove_file(r.root.join("app/Cache.php")).unwrap();
    scan_all(&mut r.app, &app_dir);
    scan_all(&mut r.app, &r.root.join("web"));
    r.git_status();

    // 2 -------------------------------------------------------------- quick open
    r.begin("quick-open", "Ctrl+P: fuzzy-open any file in the project", "Ctrl+P: projedeki herhangi bir dosyayı bulanık aramayla aç");
    r.open_file("src/main.rs");
    r.snap(500);
    r.keys("Ctrl+P", 1500);
    r.msg(Message::OpenPalette(Mode::Files));
    let index = Arc::new(palette::index::build(&[root.clone()]));
    r.msg(Message::IndexBuilt(index));
    r.snap(700);
    for q in ["r", "ro", "rou", "rout", "route", "router"] {
        type_query(&mut r.app, q);
        r.snap(230);
    }
    r.snap(900);
    r.msg(Message::Escape);
    r.open_file("app/Router.php");
    r.snap(1900);

    // 3 -------------------------------------------------------------- palette and themes
    r.begin("palette", "Command palette, dark and light themes", "Komut paleti, koyu ve açık tema");
    r.open_file("app/Router.php");
    r.snap(400);
    r.keys("Ctrl+Shift+P", 1500);
    r.msg(Message::OpenPalette(Mode::Commands));
    r.snap(900);
    for q in ["t", "th", "the", "them", "theme", "theme l", "theme li", "theme lig"] {
        r.msg(Message::PaletteQuery(q.into()));
        r.snap(150);
    }
    r.snap(500);
    r.msg(Message::PaletteSubmit);
    r.snap(2000);
    r.keys("Ctrl+Shift+P", 1200);
    r.msg(Message::OpenPalette(Mode::Commands));
    for q in ["d", "da", "dar", "dark"] {
        r.msg(Message::PaletteQuery(q.into()));
        r.snap(150);
    }
    r.snap(300);
    r.msg(Message::PaletteSubmit);
    r.snap(900);

    // 4 -------------------------------------------------------------- multiple cursors
    r.begin("multicursor", "Ctrl+D: edit many places at once", "Ctrl+D: birçok yeri aynı anda düzenle");
    r.open_file("src/main.rs");
    let (line, col) = r.find_pos("request: &Request) -> String {\n        match");
    r.click_in_text(line, col + 2);
    r.snap(1100);
    let id = r.active();
    for _ in 0..5 {
        r.keys("Ctrl+D", 700);
        r.msg(Message::Editor(id, Action::SelectNextOccurrence));
        r.snap(650);
    }
    r.snap(500);
    r.type_text("req", 190);
    r.snap(1700);
    r.msg(Message::Editor(id, Action::Collapse));
    r.snap(1000);

    // 5 -------------------------------------------------------------- find
    r.begin("find", "Find with live highlighting of every match", "Her eşleşmeyi canlı vurgulayarak ara");
    r.open_file("src/main.rs");
    r.snap(500);
    r.keys("Ctrl+F", 1400);
    r.msg(Message::OpenFind { replace: false });
    r.snap(700);
    for q in ["r", "ro", "rou", "rout", "route", "router"] {
        r.msg(Message::FindQuery(q.into()));
        deliver_find_results(&mut r.app);
        r.snap(210);
    }
    r.snap(900);
    for _ in 0..2 {
        r.keys("Enter", 600);
        r.msg(Message::FindNext);
        r.snap(700);
    }
    r.msg(Message::CloseFind);
    r.snap(300);

    // 6 -------------------------------------------------------------- find in files
    r.begin("find-in-files", "Find in files: grouped per file, with filters", "Projede ara: dosyaya göre gruplanmış, filtreli");
    r.open_file("src/main.rs");
    r.snap(400);
    r.keys("Ctrl+Shift+F", 1500);
    r.msg(Message::OpenPalette(Mode::Search));
    r.snap(700);
    for q in ["T", "TO", "TOD", "TODO"] {
        r.msg(Message::PaletteQuery(q.into()));
        r.snap(260);
    }
    finish_search(&mut r.app, &[root.clone()]);
    r.snap(1500);
    for _ in 0..2 {
        r.msg(Message::PaletteMove(1));
        r.snap(600);
    }
    r.snap(900);
    r.msg(Message::Escape);

    // 7 -------------------------------------------------------------- split view
    r.begin("split", "Split view: one file, two places, shared undo", "Bölünmüş görünüm: tek dosya, iki yer, ortak geri alma");
    r.open_file("src/main.rs");
    r.snap(500);
    // Room for two panes first: the sidebar steps aside, long lines wrap.
    r.keys("Ctrl+B", 1200);
    r.msg(Message::ToggleSidebar);
    r.app.settings.word_wrap = true;
    r.snap(1100);
    r.point_at("View", 700);
    r.click();
    r.msg(Message::OpenMenu(2));
    r.snap(700);
    r.point_at("Split Right", 600);
    r.click();
    r.msg(Message::MenuPick(Box::new(Message::SplitActive(SplitDir::Right))));
    r.snap(1300);
    let id = r.active();
    for _ in 0..8 {
        r.msg(Message::EditorPane(id, Pane::Second, Action::Scroll { dx: 0.0, dy: 110.0 }));
        r.snap(90);
    }
    r.snap(1500);
    r.msg(Message::CloseSplit(id));
    r.msg(Message::ToggleSidebar);
    r.app.settings.word_wrap = false;

    // 8 -------------------------------------------------------------- markdown
    r.begin("markdown", "Live Markdown preview, with real tables", "Canlı Markdown önizlemesi, gerçek tablolarla");
    r.open_file("README.md");
    r.msg(Message::ToggleSidebar);
    r.snap(2800);
    let id = r.active();
    r.msg(Message::Editor(id, Action::Move { motion: crate::editor::state::Motion::DocEnd, extend: false }));
    r.type_text("\n- [x] a live preview", 80);
    r.snap(1800);
    r.msg(Message::ToggleSidebar);

    // 9 -------------------------------------------------------------- folding
    r.begin("folding", "Code folding for every language", "Her dil için kod katlama");
    r.open_file("src/main.rs");
    r.snap(700);
    r.keys("Fold All", 1200);
    r.msg(Message::EditAction(Action::FoldAll));
    r.snap(1500);
    r.keys("Fold Level 2", 1200);
    r.msg(Message::EditAction(Action::UnfoldAll));
    r.msg(Message::EditAction(Action::FoldLevel(2)));
    r.snap(1800);
    r.msg(Message::EditAction(Action::UnfoldAll));
    r.snap(500);

    // 10 ------------------------------------------------------------- syntax
    r.begin("syntax", "Paste a page and it is recognized, or pick any syntax", "Bir sayfa yapıştır, tanınır; ya da istediğin dili seç");
    r.msg(Message::NewFile);
    r.snap(600);
    r.keys("Ctrl+V", 1300);
    let id = r.active();
    r.msg(Message::Editor(id, Action::Insert(PAGE_HTML.into())));
    r.snap(400);
    r.color();
    r.snap(1500);
    r.point_at("HTML", 800);
    r.click();
    r.msg(Message::OpenSyntaxPicker);
    r.snap(1000);
    for q in ["Syntax: p", "Syntax: pl", "Syntax: pla", "Syntax: plai"] {
        r.msg(Message::PaletteQuery(q.into()));
        r.snap(160);
    }
    r.snap(500);
    r.msg(Message::PaletteSubmit);
    r.snap(1200);
    r.msg(Message::SetSyntax(syntax::Syntax::Auto));
    r.color();
    r.snap(1100);

    // 11 ------------------------------------------------------------- indentation and line endings
    r.begin("convert", "Convert indentation and line endings in one step", "Girinti ve satır sonlarını tek adımda dönüştür");
    r.open_file("web/legacy.js");
    r.snap(1200);
    r.point_at("View", 700);
    r.click();
    r.msg(Message::OpenMenu(2));
    r.snap(600);
    r.point_at("Convert Indentation to Spaces", 700);
    r.click();
    r.msg(Message::MenuPick(Box::new(Message::ConvertIndent { spaces: true })));
    r.color();
    r.snap(1400);
    r.point_at("View", 600);
    r.click();
    r.msg(Message::OpenMenu(2));
    r.snap(500);
    r.point_at("Line Endings: Windows (CRLF)", 700);
    r.click();
    r.msg(Message::MenuPick(Box::new(Message::SetLineEnding(Eol::CrLf))));
    r.color();
    r.snap(1500);

    // 12 ------------------------------------------------------------- hex and images
    r.begin("hex", "A hex editor and an image viewer, built in", "Hex editörü ve resim görüntüleyici hazır");
    r.open_file("assets/data.bin");
    r.snap(2300);
    r.open_file("assets/logo.png");
    r.snap(1800);

    // 13 ------------------------------------------------------------- hot exit
    r.begin("hot-exit", "Unsaved tabs come back after a restart", "Kaydedilmemiş sekmeler yeniden başlatınca geri gelir");
    r.msg(Message::NewFile);
    r.type_text("call Ayşe about the invoice", 35);
    r.msg(Message::NewFile);
    r.type_text("SELECT * FROM orders WHERE paid = 0;", 28);
    r.msg(Message::NewFile);
    r.type_text("ideas: dark mode for the docs site", 28);
    r.color();
    r.snap(1500);
    r.keys("Ctrl+Q", 1500);
    r.snap(900);
    r.restart(1500);
    r.snap(2800);

    // 14 ------------------------------------------------------------- settings
    r.begin("settings", "Themes, fonts and optional trim-on-save", "Temalar, yazı tipi ve isteğe bağlı kaydederken boşluk temizleme");
    r.open_file("src/main.rs");
    r.snap(300);
    r.keys("Ctrl+,", 1300);
    r.msg(Message::OpenSettingsPanel);
    r.snap(1700);
    r.msg(Message::ToggleSetting(Toggle::TrimOnSave));
    r.snap(1800);
    r.msg(Message::Escape);
    r.snap(300);

    r.finish();
}
