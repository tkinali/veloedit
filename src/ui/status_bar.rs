//! Bottom status bar.

use iced::{
    Alignment, Element, Length,
    widget::{button, container, row, space, text},
};

use super::theme::{self, C};
use crate::{
    editor::{Doc, Tab},
    message::Message,
};

pub fn view<'a>(c: C, active: Option<&Tab>, notice: Option<&'a str>) -> Element<'a, Message> {
    // The language of a text is a button: it opens the list to change it (View > Syntax).
    let mut language = None;
    let left = match active.map(|t| &t.doc) {
        Some(Doc::Text(d)) => {
            let (line, col) = d.caret();
            let n = d.state.line_count();
            language = Some(d.hl.lang.map_or("Plain Text", |l| l.name()));
            format!(
                "Ln {line}, Col {col}  \u{00B7}  {n} line{}  \u{00B7}  UTF-8  \u{00B7}  {}  \u{00B7}",
                if n == 1 { "" } else { "s" },
                d.state.eol().name(),
            )
        }
        Some(Doc::Hex(d)) => format!(
            "Hex  \u{00B7}  {}  \u{00B7}  offset 0x{:X} ({})  \u{00B7}  {}{}",
            human_size(d.len),
            d.cursor(),
            d.cursor(),
            if d.area() == crate::editor::hex::Area::Hex { "hex" } else { "text" },
            match d.edits().len() {
                0 => String::new(),
                n => format!("  \u{00B7}  {n} unsaved byte{}", if n == 1 { "" } else { "s" }),
            }
        ),
        Some(Doc::Image(d)) => format!(
            "{}  \u{00B7}  {}\u{00D7}{}  \u{00B7}  {}",
            d.format,
            d.width,
            d.height,
            human_size(d.bytes)
        ),
        None => String::new(),
    };

    container(
        row![
            text(left).size(12).color(c.dim),
            language.map_or_else(
                || Element::from(space()),
                |name| {
                    button(text(name).size(12).color(c.dim))
                        .padding([0, 6])
                        .style(theme::flat(c, false))
                        .on_press(Message::OpenSyntaxPicker)
                        .into()
                },
            ),
            space::horizontal(),
            text(notice.unwrap_or("")).size(12).color(c.dirty),
        ]
        .align_y(Alignment::Center),
    )
    .padding([4, 12])
    .width(Length::Fill)
    .style(theme::panel(c, c.status))
    .into()
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", UNITS[u]) }
}
