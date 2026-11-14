//! The editor widget: line-number gutter (+ git marks), syntax-colored text,
//! selection, caret, horizontal/vertical scrolling and a minimap.
//!
//! It owns no document state. It reads an [`EditorState`], draws it, and turns
//! mouse/keyboard input into [`Action`]s that the app applies (Elm style).
//! The few things only layout can know (character width, viewport size) are
//! written back into the state's `Metrics` cell during `layout`.

use std::time::{Duration, Instant};

use iced::{
    Border, Color, Element, Event, Length, Pixels, Point, Rectangle, Shadow, Size, Theme,
    advanced::{
        Clipboard, Layout, Renderer as _, Shell, Widget, clipboard, layout, mouse,
        renderer::{self, Quad},
        text::{self, LineHeight, Paragraph as _, Renderer as _, Shaping, Text, Wrapping},
        widget::{
            self, Operation, Tree, tree,
            operation::Focusable,
        },
    },
    alignment, keyboard,
};

use crate::{
    editor::state::{Action, EditorState, MAX_COLS, Metrics, Motion, char_width, display_width},
    git::{GitMarks, Mark},
    syntax::{Group, LineSpans, Span},
    ui::theme::C,
};

/// Width of the minimap (hidden when the widget is narrower than
/// `MINIMAP_MIN_WIDTH`).
const MINIMAP_W: f32 = 110.0;
const MINIMAP_MIN_WIDTH: f32 = 560.0;
/// Minimap scale: 1 px per column, 2 px per line.
const MM_LINE_H: f32 = 2.0;
/// Width of the vertical scrollbar column / height of the horizontal one.
const BAR_W: f32 = 12.0;
/// Shortest scrollbar thumb.
const THUMB_MIN: f32 = 28.0;
/// Width of the gutter beyond the digits, in characters: a margin on the left, the line
/// number, then [`FOLD_ZONE`] for the fold arrow.
const GUTTER_EXTRA: f32 = 3.2;
/// The fold arrow's clickable zone at the gutter's right edge, in characters.
const FOLD_ZONE: f32 = 2.0;
/// Space between gutter and first text column.
const PAD: f32 = 6.0;
/// Half a blink period.
const BLINK: Duration = Duration::from_millis(530);
const MEASURE: &str = "MMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMM";

type Renderer = iced::Renderer;

pub struct EditorView<'a, Message> {
    id: Option<widget::Id>,
    state: &'a EditorState,
    spans: Option<&'a LineSpans>,
    git: Option<&'a GitMarks>,
    /// Find results as `(start, end)` char ranges (only when up to date).
    matches: Option<&'a [(usize, usize)]>,
    colors: C,
    font_size: f32,
    modal: bool,
    minimap: bool,
    wrap: bool,
    on_action: Box<dyn Fn(Action) -> Message + 'a>,
}

impl<'a, Message> EditorView<'a, Message> {
    pub fn new(
        state: &'a EditorState,
        colors: C,
        font_size: f32,
        on_action: impl Fn(Action) -> Message + 'a,
    ) -> Self {
        Self {
            id: None,
            state,
            spans: None,
            git: None,
            matches: None,
            colors,
            font_size,
            modal: false,
            minimap: true,
            wrap: false,
            on_action: Box::new(on_action),
        }
    }

    pub fn id(mut self, id: impl Into<widget::Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn spans(mut self, spans: Option<&'a LineSpans>) -> Self {
        self.spans = spans;
        self
    }

    pub fn git(mut self, git: Option<&'a GitMarks>) -> Self {
        self.git = git;
        self
    }

    pub fn matches(mut self, matches: Option<&'a [(usize, usize)]>) -> Self {
        self.matches = matches;
        self
    }

    /// Soft-wrap long lines at the edge of the text area.
    pub fn wrap(mut self, on: bool) -> Self {
        self.wrap = on;
        self
    }

    pub fn minimap(mut self, on: bool) -> Self {
        self.minimap = on;
        self
    }

    /// While a dialog/palette is open the editor ignores all input.
    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    fn line_h(&self) -> f32 {
        (self.font_size * 1.4).round()
    }
}

// ------------------------------------------------------------------ geometry

struct Geo {
    /// Vertical scrollbar column at the right edge.
    vbar: Rectangle,
    gutter_w: f32,
    /// Absolute x of the text area (right of the gutter).
    text_left: f32,
    text_right: f32,
    minimap: Option<Rectangle>,
}

impl Geo {
    fn text_clip(&self, b: Rectangle) -> Rectangle {
        Rectangle { x: self.text_left, y: b.y, width: (self.text_right - self.text_left).max(0.0), height: b.height }
    }

    /// Absolute x where column 0 of the text starts (before scrolling).
    fn origin_x(&self) -> f32 {
        self.text_left + PAD
    }
}

fn geometry(state: &EditorState, bounds: Rectangle, char_w: f32, minimap_on: bool) -> Geo {
    let digits = state.line_count().max(1).to_string().len().max(3);
    let gutter_w = (digits as f32 + GUTTER_EXTRA) * char_w;
    // The scrollbar column is always reserved: it must not depend on the
    // content (wrapping depends on the text width, which would loop).
    let vbar = Rectangle { x: bounds.x + bounds.width - BAR_W, y: bounds.y, width: BAR_W, height: bounds.height };
    let minimap = (minimap_on && bounds.width >= MINIMAP_MIN_WIDTH).then_some(Rectangle {
        x: vbar.x - MINIMAP_W,
        y: bounds.y,
        width: MINIMAP_W,
        height: bounds.height,
    });
    Geo {
        vbar,
        gutter_w,
        text_left: bounds.x + gutter_w,
        text_right: minimap.map_or(vbar.x, |m| m.x),
        minimap,
    }
}

/// Vertical offset of the minimap content: proportional to the editor scroll so
/// the whole file maps onto the minimap height (Sublime style).
fn minimap_scroll(state: &EditorState, m: Metrics, view_h: f32) -> f32 {
    let total = state.line_count() as f32 * MM_LINE_H;
    if total <= view_h || m.line_h <= 0.0 {
        return 0.0;
    }
    let max_scroll = (state.total_rows() as f32 * m.line_h - m.view_h / 3.0).max(1.0);
    (state.scroll_y / max_scroll).clamp(0.0, 1.0) * (total - view_h)
}

// ------------------------------------------------------------------- runs

/// A same-colored piece of one line.
#[derive(Debug, PartialEq)]
pub struct Run {
    /// Display column where the run starts.
    pub col: usize,
    pub text: String,
    pub group: Option<Group>,
}

/// Splits a line into colored runs for display columns `[from_col, to_col)`.
/// Tabs only advance the column; control chars are shown as `·`. Spans may be
/// stale (one edit behind): offsets are compared, never sliced, so they cannot
/// go out of range or split a character.
pub fn build_runs(line: &str, spans: &[Span], tab: usize, from_col: usize, to_col: usize) -> Vec<Run> {
    let mut out = Vec::new();
    let mut cur: Option<Run> = None;
    let mut col = 0;
    let mut si = 0;
    let flush = |cur: &mut Option<Run>, out: &mut Vec<Run>| {
        if let Some(r) = cur.take() {
            if !r.text.trim().is_empty() {
                out.push(r);
            }
        }
    };
    for (b, ch) in line.char_indices() {
        if col >= to_col {
            break;
        }
        let w = char_width(ch, col, tab);
        if ch == '\t' {
            flush(&mut cur, &mut out);
            col += w;
            continue;
        }
        if col + w <= from_col {
            col += w;
            continue;
        }
        while si < spans.len() && spans[si].end as usize <= b {
            si += 1;
        }
        let group = spans.get(si).filter(|s| s.start as usize <= b).map(|s| s.group);
        if cur.as_ref().is_some_and(|r| r.group != group) {
            flush(&mut cur, &mut out);
        }
        let run = cur.get_or_insert_with(|| Run { col, text: String::new(), group });
        run.text.push(if ch.is_control() { '\u{00B7}' } else { ch });
        col += w;
    }
    flush(&mut cur, &mut out);
    out
}

// ------------------------------------------------------------- widget state

#[derive(Default)]
struct State {
    focused: bool,
    drag: Option<Drag>,
    last_click: Option<mouse::Click>,
    modifiers: keyboard::Modifiers,
    /// `(font size, measured character width)`.
    char_w: Option<(f32, f32)>,
    /// The caret is solid for a moment after every input, then blinks.
    blink: Option<Instant>,
    /// Visual row under the pointer while it is over the gutter (fold arrows show there).
    hover: Option<usize>,
    /// Touchpad scroll collected towards one zoom step.
    zoom_acc: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    Text,
    Minimap,
    /// Scrollbar thumb; `grab` = pointer offset inside the thumb.
    VBar { grab: f32 },
    HBar { grab: f32 },
}

impl Focusable for State {
    fn is_focused(&self) -> bool {
        self.focused
    }
    fn focus(&mut self) {
        self.focused = true;
    }
    fn unfocus(&mut self) {
        self.focused = false;
    }
}

pub(super) fn measure_char_w(size: f32) -> f32 {
    let p = <Renderer as text::Renderer>::Paragraph::with_text(Text {
        content: MEASURE,
        bounds: Size::INFINITE,
        size: Pixels(size),
        line_height: LineHeight::Absolute(Pixels(size * 1.4)),
        font: super::theme::MONO,
        align_x: text::Alignment::Left,
        align_y: alignment::Vertical::Top,
        shaping: Shaping::Basic,
        wrapping: Wrapping::None,
    });
    p.min_bounds().width / MEASURE.chars().count() as f32
}

// ------------------------------------------------------------------- input

enum Key {
    Act(Action),
    Copy,
    Cut,
    Paste,
    /// Not ours (app shortcuts such as Ctrl+S): let it bubble up.
    Ignore,
}

fn translate_key(
    key: &keyboard::Key,
    logical: &keyboard::Key,
    physical: keyboard::key::Physical,
    mods: keyboard::Modifiers,
    text: Option<&str>,
    multi: bool,
) -> Key {
    use keyboard::{Key as K, key::Named};
    let (ctrl, shift) = (mods.command(), mods.shift());
    // The numpad's digits are digits (or End / ↓ / ... without NumLock), whatever `key` says.
    let key = super::effective_key(key, logical, physical);
    let mv = |motion| Key::Act(Action::Move { motion, extend: shift });

    if let K::Named(named) = key.as_ref() {
        match named {
            Named::ArrowLeft => return mv(if ctrl { Motion::WordLeft } else { Motion::Left }),
            Named::ArrowRight => return mv(if ctrl { Motion::WordRight } else { Motion::Right }),
            Named::ArrowUp if ctrl && mods.alt() => return Key::Act(Action::AddCursorAbove),
            Named::ArrowDown if ctrl && mods.alt() => return Key::Act(Action::AddCursorBelow),
            Named::ArrowUp => return mv(Motion::Up),
            Named::ArrowDown => return mv(Motion::Down),
            Named::Escape if multi => return Key::Act(Action::Collapse),
            Named::Home => return mv(if ctrl { Motion::DocStart } else { Motion::LineStart }),
            Named::End => return mv(if ctrl { Motion::DocEnd } else { Motion::LineEnd }),
            Named::PageUp if !ctrl => return mv(Motion::PageUp),
            Named::PageDown if !ctrl => return mv(Motion::PageDown),
            Named::Backspace => {
                return Key::Act(if ctrl { Action::DeleteWordBack } else { Action::Backspace });
            }
            Named::Delete => {
                return Key::Act(if ctrl { Action::DeleteWordForward } else { Action::Delete });
            }
            Named::Enter if !ctrl => return Key::Act(Action::Enter),
            Named::Tab if !ctrl => {
                return Key::Act(if shift { Action::Unindent } else { Action::Indent });
            }
            _ => {}
        }
    }
    if ctrl && shift {
        // Fold / unfold sit on the bracket *keys*, wherever the layout puts its brackets
        // (on a Turkish keyboard they are the ğ and ü keys).
        use keyboard::key::{Code, Physical};
        match physical {
            Physical::Code(Code::BracketLeft) => return Key::Act(Action::Fold),
            Physical::Code(Code::BracketRight) => return Key::Act(Action::Unfold),
            _ => {}
        }
    }
    if ctrl {
        // Normalize case: with Shift held some platforms report the upper-case letter.
        return match key.to_latin(physical).map(|c| c.to_ascii_lowercase()) {
            Some('a') => Key::Act(Action::SelectAll),
            Some('d') => Key::Act(Action::SelectNextOccurrence),
            Some('c') => Key::Copy,
            Some('x') => Key::Cut,
            Some('v') => Key::Paste,
            Some('z') if shift => Key::Act(Action::Redo),
            Some('z') => Key::Act(Action::Undo),
            Some('y') => Key::Act(Action::Redo),
            // Ctrl+Shift+[ / ]: fold / unfold (some layouts report the shifted `{` / `}`).
            Some('[' | '{') if shift => Key::Act(Action::Fold),
            Some(']' | '}') if shift => Key::Act(Action::Unfold),
            _ => Key::Ignore,
        };
    }
    match text {
        Some(t) if !t.is_empty() && !t.chars().any(char::is_control) => Key::Act(Action::Insert(t.to_owned())),
        _ => Key::Ignore,
    }
}

fn hbar_target(x: f32, grab: f32, track: Rectangle, thumb_w: f32, max_x: f32) -> Action {
    let free = (track.width - thumb_w).max(1.0);
    Action::ScrollToX { x: ((x - grab - track.x) / free).clamp(0.0, 1.0) * max_x }
}

impl<'a, Message> EditorView<'a, Message> {
    /// Mouse position (relative to the widget) -> (line, display column in the line).
    fn hit(&self, pos: Point, g: &Geo, bounds: Rectangle, m: Metrics) -> (usize, usize) {
        let st = self.state;
        let row = (((pos.y + st.scroll_y) / m.line_h).floor().max(0.0) as usize).min(st.total_rows().saturating_sub(1));
        let x = pos.x + bounds.x - g.origin_x() + st.scroll_x;
        let col = (x / m.char_w + 0.5).floor().max(0.0) as usize;
        let sp = st.row_span(row);
        (sp.line, (sp.start_col + col).min(MAX_COLS))
    }

    /// Scroll offset for a pointer at absolute `y` holding the thumb `grab` px from its top.
    fn vbar_target(&self, y: f32, grab: f32, g: &Geo, thumb_h: f32, _m: Metrics) -> Action {
        let free = (g.vbar.height - thumb_h).max(1.0);
        let t = ((y - grab - g.vbar.y) / free).clamp(0.0, 1.0);
        Action::ScrollTo { y: t * self.state.max_scroll_y() }
    }

    /// Vertical scrollbar thumb (absolute coordinates), when there is anything to scroll.
    fn vthumb(&self, g: &Geo, m: Metrics) -> Option<Rectangle> {
        let max_y = self.state.max_scroll_y();
        if max_y <= 0.0 || m.view_h <= 0.0 {
            return None;
        }
        let track = g.vbar.height;
        let h = (m.view_h / (max_y + m.view_h) * track).clamp(THUMB_MIN.min(track), track);
        let y = g.vbar.y + (self.state.scroll_y / max_y).clamp(0.0, 1.0) * (track - h);
        Some(Rectangle { x: g.vbar.x + 3.0, y, width: BAR_W - 6.0, height: h })
    }

    /// Horizontal scrollbar `(track, thumb, max scroll)`; none when wrapping or when
    /// everything fits. The content width is estimated from the visible lines (the
    /// exact widest line would be O(lines) after every edit).
    fn hbar(&self, g: &Geo, bounds: Rectangle, m: Metrics) -> Option<(Rectangle, Rectangle, f32)> {
        let s = self.state;
        if s.wrapping() || m.text_w <= 0.0 || m.line_h <= 0.0 {
            return None;
        }
        let first = (s.scroll_y / m.line_h).floor().max(0.0) as usize;
        let last = first + (bounds.height / m.line_h).ceil() as usize + 2;
        let rows = s.total_rows();
        // Rows are not lines once something is folded away.
        let (a, b) = (s.line_of_row(first.min(rows.saturating_sub(1))), s.line_of_row(last.min(rows).saturating_sub(1)) + 1);
        let mut content_w = (s.widest_of(a..b).min(MAX_COLS) as f32 + 4.0) * m.char_w;
        if s.scroll_x > 0.0 {
            content_w = content_w.max(s.scroll_x + m.text_w); // scrolled: at least what we have seen
        }
        let max_x = content_w - m.text_w;
        if max_x <= 1.0 {
            return None;
        }
        let track = Rectangle {
            x: g.text_left,
            y: bounds.y + bounds.height - BAR_W,
            width: (g.text_right - g.text_left).max(0.0),
            height: BAR_W,
        };
        let w = (m.text_w / content_w * track.width).clamp(THUMB_MIN.min(track.width), track.width);
        let x = track.x + (s.scroll_x / max_x).clamp(0.0, 1.0) * (track.width - w);
        Some((track, Rectangle { x, y: track.y + 3.0, width: w, height: BAR_W - 6.0 }, max_x))
    }

    fn minimap_target(&self, pos: Point, g: &Geo, m: Metrics) -> Action {
        let mm = g.minimap.expect("only called when the minimap exists");
        let scroll = minimap_scroll(self.state, m, mm.height);
        let line = (((pos.y + scroll) / MM_LINE_H).max(0.0) as usize).min(self.state.line_count().saturating_sub(1));
        let row = self.state.rows_of_line(line).start;
        Action::ScrollTo { y: row as f32 * m.line_h - m.view_h / 2.0 }
    }
}

impl<Message> Widget<Message, Theme, Renderer> for EditorView<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        let size = limits.resolve(Length::Fill, Length::Fill, Size::ZERO);
        let st = tree.state.downcast_mut::<State>();
        let char_w = match st.char_w {
            Some((fs, w)) if fs == self.font_size => w,
            _ => {
                let w = measure_char_w(self.font_size);
                st.char_w = Some((self.font_size, w));
                w
            }
        };
        let g = geometry(self.state, Rectangle::new(Point::ORIGIN, size), char_w, self.minimap);
        let text_w = (g.text_right - g.text_left - PAD).max(0.0);
        // One cell is kept free for the caret at the end of a full row.
        self.state.set_wrap(self.wrap.then(|| ((text_w / char_w).floor() as usize).saturating_sub(1)));
        self.state.set_metrics(Metrics { char_w, line_h: self.line_h(), view_h: size.height, text_w });
        layout::Node::new(size)
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, _renderer: &Renderer, operation: &mut dyn Operation) {
        let st = tree.state.downcast_mut::<State>();
        operation.focusable(self.id.as_ref(), layout.bounds(), st);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let st = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        let m = self.state.metrics();
        if m.char_w <= 0.0 || m.line_h <= 0.0 {
            return; // not laid out yet
        }
        let g = geometry(self.state, bounds, m.char_w, self.minimap);
        let publish = |shell: &mut Shell<'_, Message>, a: Action| {
            shell.publish((self.on_action)(a));
            shell.capture_event();
        };

        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(mods)) => st.modifiers = *mods,
            Event::Window(iced::window::Event::RedrawRequested(now)) if st.focused && !self.modal => {
                // Wake up exactly when the caret has to toggle.
                let elapsed = now.saturating_duration_since(*st.blink.get_or_insert(*now));
                let into_phase = elapsed.as_millis() % BLINK.as_millis();
                shell.request_redraw_at(*now + BLINK - Duration::from_millis(into_phase as u64));
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(pos) = cursor.position_in(bounds) else {
                    st.focused = false;
                    return;
                };
                if self.modal {
                    return;
                }
                st.focused = true;
                st.blink = Some(Instant::now());
                let at = cursor.position().unwrap_or_default();
                if cursor.is_over(g.vbar) {
                    if let Some(thumb) = self.vthumb(&g, m) {
                        let grab = if thumb.contains(at) { at.y - thumb.y } else { thumb.height / 2.0 };
                        st.drag = Some(Drag::VBar { grab });
                        publish(shell, self.vbar_target(at.y, grab, &g, thumb.height, m));
                    } else {
                        shell.capture_event();
                    }
                    return;
                }
                if let Some((track, thumb, max_x)) = self.hbar(&g, bounds, m).filter(|(t, _, _)| cursor.is_over(*t)) {
                    let grab = if thumb.contains(at) { at.x - thumb.x } else { thumb.width / 2.0 };
                    st.drag = Some(Drag::HBar { grab });
                    publish(shell, hbar_target(at.x, grab, track, thumb.width, max_x));
                    return;
                }
                if g.minimap.is_some_and(|mm| cursor.is_over(mm)) {
                    st.drag = Some(Drag::Minimap);
                    publish(shell, self.minimap_target(Point::new(pos.x, pos.y), &g, m));
                    return;
                }
                let click = mouse::Click::new(cursor.position().unwrap_or(pos), mouse::Button::Left, st.last_click);
                let count = match click.kind() {
                    mouse::click::Kind::Single => 1,
                    mouse::click::Kind::Double => 2,
                    mouse::click::Kind::Triple => 3,
                };
                st.last_click = Some(click);
                let in_gutter = pos.x < g.gutter_w;
                let (line, col) = self.hit(pos, &g, bounds, m);
                // The fold arrow lives in the gutter's right-hand margin.
                if in_gutter && pos.x >= g.gutter_w - m.char_w * FOLD_ZONE && self.state.fold_toggleable(line) {
                    publish(shell, Action::ToggleFold { line });
                    return;
                }
                st.drag = Some(Drag::Text);
                publish(
                    shell,
                    Action::Click {
                        line,
                        col: if in_gutter { 0 } else { col },
                        extend: st.modifiers.shift(),
                        count: if in_gutter { 3 } else { count },
                        add: st.modifiers.command() && !in_gutter,
                    },
                );
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                // The app opens the context menu (it wraps this widget); here we only make
                // sure the caret is where the user clicked, unless that is inside a selection.
                let Some(pos) = cursor.position_in(bounds) else { return };
                if self.modal || !cursor.is_over(g.text_clip(bounds)) {
                    return;
                }
                st.focused = true;
                st.blink = Some(Instant::now());
                let (line, col) = self.hit(pos, &g, bounds, m);
                if !self.state.point_in_selection(line, col) {
                    // Not captured: the menu request must still reach the wrapper.
                    shell.publish((self.on_action)(Action::Click { line, col, extend: false, count: 1, add: false }));
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                // Fold arrows show for the hovered gutter row: redraw when that row changes.
                let over_gutter = bounds.contains(*position) && position.x - bounds.x < g.gutter_w;
                let hover = over_gutter
                    .then(|| ((position.y - bounds.y + self.state.scroll_y) / m.line_h).floor().max(0.0) as usize);
                if hover != st.hover {
                    st.hover = hover;
                    shell.request_redraw();
                }
                let Some(drag) = st.drag else { return };
                let pos = Point::new(position.x - bounds.x, position.y - bounds.y);
                match drag {
                    Drag::Text => {
                        let (line, col) = self.hit(pos, &g, bounds, m);
                        publish(shell, Action::Drag { line, col });
                    }
                    Drag::Minimap if g.minimap.is_some() => publish(shell, self.minimap_target(pos, &g, m)),
                    Drag::Minimap => {}
                    Drag::VBar { grab } => {
                        if let Some(thumb) = self.vthumb(&g, m) {
                            publish(shell, self.vbar_target(position.y, grab, &g, thumb.height, m));
                        }
                    }
                    Drag::HBar { grab } => {
                        if let Some((track, thumb, max_x)) = self.hbar(&g, bounds, m) {
                            publish(shell, hbar_target(position.x, grab, track, thumb.width, max_x));
                        }
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => st.drag = None,
            // Ctrl + wheel zooms the text instead of scrolling it.
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) && !self.modal && st.modifiers.command() => {
                let steps = match *delta {
                    mouse::ScrollDelta::Lines { y, .. } => y.signum() as i32 * i32::from(y != 0.0),
                    mouse::ScrollDelta::Pixels { y, .. } => {
                        st.zoom_acc += y;
                        let n = (st.zoom_acc / 30.0).trunc();
                        st.zoom_acc -= n * 30.0;
                        n as i32
                    }
                };
                if steps != 0 {
                    publish(shell, Action::Zoom(steps));
                } else {
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) && !self.modal => {
                let (mut dx, mut dy) = match *delta {
                    mouse::ScrollDelta::Lines { x, y } => (-x * m.char_w * 6.0, -y * m.line_h * 3.0),
                    mouse::ScrollDelta::Pixels { x, y } => (-x, -y),
                };
                if st.modifiers.shift() && dx == 0.0 {
                    std::mem::swap(&mut dx, &mut dy);
                }
                publish(shell, Action::Scroll { dx, dy });
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key, modified_key, physical_key, modifiers, text, .. })
                if st.focused && !self.modal =>
            {
                st.modifiers = *modifiers;
                st.blink = Some(Instant::now());
                match translate_key(key, modified_key, *physical_key, *modifiers, text.as_deref(), self.state.cursor_count() > 1) {
                    Key::Act(a) => publish(shell, a),
                    Key::Copy => {
                        if let Some(t) = self.state.selected_text() {
                            clipboard.write(clipboard::Kind::Standard, t);
                        }
                        shell.capture_event();
                    }
                    Key::Cut => {
                        if let Some(t) = self.state.selected_text() {
                            clipboard.write(clipboard::Kind::Standard, t);
                            publish(shell, Action::DeleteSelection);
                        }
                    }
                    Key::Paste => {
                        if let Some(t) = clipboard.read(clipboard::Kind::Standard) {
                            publish(shell, Action::Insert(t));
                        }
                    }
                    Key::Ignore => {}
                }
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        let st = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let m = self.state.metrics();
        let g = geometry(self.state, bounds, m.char_w, self.minimap);
        if st.drag == Some(Drag::Minimap) || g.minimap.is_some_and(|mm| cursor.is_over(mm)) {
            mouse::Interaction::Pointer
        } else if cursor.is_over(g.text_clip(bounds)) && !self.modal {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::None
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let st = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let m = self.state.metrics();
        if m.char_w <= 0.0 {
            return;
        }
        let c = self.colors;
        let g = geometry(self.state, bounds, m.char_w, self.minimap);
        let s = self.state;
        let tab = s.tab_size();
        let lines = s.line_count();
        let rows = s.total_rows();
        let wrapping = s.wrapping();
        let first = ((s.scroll_y / m.line_h).floor().max(0.0) as usize).min(rows);
        let last = (first + (bounds.height / m.line_h).ceil() as usize + 2).min(rows);
        let (caret_line, _) = s.caret_line_col();
        let sel_empty = s.selection().is_empty();
        let from_col = (s.scroll_x / m.char_w).floor() as usize;
        let to_col = (((s.scroll_x + m.text_w) / m.char_w).ceil() as usize + 2).min(MAX_COLS);
        let text_clip = g.text_clip(bounds);
        let hover_line = st.hover.filter(|_| rows > 0).map(|row| s.row_span(row.min(rows - 1)).line);
        let origin_x = g.origin_x();
        let y_of = |i: usize| bounds.y + i as f32 * m.line_h - s.scroll_y;

        let quad = |r: &mut Renderer, rect: Rectangle, color: Color| {
            r.fill_quad(Quad { bounds: rect, border: Border::default(), shadow: Shadow::default(), snap: true }, color);
        };
        let text = |r: &mut Renderer, content: String, size: Pixels, w: f32, pos: Point, color: Color, clip: Rectangle| {
            r.fill_text(
                Text {
                    content,
                    bounds: Size::new(w, m.line_h),
                    size,
                    line_height: LineHeight::Absolute(Pixels(m.line_h)),
                    font: super::theme::MONO,
                    align_x: text::Alignment::Left,
                    align_y: alignment::Vertical::Top,
                    shaping: Shaping::Auto,
                    wrapping: Wrapping::None,
                },
                pos,
                color,
                clip,
            );
        };
        let size = Pixels(self.font_size);

        renderer.with_layer(bounds, |r| quad(r, bounds, c.bg));

        // --- text area: current line, selection, glyphs, caret
        renderer.with_layer(text_clip, |r| {
            for row in first..last {
                let sp = s.row_span(row);
                let y = y_of(row);
                if sp.line == caret_line && sel_empty && st.focused && s.cursor_count() == 1 {
                    quad(r, Rectangle { x: text_clip.x, y, width: text_clip.width, height: m.line_h }, Color { a: 0.55, ..c.hover });
                }
                for (c0, c1, nl) in s.selections_in_row(row) {
                    let x = origin_x + c0 as f32 * m.char_w - s.scroll_x;
                    let w = (c1 - c0) as f32 * m.char_w + if nl { m.char_w * 0.6 } else { 0.0 };
                    quad(r, Rectangle { x, y, width: w.max(2.0), height: m.line_h }, Color { a: 0.32, ..c.accent });
                }
                if let Some(found) = self.matches {
                    let ls = s.line_start(sp.line);
                    let (rs, re) = (ls + sp.start, ls + sp.end);
                    let cur = s.selection().range();
                    let from = found.partition_point(|f| f.1 <= rs);
                    for &(f0, f1) in found[from..].iter().take_while(|f| f.0 <= re) {
                        let (a, b) = (f0.max(rs), f1.min(re));
                        let (_, c0) = s.line_and_display_col(a);
                        let (_, c1) = s.line_and_display_col(b);
                        let x = origin_x + (c0 - sp.start_col) as f32 * m.char_w - s.scroll_x;
                        let alpha = if (f0, f1) == cur { 0.55 } else { 0.28 };
                        quad(r, Rectangle { x, y, width: ((c1 - c0) as f32 * m.char_w).max(2.0), height: m.line_h }, Color { a: alpha, ..c.dirty });
                    }
                }
            }
            // Glyphs: one colored run per piece; a long line's rows reuse its string.
            let mut cached: Option<(usize, String)> = None;
            for row in first..last {
                let sp = s.row_span(row);
                let y = y_of(row);
                if cached.as_ref().is_none_or(|(l, _)| *l != sp.line) {
                    cached = Some((sp.line, s.line_string(sp.line)));
                }
                let line = &cached.as_ref().expect("just set").1;
                let spans = self.spans.map_or(&[][..], |ls| ls.line(sp.line));
                let (from, to) = if wrapping {
                    (sp.start_col, sp.end_col.unwrap_or(sp.start_col + (m.text_w / m.char_w) as usize + 4))
                } else {
                    (from_col, to_col)
                };
                for run in build_runs(line, spans, tab, from, to) {
                    let color = run.group.map_or(c.text_bright, |gr| c.syntax(gr));
                    let w = run.text.chars().count() as f32 * m.char_w + m.char_w * 2.0;
                    let x = origin_x + (run.col - sp.start_col) as f32 * m.char_w - s.scroll_x;
                    text(r, run.text, size, w, Point::new(x, y), color, text_clip);
                }
                // A collapsed block: `…` after the header (and its closing brace, if it has one).
                if let (true, Some(fold)) = (sp.last, s.fold_at(sp.line)) {
                    let end_col = display_width(line.chars(), tab);
                    let x = origin_x + (end_col - sp.start_col) as f32 * m.char_w - s.scroll_x + m.char_w * 0.6;
                    let pill = Rectangle { x, y: y + 2.0, width: m.char_w * 2.6, height: m.line_h - 4.0 };
                    r.fill_quad(
                        Quad {
                            bounds: pill,
                            border: Border { color: Color { a: 0.5, ..c.dim }, width: 1.0, radius: 3.0.into() },
                            shadow: Shadow::default(),
                            snap: true,
                        },
                        Color { a: 0.18, ..c.dim },
                    );
                    text(r, "\u{2026}".into(), size, m.char_w * 2.0, Point::new(x + m.char_w * 0.8, y), c.dim, text_clip);
                    if fold.closer {
                        let closer = s.line_string(fold.end);
                        let closer = closer.trim();
                        text(r, closer.to_owned(), size, closer.chars().count() as f32 * m.char_w + m.char_w * 2.0, Point::new(x + m.char_w * 3.2, y), c.text_bright, text_clip);
                    }
                }
            }
            let blink_on = st.blink.is_none_or(|t| (t.elapsed().as_millis() / BLINK.as_millis()) % 2 == 0);
            if st.focused && !self.modal {
                for (row, col, _primary) in s.carets_in_rows(first..last).into_iter().filter(|_| blink_on) {
                    let x = origin_x + col.min(MAX_COLS) as f32 * m.char_w - s.scroll_x;
                    quad(r, Rectangle { x, y: y_of(row), width: 2.0, height: m.line_h }, c.text_bright);
                }
                // Matching brackets of the primary caret.
                if let Some((a, b)) = s.matching_bracket() {
                    for idx in [a, b] {
                        let (row, col) = s.row_of(idx);
                        if s.has_folds() && s.line_hidden(s.line_and_display_col(idx).0) {
                            continue; // its line is folded away
                        }
                        if (first..last).contains(&row) {
                            let x = origin_x + col as f32 * m.char_w - s.scroll_x;
                            r.fill_quad(
                                Quad {
                                    bounds: Rectangle { x, y: y_of(row), width: m.char_w, height: m.line_h },
                                    border: Border { color: Color { a: 0.8, ..c.accent }, width: 1.0, radius: 2.0.into() },
                                    shadow: Shadow::default(),
                                    snap: true,
                                },
                                Color::TRANSPARENT,
                            );
                        }
                    }
                }
            }
        });

        // --- gutter: git marks + line numbers
        let gutter = Rectangle { x: bounds.x, y: bounds.y, width: g.gutter_w, height: bounds.height };
        renderer.with_layer(gutter, |r| {
            quad(r, gutter, c.bg);
            for row in first..last {
                let sp = s.row_span(row);
                let y = y_of(row);
                if let Some(mark) = self.git.and_then(|gm| gm.get(sp.line)) {
                    let (color, rect) = match mark {
                        Mark::Added => (c.hex_ascii, Rectangle { x: bounds.x + 1.0, y, width: 3.0, height: m.line_h }),
                        Mark::Modified => (c.dirty, Rectangle { x: bounds.x + 1.0, y, width: 3.0, height: m.line_h }),
                        Mark::Removed if sp.first => (c.danger, Rectangle { x: bounds.x + 1.0, y: y - 2.0, width: 8.0, height: 3.0 }),
                        Mark::Removed => continue,
                    };
                    quad(r, rect, color);
                }
                if sp.first {
                    // Fold arrow: always on a collapsed block, on hover for a block that can be collapsed.
                    let folded = s.fold_at(sp.line).is_some();
                    if folded || (hover_line == Some(sp.line) && s.is_foldable(sp.line)) {
                        // A chip the size of the click zone, so what you see is what you hit.
                        let zone = Rectangle {
                            x: bounds.x + g.gutter_w - m.char_w * FOLD_ZONE,
                            y: y + 1.0,
                            width: m.char_w * FOLD_ZONE - 2.0,
                            height: m.line_h - 2.0,
                        };
                        if hover_line == Some(sp.line) {
                            r.fill_quad(
                                Quad { bounds: zone, border: Border { radius: 3.0.into(), ..Border::default() }, shadow: Shadow::default(), snap: true },
                                Color { a: 0.28, ..c.dim },
                            );
                        }
                        let glyph = if folded { "\u{25B8}" } else { "\u{25BE}" };
                        let gsize = Pixels(self.font_size * 1.8);
                        text(
                            r,
                            glyph.into(),
                            gsize,
                            zone.width + 4.0,
                            Point::new(zone.x + (zone.width - gsize.0 * 0.55) / 2.0, y - (gsize.0 - self.font_size) * 0.5),
                            if folded { c.accent } else { c.text_bright },
                            gutter,
                        );
                    }
                    let label = (sp.line + 1).to_string();
                    let w = label.len() as f32 * m.char_w;
                    let x = bounds.x + g.gutter_w - m.char_w * (FOLD_ZONE + 0.35) - w;
                    let color = if sp.line == caret_line { c.text } else { c.dim };
                    text(r, label, size, w + m.char_w, Point::new(x, y), color, gutter);
                }
            }
        });

        // --- minimap
        if let Some(mm) = g.minimap {
            renderer.with_layer(mm, |r| {
                quad(r, mm, c.sidebar);
                let scroll = minimap_scroll(s, m, mm.height);
                let mm_first = (scroll / MM_LINE_H).floor() as usize;
                let mm_last = (mm_first + (mm.height / MM_LINE_H).ceil() as usize + 2).min(lines);
                for i in mm_first..mm_last {
                    let y = mm.y + i as f32 * MM_LINE_H - scroll;
                    let line = s.line_string(i);
                    let spans = self.spans.map_or(&[][..], |sp| sp.line(i));
                    for run in build_runs(&line, spans, tab, 0, (MINIMAP_W as usize) - 8) {
                        let color = run.group.map_or(c.dim, |gr| c.syntax(gr));
                        for (col, len) in words(&run) {
                            quad(
                                r,
                                Rectangle { x: mm.x + 6.0 + col as f32, y, width: len as f32, height: MM_LINE_H - 0.5 },
                                Color { a: 0.75, ..color },
                            );
                        }
                    }
                }
                // The visible part of the file.
                let top_line = s.line_of_row(first.min(rows.saturating_sub(1)));
                let bottom_line = s.line_of_row(last.saturating_sub(1).min(rows.saturating_sub(1)));
                let y0 = mm.y + top_line as f32 * MM_LINE_H - scroll;
                let h = (((bottom_line + 1 - top_line.min(bottom_line)) as f32) * MM_LINE_H).min(mm.height);
                quad(r, Rectangle { x: mm.x, y: y0, width: mm.width, height: h }, Color { a: 0.06, ..c.text_bright });
                quad(r, Rectangle { x: mm.x, y: mm.y, width: 1.0, height: mm.height }, c.border);
            });
        }

        // --- scrollbars
        let thumb_color = |thumb: Rectangle, dragging: bool| {
            let a = if dragging { 0.6 } else if cursor.is_over(thumb) { 0.45 } else { 0.28 };
            Color { a, ..c.text }
        };
        let round = |r: &mut Renderer, rect: Rectangle, color: Color| {
            r.fill_quad(
                Quad { bounds: rect, border: Border { radius: (rect.width.min(rect.height) / 2.0).into(), ..Border::default() }, shadow: Shadow::default(), snap: true },
                color,
            );
        };
        renderer.with_layer(g.vbar, |r| {
            quad(r, g.vbar, c.bg);
            if g.minimap.is_none() {
                quad(r, Rectangle { x: g.vbar.x, y: g.vbar.y, width: 1.0, height: g.vbar.height }, c.border);
            }
            if let Some(thumb) = self.vthumb(&g, m) {
                round(r, thumb, thumb_color(thumb, matches!(st.drag, Some(Drag::VBar { .. }))));
            }
        });
        if let Some((track, thumb, _)) = self.hbar(&g, bounds, m) {
            renderer.with_layer(track, |r| {
                quad(r, track, Color { a: 0.85, ..c.bg });
                round(r, thumb, thumb_color(thumb, matches!(st.drag, Some(Drag::HBar { .. }))));
            });
        }
    }
}

/// `(column, length)` of the non-blank words inside a run, in display columns.
fn words(run: &Run) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut col = run.col;
    let mut start: Option<usize> = None;
    for ch in run.text.chars() {
        let w = char_width(ch, col, 4);
        if ch.is_whitespace() {
            if let Some(s) = start.take() {
                out.push((s, col - s));
            }
        } else if start.is_none() {
            start = Some(col);
        }
        col += w;
    }
    if let Some(s) = start {
        out.push((s, col - s));
    }
    out
}

impl<'a, Message: 'a> From<EditorView<'a, Message>> for Element<'a, Message, Theme, Renderer> {
    fn from(view: EditorView<'a, Message>) -> Self {
        Element::new(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: u32, end: u32, group: Group) -> Span {
        Span { start, end, group }
    }

    fn runs(line: &str, spans: &[Span]) -> Vec<(usize, String, Option<Group>)> {
        build_runs(line, spans, 4, 0, 1000).into_iter().map(|r| (r.col, r.text, r.group)).collect()
    }

    #[test]
    fn runs_follow_spans_and_skip_blank_pieces() {
        let line = "fn main() {";
        let spans = [span(0, 2, Group::Keyword), span(3, 7, Group::Function)];
        assert_eq!(
            runs(line, &spans),
            vec![
                (0, "fn".into(), Some(Group::Keyword)),
                (3, "main".into(), Some(Group::Function)),
                (7, "() {".into(), None),
            ],
            "the blank gap between tokens produces no run"
        );
    }

    #[test]
    fn tabs_advance_columns_and_wide_chars_take_two() {
        let r = runs("\tab\u{65E5}c", &[]);
        assert_eq!(r, vec![(4, "ab\u{65E5}c".into(), None)]);
        // Visible window: columns before `from` are skipped, a run may start earlier.
        let r = build_runs("abcdefgh", &[], 4, 3, 6);
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].col, r[0].text.as_str()), (3, "def"));
    }

    #[test]
    fn stale_spans_never_break_runs() {
        // Spans computed for a longer / different line, with multibyte chars.
        let line = "é = ğ";
        let spans = [span(0, 40, Group::String), span(500, 600, Group::Comment)];
        let r = runs(line, &spans);
        assert_eq!(r, vec![(0, "é = ğ".into(), Some(Group::String))]);
        // A span boundary in the middle of a multibyte char (stale offsets) is fine.
        let r = runs("éa", &[span(1, 3, Group::Number)]);
        assert_eq!(r.iter().map(|r| r.1.as_str()).collect::<String>(), "éa");
    }

    #[test]
    fn control_chars_are_visible_and_minimap_words_split_on_blanks() {
        let r = runs("a\u{1}b", &[]);
        assert_eq!(r[0].1, "a\u{00B7}b");
        let run = Run { col: 2, text: "ab  cd e".into(), group: None };
        assert_eq!(words(&run), vec![(2, 2), (6, 2), (9, 1)]);
    }

    fn k(c: &str) -> keyboard::Key {
        keyboard::Key::Character(c.into())
    }

    fn key(key: keyboard::Key, mods: keyboard::Modifiers, text: Option<&str>) -> Option<Action> {
        let physical = keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
        match translate_key(&key, &key, physical, mods, text, false) {
            Key::Act(a) => Some(a),
            _ => None,
        }
    }

    /// A numpad key as the system reports it: `key` is read without modifiers (so it is the
    /// NumLock-off meaning), `logical` has NumLock applied.
    fn numpad(code: keyboard::key::Code, key: keyboard::Key, logical: keyboard::Key, text: Option<&str>) -> Option<Action> {
        let physical = keyboard::key::Physical::Code(code);
        match translate_key(&key, &logical, physical, keyboard::Modifiers::empty(), text, false) {
            Key::Act(a) => Some(a),
            _ => None,
        }
    }

    #[test]
    fn numpad_digits_type_with_numlock_on_and_navigate_with_it_off() {
        use keyboard::key::{Code, Named};
        let named = |n| keyboard::Key::Named(n);
        // NumLock on: KP_1 / KP_2 / KP_4 / KP_0 / KP_. type their digit (the unmodified key
        // is End / ArrowDown / ArrowLeft / Insert / Delete either way).
        for (code, nav, digit) in [
            (Code::Numpad1, Named::End, "1"),
            (Code::Numpad2, Named::ArrowDown, "2"),
            (Code::Numpad3, Named::PageDown, "3"),
            (Code::Numpad4, Named::ArrowLeft, "4"),
            (Code::Numpad6, Named::ArrowRight, "6"),
            (Code::Numpad7, Named::Home, "7"),
            (Code::Numpad8, Named::ArrowUp, "8"),
            (Code::Numpad9, Named::PageUp, "9"),
            (Code::Numpad0, Named::Insert, "0"),
        ] {
            assert!(
                matches!(numpad(code, named(nav), k(digit), Some(digit)), Some(Action::Insert(t)) if t == digit),
                "numpad {digit} types with NumLock on"
            );
        }
        assert!(
            matches!(numpad(Code::NumpadDecimal, named(Named::Delete), k("."), Some(".")), Some(Action::Insert(t)) if t == "."),
            "the decimal key types a point with NumLock on"
        );
        // NumLock off: the same keys navigate.
        assert!(matches!(
            numpad(Code::Numpad1, named(Named::End), named(Named::End), None),
            Some(Action::Move { motion: Motion::LineEnd, extend: false })
        ));
        assert!(matches!(
            numpad(Code::Numpad4, named(Named::ArrowLeft), named(Named::ArrowLeft), None),
            Some(Action::Move { motion: Motion::Left, extend: false })
        ));
        assert!(matches!(numpad(Code::NumpadDecimal, named(Named::Delete), named(Named::Delete), None), Some(Action::Delete)));
        // The operator keys do not depend on NumLock.
        assert!(matches!(numpad(Code::NumpadAdd, k("+"), k("+"), Some("+")), Some(Action::Insert(t)) if t == "+"));
    }

    #[test]
    fn keys_map_to_editor_actions_and_app_shortcuts_pass_through() {
        use keyboard::{Modifiers as M, key::Named};
        let named = |n| keyboard::Key::Named(n);
        assert!(matches!(key(k("z"), M::CTRL, None), Some(Action::Undo)));
        assert!(matches!(key(k("z"), M::CTRL | M::SHIFT, None), Some(Action::Redo)));
        assert!(matches!(key(k("Z"), M::CTRL | M::SHIFT, None), Some(Action::Redo)), "upper-case letter with Shift");
        assert!(matches!(key(k("y"), M::CTRL, None), Some(Action::Redo)));
        assert!(matches!(key(k("a"), M::CTRL, None), Some(Action::SelectAll)));
        // Ctrl+S / Ctrl+P / Ctrl+B belong to the app: the editor must not eat them.
        for c in ["s", "p", "b", "w", "n"] {
            assert!(key(k(c), M::CTRL, Some(c)).is_none(), "ctrl+{c} must pass through");
        }
        assert!(matches!(key(k("a"), M::empty(), Some("a")), Some(Action::Insert(t)) if t == "a"));
        assert!(matches!(key(k("ğ"), M::empty(), Some("ğ")), Some(Action::Insert(t)) if t == "ğ"));
        assert!(
            matches!(key(k("q"), M::ALT, Some("@")), Some(Action::Insert(t)) if t == "@"),
            "AltGr-style characters are typed"
        );
        assert!(key(named(Named::F5), M::empty(), None).is_none());
        assert!(key(k("c"), M::empty(), Some("\u{3}")).is_none(), "control text is never inserted");
        assert!(matches!(key(named(Named::Enter), M::empty(), Some("\r")), Some(Action::Enter)));
        assert!(matches!(key(named(Named::Tab), M::empty(), None), Some(Action::Indent)));
        assert!(matches!(key(named(Named::Tab), M::SHIFT, None), Some(Action::Unindent)));
        assert!(key(named(Named::Tab), M::CTRL, None).is_none(), "Ctrl+Tab switches tabs (app)");
        assert!(matches!(key(named(Named::Backspace), M::CTRL, None), Some(Action::DeleteWordBack)));
        assert!(matches!(
            key(named(Named::ArrowLeft), M::CTRL, None),
            Some(Action::Move { motion: Motion::WordLeft, extend: false })
        ));
        assert!(matches!(
            key(named(Named::ArrowRight), M::SHIFT, None),
            Some(Action::Move { motion: Motion::Right, extend: true })
        ));
        assert!(matches!(
            key(named(Named::Home), M::CTRL | M::SHIFT, None),
            Some(Action::Move { motion: Motion::DocStart, extend: true })
        ));
        assert!(matches!(key(named(Named::Space), M::empty(), Some(" ")), Some(Action::Insert(t)) if t == " "));
    }
}
