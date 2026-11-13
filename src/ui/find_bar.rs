//! The find / replace bar shown above the editor.

use iced::{
    Alignment, Element, Length,
    widget::{button, column, container, row, space, text, text_input},
};

use super::theme::{self, C};
use crate::{
    find::Find,
    message::{FindFlag, Message},
};

pub const FIND_INPUT_ID: &str = "veloedit-find-input";
pub const REPLACE_INPUT_ID: &str = "veloedit-replace-input";

fn toggle<'a>(c: C, label: &'static str, on: bool, flag: FindFlag) -> Element<'a, Message> {
    button(text(label).size(12))
        .padding([3, 7])
        .style(theme::flat(c, on))
        .on_press(Message::FindToggle(flag))
        .into()
}

fn icon_button<'a>(c: C, label: &'static str, msg: Message) -> Element<'a, Message> {
    button(text(label).size(13))
        .padding([3, 8])
        .style(theme::flat(c, false))
        .on_press(msg)
        .into()
}

/// `status`: "3 of 12", "No results", ... shown right of the toggles.
pub fn view<'a>(c: C, f: &'a Find, status: String) -> Element<'a, Message> {
    let status_color = if f.error.is_some() { c.danger } else { c.dim };
    let status = f.error.clone().unwrap_or(status);

    let mut rows = column![
        row![
            text_input("Find", &f.query.text)
                .id(FIND_INPUT_ID)
                .on_input(Message::FindQuery)
                .on_submit(Message::FindSubmit)
                .size(13)
                .padding([5, 8])
                .style(theme::input(c)),
            toggle(c, "Aa", f.query.case_sensitive, FindFlag::Case),
            toggle(c, "\\b", f.query.whole_word, FindFlag::Word),
            toggle(c, ".*", f.query.regex, FindFlag::Regex),
            icon_button(c, "\u{2191}", Message::FindPrev),
            icon_button(c, "\u{2193}", Message::FindNext),
            container(text(status).size(12).color(status_color)).width(130).padding([0, 6]),
            icon_button(c, "\u{00D7}", Message::CloseFind),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    ]
    .spacing(4);

    if f.replace {
        rows = rows.push(
            row![
                text_input("Replace", &f.replacement)
                    .id(REPLACE_INPUT_ID)
                    .on_input(Message::FindReplacement)
                    .on_submit(Message::ReplaceOne)
                    .size(13)
                    .padding([5, 8])
                    .style(theme::input(c)),
                button(text("Replace").size(12))
                    .padding([4, 10])
                    .style(theme::secondary_button(c))
                    .on_press(Message::ReplaceOne),
                button(text("Replace All").size(12))
                    .padding([4, 10])
                    .style(theme::secondary_button(c))
                    .on_press(Message::ReplaceAll),
                space::horizontal().width(Length::Fixed(0.0)),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        );
    }

    container(rows)
        .padding([6, 10])
        .width(Length::Fill)
        .style(theme::bordered(c, c.sidebar))
        .into()
}
