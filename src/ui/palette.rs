//! Palette overlay: input + result list (Sublime-style, top-centered).

use iced::{
    Alignment, Background, Border, Element, Length, border,
    widget::{button, column, container, mouse_area, opaque, row, space, text, text_input},
};

use super::theme::{self, C};
use crate::{
    message::{FindFlag, Message},
    palette::{Mode, Palette, Target, VISIBLE},
};

/// Stable id so the app can focus the input when the palette opens.
pub const INPUT_ID: &str = "veloedit-palette-input";
const ROW_H: f32 = 28.0;
const WIDTH: f32 = 640.0;

pub fn view<'a>(c: C, p: &'a Palette, indexing: bool, no_roots: bool) -> Element<'a, Message> {
    let placeholder = match p.mode {
        Mode::Files => "Go to file...",
        Mode::Commands => "Type a command...",
        Mode::Search => "Find in files...",
    };
    let input = text_input(placeholder, &p.query)
        .id(INPUT_ID)
        .on_input(Message::PaletteQuery)
        .on_submit(Message::PaletteSubmit)
        .size(15)
        .padding([10, 14])
        .style(theme::input(c));

    let start = p.window_start();
    let mut list = column![].spacing(0);
    for (i, item) in p.items.iter().enumerate().skip(start).take(VISIBLE) {
        let selected = i == p.selected;
        let line = row![
            text(item.title.as_str())
                .size(theme::FONT_SIZE)
                .color(if selected { c.text_bright } else { c.text }),
            space::horizontal(),
            text(item.detail.as_str()).size(11).color(c.dim),
        ]
        .spacing(12)
        .height(Length::Fill)
        .align_y(Alignment::Center);
        list = list.push(
            button(line)
                .width(Length::Fill)
                .height(ROW_H)
                .padding([0, 12])
                .style(theme::flat(c, selected))
                .on_press(Message::PaletteClick(i)),
        );
    }
    if p.items.is_empty() {
        let note = match (p.mode, indexing, no_roots) {
            (Mode::Files, _, true) => "No folder open - add one with Ctrl+Shift+O",
            (Mode::Files, true, _) => "Indexing files...",
            _ => "No matches",
        };
        list = list.push(container(text(note).size(12).color(c.dim)).padding([8, 12]));
    }

    let toggle = |label: &'static str, on: bool, flag: FindFlag| -> Element<'a, Message> {
        button(text(label).size(12))
            .padding([4, 8])
            .style(theme::flat(c, on))
            .on_press(Message::PaletteToggle(flag))
            .into()
    };
    let head: Element<'a, Message> = if p.mode == Mode::Search {
        row![
            input,
            toggle("Aa", p.case_sensitive, FindFlag::Case),
            toggle("\\b", p.whole_word, FindFlag::Word),
            toggle(".*", p.regex, FindFlag::Regex),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
    } else {
        input.into()
    };
    let status: Element<'a, Message> = if p.status.is_empty() {
        space().height(0.0).into()
    } else {
        container(text(p.status.as_str()).size(11).color(c.dim)).padding([0, 12]).into()
    };
    let card = container(column![head, status, list].spacing(6))
        .padding(8)
        .width(WIDTH)
        .style(move |_| container::Style {
            background: Some(Background::Color(c.sidebar)),
            border: Border { color: c.border, width: 1.0, radius: border::Radius::from(8) },
            text_color: Some(c.text),
            ..container::Style::default()
        });

    // Transparent click-away layer; the card itself swallows clicks.
    mouse_area(
        container(opaque(card))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .padding(iced::Padding { top: 56.0, ..iced::Padding::ZERO }),
    )
    .on_press(Message::ClosePalette)
    .into()
}

/// What the Enter key / a click should do for the current selection.
pub fn target_of(p: &Palette, index: usize) -> Option<Target> {
    p.items.get(index).map(|i| i.target.clone())
}
