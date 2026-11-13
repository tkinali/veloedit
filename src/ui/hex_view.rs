//! The hex editor widget: offset | hex bytes | ASCII, with a byte cursor,
//! in-place editing and its own row-based scrolling (so a multi-gigabyte file
//! scrolls exactly: no pixel-height cap, no scrollable involved).

use iced::{
    Border, Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Shadow, Size, Theme,
    advanced::{
        Clipboard, Layout, Renderer as _, Shell, Widget, layout, mouse,
        renderer::{self, Quad},
        text::{self, LineHeight, Renderer as _, Shaping, Text, Wrapping},
        widget::{self, Operation, Tree, operation::Focusable, tree},
    },
    alignment, keyboard,
};

use super::{editor_view::measure_char_w, theme::C};
use crate::editor::hex::{Area, BYTES_PER_ROW, HexAction, HexDoc, Move};

type Renderer = iced::Renderer;

const PAD: f32 = 14.0;
const BAR_W: f32 = 12.0;

pub struct HexView<'a, Message> {
    id: Option<widget::Id>,
    doc: &'a HexDoc,
    colors: C,
    font_size: f32,
    modal: bool,
    on_action: Box<dyn Fn(HexAction) -> Message + 'a>,
}

impl<'a, Message> HexView<'a, Message> {
    pub fn new(doc: &'a HexDoc, colors: C, font_size: f32, on_action: impl Fn(HexAction) -> Message + 'a) -> Self {
        Self { id: None, doc, colors, font_size, modal: false, on_action: Box::new(on_action) }
    }

    pub fn id(mut self, id: impl Into<widget::Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    fn line_h(&self) -> f32 {
        (self.font_size * 1.4).round()
    }
}

// ------------------------------------------------------------------ layout

/// Character-cell columns of the three areas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cells {
    pub offset_digits: usize,
}

impl Cells {
    /// Column where byte `i`'s two hex digits start (a gap after byte 7).
    pub fn hex(self, i: usize) -> usize {
        self.offset_digits + 2 + 3 * i + i / 8
    }

    /// Column where the ASCII area starts.
    pub fn ascii(self) -> usize {
        self.hex(15) + 2 + 3
    }

    /// The byte (and area) under character cell `cell`, if any.
    pub fn hit(self, cell: usize) -> Option<(usize, Area)> {
        (0..16)
            .find(|&i| (self.hex(i)..self.hex(i) + 2).contains(&cell) || cell == self.hex(i) + 2 && i % 8 != 7 && i != 15)
            .map(|i| (i, Area::Hex))
            .or_else(|| (self.ascii()..self.ascii() + 16).contains(&cell).then(|| (cell - self.ascii(), Area::Ascii)))
    }
}

struct Geo {
    origin_x: f32,
    body_y: f32,
    rows: u64,
    bar: Rectangle,
}

fn geometry(doc: &HexDoc, bounds: Rectangle, line_h: f32) -> Geo {
    let body_y = bounds.y + line_h + 6.0;
    let rows = (((bounds.y + bounds.height - body_y) / line_h).floor() as u64).max(1);
    Geo {
        origin_x: bounds.x + PAD,
        body_y,
        rows,
        bar: Rectangle { x: bounds.x + bounds.width - BAR_W, y: body_y, width: BAR_W, height: (bounds.y + bounds.height - body_y).max(0.0) },
    }
    .with(doc)
}

impl Geo {
    fn with(self, _doc: &HexDoc) -> Self {
        self
    }
}

// ------------------------------------------------------------- widget state

#[derive(Default)]
struct State {
    focused: bool,
    dragging_bar: bool,
    modifiers: keyboard::Modifiers,
    char_w: Option<(f32, f32)>,
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

fn translate_key(
    key: &keyboard::Key,
    logical: &keyboard::Key,
    physical: keyboard::key::Physical,
    mods: keyboard::Modifiers,
    text: Option<&str>,
    area: Area,
) -> Option<HexAction> {
    use keyboard::{Key as K, key::Named};
    let ctrl = mods.command();
    let key = super::effective_key(key, logical, physical);
    let mv = |m| Some(HexAction::Move(m));
    if let K::Named(n) = key.as_ref() {
        match n {
            Named::ArrowLeft => return mv(Move::Left),
            Named::ArrowRight => return mv(Move::Right),
            Named::ArrowUp => return mv(Move::Up),
            Named::ArrowDown => return mv(Move::Down),
            Named::PageUp if !ctrl => return mv(Move::PageUp),
            Named::PageDown if !ctrl => return mv(Move::PageDown),
            Named::Home => return mv(if ctrl { Move::Start } else { Move::RowStart }),
            Named::End => return mv(if ctrl { Move::End } else { Move::RowEnd }),
            Named::Tab if !ctrl => return Some(HexAction::ToggleArea),
            _ => {}
        }
    }
    if ctrl {
        return match key.to_latin(physical).map(|c| c.to_ascii_lowercase()) {
            Some('z') if mods.shift() => Some(HexAction::Redo),
            Some('z') => Some(HexAction::Undo),
            Some('y') => Some(HexAction::Redo),
            _ => None, // Ctrl+S & co belong to the app
        };
    }
    let c = text?.chars().next().filter(|c| !c.is_control())?;
    match area {
        Area::Hex => c.to_digit(16).map(|d| HexAction::Digit(d as u8)),
        Area::Ascii => u8::try_from(c as u32).ok().filter(|b| (0x20..0x7F).contains(b)).map(HexAction::Byte),
    }
}

impl<Message> Widget<Message, Theme, Renderer> for HexView<'_, Message> {
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
        let _ = char_w;
        let g = geometry(self.doc, Rectangle::new(Point::ORIGIN, size), self.line_h());
        self.doc.set_view_rows(g.rows);
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
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let st = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        let Some((_, char_w)) = st.char_w else { return };
        let line_h = self.line_h();
        let g = geometry(self.doc, bounds, line_h);
        let cells = Cells { offset_digits: self.doc.offset_digits() };
        let publish = |shell: &mut Shell<'_, Message>, a: HexAction| {
            shell.publish((self.on_action)(a));
            shell.capture_event();
        };
        let bar_row = |y: f32, doc: &HexDoc| -> f64 {
            let frac = ((y - g.bar.y) / g.bar.height.max(1.0)).clamp(0.0, 1.0) as f64;
            frac * doc.rows as f64
        };

        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => st.modifiers = *m,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(pos) = cursor.position_in(bounds) else {
                    st.focused = false;
                    return;
                };
                if self.modal {
                    return;
                }
                st.focused = true;
                let abs = Point::new(pos.x + bounds.x, pos.y + bounds.y);
                if g.bar.contains(abs) {
                    st.dragging_bar = true;
                    publish(shell, HexAction::ScrollTo { row: bar_row(abs.y, self.doc) });
                    return;
                }
                if abs.y < g.body_y {
                    return;
                }
                let row = self.doc.top_row() + ((abs.y - g.body_y) / line_h).floor() as u64;
                let cell = ((abs.x - g.origin_x) / char_w).floor();
                if cell < 0.0 {
                    return;
                }
                if let Some((i, area)) = cells.hit(cell as usize) {
                    publish(shell, HexAction::Click { offset: row * BYTES_PER_ROW + i as u64, area });
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) if st.dragging_bar => {
                publish(shell, HexAction::ScrollTo { row: bar_row(position.y, self.doc) });
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => st.dragging_bar = false,
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) && !self.modal => {
                let rows = match *delta {
                    mouse::ScrollDelta::Lines { y, .. } => -f64::from(y) * 3.0,
                    mouse::ScrollDelta::Pixels { y, .. } => -f64::from(y / line_h),
                };
                publish(shell, HexAction::Scroll { rows });
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key, modified_key, physical_key, modifiers, text, .. })
                if st.focused && !self.modal =>
            {
                st.modifiers = *modifiers;
                if let Some(a) = translate_key(key, modified_key, *physical_key, *modifiers, text.as_deref(), self.doc.area()) {
                    publish(shell, a);
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
        let g = geometry(self.doc, layout.bounds(), self.line_h());
        if st.dragging_bar || cursor.is_over(g.bar) {
            mouse::Interaction::Pointer
        } else if cursor.is_over(layout.bounds()) && !self.modal {
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
        let Some((_, char_w)) = st.char_w else { return };
        let c = self.colors;
        let line_h = self.line_h();
        let g = geometry(self.doc, bounds, line_h);
        let d = self.doc;
        let cells = Cells { offset_digits: d.offset_digits() };
        let digits = cells.offset_digits;
        let size = Pixels(self.font_size);
        let x_of = |cell: usize| g.origin_x + cell as f32 * char_w;

        let quad = |r: &mut Renderer, rect: Rectangle, color: Color| {
            r.fill_quad(Quad { bounds: rect, border: Border::default(), shadow: Shadow::default(), snap: true }, color);
        };
        let text = |r: &mut Renderer, s: String, x: f32, y: f32, color: Color| {
            let w = s.chars().count() as f32 * char_w + char_w * 2.0;
            r.fill_text(
                Text {
                    content: s,
                    bounds: Size::new(w, line_h),
                    size,
                    line_height: LineHeight::Absolute(Pixels(line_h)),
                    font: Font::MONOSPACE,
                    align_x: text::Alignment::Left,
                    align_y: alignment::Vertical::Top,
                    shaping: Shaping::Basic,
                    wrapping: Wrapping::None,
                },
                Point::new(x, y),
                color,
                bounds,
            );
        };

        renderer.with_layer(bounds, |r| {
            quad(r, bounds, c.bg);

            // Header.
            let head_y = bounds.y + 3.0;
            let cols: Vec<String> = (0..16).map(|i| format!("{i:02X}")).collect();
            text(r, format!("{:<digits$}", "Offset"), x_of(0), head_y, c.dim);
            for (i, label) in cols.into_iter().enumerate() {
                text(r, label, x_of(cells.hex(i)), head_y, c.dim);
            }
            text(r, "Decoded text".to_owned(), x_of(cells.ascii()), head_y, c.dim);
            quad(r, Rectangle { x: bounds.x, y: g.body_y - 3.0, width: bounds.width, height: 1.0 }, c.border);

            let top = d.top_row();
            let cursor_row = d.cursor() / BYTES_PER_ROW;
            for k in 0..=g.rows {
                let row = top + k;
                if row >= d.rows {
                    break;
                }
                let y = g.body_y + k as f32 * line_h;
                if row == cursor_row {
                    quad(r, Rectangle { x: bounds.x, y, width: bounds.width - BAR_W, height: line_h }, Color { a: 0.35, ..c.hover });
                }
                let base = row * BYTES_PER_ROW;
                text(r, format!("{base:0digits$X}"), x_of(0), y, c.hex_addr);

                let (mut hex, mut ascii) = (String::new(), String::new());
                let mut edited: Vec<(usize, u8)> = Vec::new();
                for i in 0..16usize {
                    if i == 8 {
                        hex.push(' ');
                    }
                    let off = base + i as u64;
                    match d.byte(off) {
                        Some(b) if d.is_edited(off) => {
                            edited.push((i, b));
                            hex.push_str("   ");
                            ascii.push(' ');
                        }
                        Some(b) => {
                            hex.push_str(&format!("{b:02X} "));
                            ascii.push(if (0x20..0x7F).contains(&b) { b as char } else { '.' });
                        }
                        None if off < d.len => {
                            hex.push_str("?? ");
                            ascii.push('?');
                        }
                        None => {
                            hex.push_str("   ");
                            ascii.push(' ');
                        }
                    }
                }
                text(r, hex, x_of(cells.hex(0)), y, c.text);
                text(r, ascii, x_of(cells.ascii()), y, c.hex_ascii);
                for (i, b) in edited {
                    text(r, format!("{b:02X}"), x_of(cells.hex(i)), y, c.dirty);
                    text(r, if (0x20..0x7F).contains(&b) { (b as char).to_string() } else { ".".to_owned() }, x_of(cells.ascii() + i), y, c.dirty);
                }
            }

            // Cursor: filled in the active area, outlined in the other.
            let cur = d.cursor();
            if d.len > 0 && (top..top + g.rows + 1).contains(&(cur / BYTES_PER_ROW)) {
                let i = (cur % BYTES_PER_ROW) as usize;
                let y = g.body_y + (cur / BYTES_PER_ROW - top) as f32 * line_h;
                let regions = [
                    (Area::Hex, Rectangle { x: x_of(cells.hex(i)) - 2.0, y, width: 2.0 * char_w + 4.0, height: line_h }),
                    (Area::Ascii, Rectangle { x: x_of(cells.ascii() + i), y, width: char_w, height: line_h }),
                ];
                for (area, rect) in regions {
                    let active = area == d.area();
                    r.fill_quad(
                        Quad {
                            bounds: rect,
                            border: Border { color: c.accent, width: 1.0, radius: 2.0.into() },
                            shadow: Shadow::default(),
                            snap: true,
                        },
                        if active && st.focused { Color { a: 0.45, ..c.accent } } else { Color::TRANSPARENT },
                    );
                }
                if st.focused && d.area() == Area::Hex && !d.high_next() {
                    // Waiting for the low nibble: underline its half.
                    quad(r, Rectangle { x: x_of(cells.hex(i)) + char_w, y: y + line_h - 2.0, width: char_w, height: 2.0 }, c.text_bright);
                }
            }

            // Scrollbar.
            if d.rows > 0 && g.bar.height > 0.0 {
                quad(r, g.bar, c.bg);
                let frac_visible = (g.rows as f32 / d.rows as f32).clamp(0.0, 1.0);
                let thumb_h = (g.bar.height * frac_visible).max(24.0).min(g.bar.height);
                let pos = (d.top_row() as f64 / d.rows as f64) as f32;
                let y = g.bar.y + pos * (g.bar.height - thumb_h);
                let thumb = Rectangle { x: g.bar.x + 3.0, y, width: g.bar.width - 6.0, height: thumb_h };
                let alpha = if st.dragging_bar { 0.6 } else if cursor.is_over(thumb) { 0.45 } else { 0.28 };
                r.fill_quad(
                    Quad {
                        bounds: thumb,
                        border: Border { radius: (thumb.width / 2.0).into(), ..Border::default() },
                        shadow: Shadow::default(),
                        snap: true,
                    },
                    Color { a: alpha, ..c.text },
                );
            }
        });
    }
}

impl<'a, Message: 'a> From<HexView<'a, Message>> for Element<'a, Message, Theme, Renderer> {
    fn from(view: HexView<'a, Message>) -> Self {
        Element::new(view)
    }
}

pub const HEX_ID: &str = "veloedit-hex";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_map_columns_to_bytes_and_areas() {
        let c = Cells { offset_digits: 8 };
        assert_eq!(c.hex(0), 10);
        assert_eq!(c.hex(8), 10 + 24 + 1, "an extra gap after byte 7");
        assert_eq!(c.ascii(), c.hex(15) + 5);
        assert_eq!(c.hit(10), Some((0, Area::Hex)));
        assert_eq!(c.hit(11), Some((0, Area::Hex)));
        assert_eq!(c.hit(12), Some((0, Area::Hex)), "the gap right after a byte belongs to it");
        assert_eq!(c.hit(13), Some((1, Area::Hex)));
        assert_eq!(c.hit(c.ascii() + 5), Some((5, Area::Ascii)));
        assert_eq!(c.hit(c.ascii() + 16), None);
        assert_eq!(c.hit(3), None, "offset column");
        // Wider offsets (files > 4 GiB) shift everything.
        assert_eq!(Cells { offset_digits: 16 }.hex(0), 18);
    }

    #[test]
    fn typing_maps_by_area() {
        let p = keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
        let k = |s: &str| keyboard::Key::Character(s.into());
        let none = keyboard::Modifiers::empty();
        let t = |s: &str, area| translate_key(&k(s), &k(s), p, none, Some(s), area);
        assert!(matches!(t("a", Area::Hex), Some(HexAction::Digit(10))));
        assert!(matches!(t("F", Area::Hex), Some(HexAction::Digit(15))));
        assert!(t("g", Area::Hex).is_none(), "not a hex digit");
        assert!(matches!(t("g", Area::Ascii), Some(HexAction::Byte(b'g'))));
        assert!(t("\u{11f}", Area::Ascii).is_none(), "non-ASCII cannot be one byte");
        assert!(translate_key(&k("s"), &k("s"), p, keyboard::Modifiers::CTRL, Some("s"), Area::Hex).is_none(), "Ctrl+S passes through");
        assert!(matches!(translate_key(&k("z"), &k("z"), p, keyboard::Modifiers::CTRL, None, Area::Hex), Some(HexAction::Undo)));
        let tab = keyboard::Key::Named(keyboard::key::Named::Tab);
        assert!(matches!(translate_key(&tab, &tab, p, none, None, Area::Hex), Some(HexAction::ToggleArea)));
    }

    #[test]
    fn numpad_digits_are_typed_into_the_hex_view_with_numlock_on() {
        use keyboard::key::{Code, Named, Physical};
        // NumLock on: `key` is still the NumLock-off key (KP_7 = Home), the logical key is "7".
        let (home, seven) = (keyboard::Key::Named(Named::Home), keyboard::Key::Character("7".into()));
        let pad7 = Physical::Code(Code::Numpad7);
        let none = keyboard::Modifiers::empty();
        assert!(matches!(translate_key(&home, &seven, pad7, none, Some("7"), Area::Hex), Some(HexAction::Digit(7))));
        // NumLock off: both say Home, nothing is typed.
        assert!(matches!(translate_key(&home, &home, pad7, none, None, Area::Hex), Some(HexAction::Move(Move::RowStart))));
    }
}
