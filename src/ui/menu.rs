//! Popup menus: the sidebar's context menu and the menu bar's drop-downs.
//! Both are a list of [`Entry`]s drawn as a small card at a given position.

use iced::{
    Alignment, Background, Border, Element, Length, Padding,
    widget::{button, column, container, mouse_area, opaque, responsive, row, scrollable, space, text},
};

use super::theme::{self, C};
use crate::message::Message;

pub const ITEM_H: f32 = 26.0;
pub const SEP_H: f32 = 9.0;
/// Default card width.
pub const WIDTH: f32 = 244.0;

#[derive(Clone)]
pub enum Entry {
    Item { label: String, hint: std::borrow::Cow<'static, str>, msg: Message, enabled: bool, checked: Option<bool> },
    Sep,
}

impl Entry {
    pub fn item(label: impl Into<String>, hint: &'static str, msg: Message) -> Self {
        Entry::Item { label: label.into(), hint: hint.into(), msg, enabled: true, checked: None }
    }

    /// An entry with a computed hint (a folder name, ...) and an optional check mark.
    pub fn with_hint(label: impl Into<String>, hint: String, msg: Message, checked: Option<bool>) -> Self {
        Entry::Item { label: label.into(), hint: hint.into(), msg, enabled: true, checked }
    }

    /// A toggle entry with a check mark in front.
    pub fn toggle(label: impl Into<String>, on: bool, msg: Message) -> Self {
        Entry::Item { label: label.into(), hint: "".into(), msg, enabled: true, checked: Some(on) }
    }

    pub fn when(self, enabled: bool) -> Self {
        match self {
            Entry::Item { label, hint, msg, checked, .. } => Entry::Item { label, hint, msg, enabled, checked },
            sep => sep,
        }
    }
}

/// Height of a finished menu (for keeping it inside the window).
pub fn height(entries: &[Entry]) -> f32 {
    entries.iter().map(|e| if matches!(e, Entry::Sep) { SEP_H } else { ITEM_H }).sum::<f32>() + 8.0
}

fn card<'a>(c: C, entries: &[Entry], width: f32, max_h: f32) -> Element<'a, Message> {
    let mut col = column![].spacing(0).width(width);
    for e in entries {
        let el: Element<'a, Message> = match e {
            Entry::Sep => container(
                container(space().height(1.0).width(Length::Fill)).style(move |_| container::Style {
                    background: Some(Background::Color(c.border)),
                    ..container::Style::default()
                }),
            )
            .padding(Padding { top: 4.0, bottom: 4.0, left: 8.0, right: 8.0 })
            .height(SEP_H)
            .into(),
            Entry::Item { label, hint, msg, enabled, checked } => {
                let color = if *enabled { c.text } else { c.dim };
                let mark = match checked {
                    Some(true) => "\u{2713}",
                    _ => "",
                };
                let line = row![
                    text(mark).size(12).width(16).color(c.accent),
                    text(label.clone()).size(theme::FONT_SIZE).color(color).wrapping(text::Wrapping::None),
                    space::horizontal(),
                    text(hint.to_string()).size(11).color(c.dim).wrapping(text::Wrapping::None),
                ]
                .spacing(4)
                .align_y(Alignment::Center)
                .height(Length::Fill);
                let mut b = button(line)
                    .width(Length::Fill)
                    .height(ITEM_H)
                    .padding([0, 10])
                    .clip(true)
                    .style(theme::flat(c, false));
                if *enabled {
                    b = b.on_press(Message::MenuPick(Box::new(msg.clone())));
                }
                b.into()
            }
        };
        col = col.push(el);
    }
    // A list taller than the window scrolls instead of running off the screen.
    let body: Element<'a, Message> = if height(entries) > max_h {
        scrollable(col).height(Length::Fixed((max_h - 8.0).max(ITEM_H))).into()
    } else {
        col.into()
    };
    container(body)
        .padding(4)
        .style(move |_| container::Style {
            background: Some(Background::Color(c.sidebar)),
            border: Border { color: c.border, width: 1.0, radius: 6.0.into() },
            text_color: Some(c.text),
            ..container::Style::default()
        })
        .into()
}

/// The menu at window position `at` (clamped to stay fully visible). The layer
/// around it closes the menu when clicked; it starts `layer_top` px below the
/// window top so the menu bar stays clickable (hover/switch between menus).
pub fn popup<'a>(c: C, entries: Vec<Entry>, at: iced::Point, layer_top: f32, width: f32) -> Element<'a, Message> {
    let region = responsive(move |size| {
        let h = height(&entries).min(size.height - 8.0);
        let x = at.x.min(size.width - width - 8.0).max(4.0);
        let y = (at.y - layer_top).min(size.height - h - 8.0).max(0.0);
        let menu = container(opaque(card(c, &entries, width, size.height - y - 8.0)))
            .padding(Padding { left: x, top: y, right: 0.0, bottom: 0.0 });
        mouse_area(container(menu).width(Length::Fill).height(Length::Fill))
            .on_press(Message::CloseMenu)
            .on_right_press(Message::CloseMenu)
            .into()
    });
    container(region).padding(Padding { top: layer_top, ..Padding::ZERO }).width(Length::Fill).height(Length::Fill).into()
}
