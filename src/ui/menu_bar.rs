//! The menu bar along the top of the window. Only the labels live here; the
//! entries of each drop-down are built by the app (they depend on its state).

use iced::{
    Alignment, Element, Length,
    widget::{button, container, mouse_area, row, text},
};

use super::theme::{self, C};
use crate::message::Message;

pub const HEIGHT: f32 = 26.0;

/// `(label, width)`: widths are fixed so the app knows where to drop each menu.
pub const MENUS: [(&str, f32); 6] =
    [("File", 46.0), ("Edit", 46.0), ("View", 50.0), ("Go", 38.0), ("Preferences", 94.0), ("Help", 46.0)];

/// Left edge of menu `i`.
pub fn x_of(i: usize) -> f32 {
    4.0 + MENUS.iter().take(i).map(|(_, w)| w).sum::<f32>()
}

pub fn view<'a>(c: C, open: Option<usize>) -> Element<'a, Message> {
    let mut bar = row![].spacing(0).padding([0, 4]).align_y(Alignment::Center);
    for (i, (label, w)) in MENUS.iter().enumerate() {
        let b = button(container(text(*label).size(12.5)).center_x(Length::Fill))
            .width(*w)
            .height(HEIGHT - 4.0)
            .padding(0)
            .style(theme::flat(c, open == Some(i)))
            .on_press(Message::OpenMenu(i));
        bar = bar.push(mouse_area(b).on_enter(Message::MenuHover(i)));
    }
    container(bar)
        .width(Length::Fill)
        .height(HEIGHT)
        .center_y(HEIGHT)
        .style(theme::panel(c, c.tabbar))
        .into()
}
