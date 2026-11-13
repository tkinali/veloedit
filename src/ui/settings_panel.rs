//! The preferences overlay: theme, font size, tab size, word wrap, minimap, trim on save.

use iced::{
    Alignment, Background, Border, Color, Element, Length,
    widget::{button, column, container, mouse_area, opaque, row, space, text},
};

use super::theme::{self, C};
use crate::{message::{Message, Toggle}, settings::Settings};

fn label<'a>(c: C, s: &'static str) -> Element<'a, Message> {
    container(text(s).size(13).color(c.text)).width(110).into()
}

fn chip<'a>(c: C, s: String, on: bool, msg: Message) -> Element<'a, Message> {
    button(text(s).size(12))
        .padding([4, 12])
        .style(move |t, status| {
            let mut st = theme::flat(c, on)(t, status);
            if on {
                st.background = Some(Background::Color(c.selected));
                st.border = Border { color: c.accent, width: 1.0, radius: 4.0.into() };
            } else {
                st.border = Border { color: c.border, width: 1.0, radius: 4.0.into() };
            }
            st
        })
        .on_press(msg)
        .into()
}

fn stepper<'a>(c: C, value: String, minus: Message, plus: Message) -> Element<'a, Message> {
    row![
        chip(c, "\u{2212}".into(), false, minus),
        container(text(value).size(13).color(c.text_bright)).center_x(Length::Fixed(44.0)),
        chip(c, "+".into(), false, plus),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

pub fn view<'a>(c: C, s: &'a Settings) -> Element<'a, Message> {
    let themes = theme::ALL.iter().fold(row![].spacing(6), |r, t| {
        r.push(chip(c, t.name.to_owned(), s.colors().name == t.name, Message::SetTheme(t.name.to_owned())))
    });
    let on_off = |on: bool, toggle: Toggle| chip(c, if on { "On" } else { "Off" }.to_owned(), on, Message::ToggleSetting(toggle));

    let rows = column![
        text("Preferences").size(16).color(c.text_bright),
        space().height(6.0),
        row![label(c, "Theme"), themes].align_y(Alignment::Center),
        row![
            label(c, "Font size"),
            stepper(c, format!("{}", s.font_size() as i32), Message::ChangeFontSize(-1), Message::ChangeFontSize(1))
        ]
        .align_y(Alignment::Center),
        row![
            label(c, "Tab size"),
            stepper(c, s.tab_size().to_string(), Message::ChangeTabSize(-1), Message::ChangeTabSize(1))
        ]
        .align_y(Alignment::Center),
        row![label(c, "Word wrap"), on_off(s.word_wrap, Toggle::WordWrap)].align_y(Alignment::Center),
        row![label(c, "Minimap"), on_off(s.minimap, Toggle::Minimap)].align_y(Alignment::Center),
        row![
            label(c, "Trim on save"),
            row![
                on_off(s.trim_trailing_whitespace, Toggle::TrimOnSave),
                text("Remove spaces and tabs at line ends").size(11).color(c.dim),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        ]
        .align_y(Alignment::Center),
        space().height(8.0),
        row![
            button(text("Open settings.json").size(12))
                .padding([6, 12])
                .style(theme::secondary_button(c))
                .on_press(Message::OpenSettings),
            space::horizontal(),
            button(text("Close").size(12))
                .padding([6, 18])
                .style(theme::primary_button(c))
                .on_press(Message::Escape),
        ],
    ]
    .spacing(12);

    let card = container(rows).padding(22).width(440).style(move |_| container::Style {
        background: Some(Background::Color(c.sidebar)),
        border: Border { color: c.border, width: 1.0, radius: 8.0.into() },
        text_color: Some(c.text),
        ..container::Style::default()
    });

    mouse_area(
        container(opaque(card)).center(Length::Fill).style(|_| container::Style {
            background: Some(Background::Color(Color { a: 0.55, ..Color::BLACK })),
            ..container::Style::default()
        }),
    )
    .on_press(Message::Escape)
    .into()
}
