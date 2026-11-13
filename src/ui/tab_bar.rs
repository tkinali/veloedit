//! Tab strip above the editor area.

use iced::{
    Alignment, Background, Border, Element, Length, Padding,
    mouse,
    widget::{Row, button, column, container, mouse_area, row, scrollable, space, stack, text, text::Wrapping},
};

use super::theme::{self, C};
use crate::{
    editor::{Doc, Pane, TAB_LABEL, Tab, TabId, TabSet},
    message::Message,
};

/// Stable id: lets the app scroll the strip (wheel, "reveal the active tab").
pub const SCROLL_ID: &str = "veloedit-tabs-scroll";
/// Preview tabs are set in italics.
const PREVIEW_FONT: iced::Font = iced::Font { style: iced::font::Style::Italic, ..iced::Font::DEFAULT };
const MIN_W: f32 = 100.0;
const MAX_W: f32 = 240.0;

/// Width of a tab, estimated from its title. Tabs have a *fixed* width so that
/// the app can compute where each one sits and scroll the active one into view.
fn tab_width(t: &Tab) -> f32 {
    let badge = if matches!(t.doc, Doc::Text(_)) { 0.0 } else { 30.0 };
    (t.label(TAB_LABEL).chars().count() as f32 * 7.4 + 44.0 + badge).clamp(MIN_W, MAX_W)
}

/// `(left, right)` of tab `id` inside the strip's content.
pub fn span_of(tabs: &TabSet, id: TabId) -> Option<(f32, f32)> {
    let mut x = 0.0;
    for t in tabs.iter() {
        let w = tab_width(t);
        if t.id == id {
            return Some((x, x + w));
        }
        x += w;
    }
    None
}

/// Which gap between tabs a pointer at `x` (in the strip's content) is nearest to:
/// `0` is before the first tab, `tabs.len()` after the last.
pub fn slot_at(tabs: &TabSet, x: f32) -> usize {
    let mut left = 0.0;
    let mut slot = 0;
    for t in tabs.iter() {
        let w = tab_width(t);
        if left + w / 2.0 < x {
            slot += 1;
        }
        left += w;
    }
    slot
}

/// A tab that is being dragged to a new place: where it would land (`None` = nowhere).
#[derive(Clone, Copy)]
pub struct Drag {
    pub id: TabId,
    pub slot: Option<usize>,
}

/// Wheel movement in pixels along the strip (down/right = towards later tabs).
pub fn wheel_px(delta: mouse::ScrollDelta) -> f32 {
    match delta {
        mouse::ScrollDelta::Lines { x, y } => -(if y != 0.0 { y } else { x }) * 60.0,
        mouse::ScrollDelta::Pixels { x, y } => -(if y != 0.0 { y } else { x }),
    }
}

pub fn view(c: C, tabs: &TabSet, drag: Option<Drag>) -> Element<'_, Message> {
    let active = tabs.active_id();
    let mut strip = Row::new();
    for t in tabs.iter() {
        strip = strip.push(tab(c, t, Some(t.id) == active, drag.is_some_and(|d| d.id == t.id)));
    }
    // The insertion mark of a drag. The layer is always there (empty when nothing is
    // dragged) so the widget tree keeps its shape and no widget state is reset.
    let mark: Element<'_, Message> = match drag.and_then(|d| d.slot) {
        Some(slot) => {
            let x: f32 = tabs.iter().take(slot).map(tab_width).sum();
            container(container(space().width(2.0).height(Length::Fill)).style(theme::panel(c, c.accent)))
                .padding(Padding { left: (x - 1.0).max(0.0), ..Padding::ZERO })
                .height(Length::Fill)
                .into()
        }
        None => space().width(0.0).height(0.0).into(),
    };
    let strip = stack![strip, mark];

    // A vertical wheel does nothing to a horizontal scrollable (and the scrollable swallows
    // the event), so the wheel is caught *inside* it and turned into a sideways scroll.
    let strip = mouse_area(strip).on_scroll(|d| Message::TabsWheel(wheel_px(d)));
    let strip = scrollable(strip)
        .id(SCROLL_ID)
        .width(Length::Fill)
        .direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::new().width(0).scroller_width(0)))
        .on_scroll(|v| Message::TabsViewport {
            x: v.absolute_offset().x,
            width: v.bounds().width,
            content: v.content_bounds().width,
        });

    let mut right = row![button(text("+").size(16).color(c.dim))
        .padding([4, 10])
        .style(theme::flat(c, false))
        .on_press(Message::NewFile)]
    .align_y(Alignment::Center);
    if !tabs.is_empty() {
        right = right.push(
            button(text("\u{25BE}").size(13).color(c.dim))
                .padding([5, 10])
                .style(theme::flat(c, false))
                .on_press(Message::OpenTabList),
        );
    }

    container(row![strip, right].align_y(Alignment::Center))
        .width(Length::Fill)
        .style(theme::panel(c, c.tabbar))
        .into()
}

fn tab(c: C, t: &Tab, active: bool, dragged: bool) -> Element<'_, Message> {
    let badge = match t.doc {
        Doc::Text(_) => None,
        Doc::Hex(_) => Some("HEX"),
        Doc::Image(_) => Some("IMG"),
    };
    let dirty = t.is_dirty();

    let mut label = Row::new().spacing(6).align_y(Alignment::Center);
    if let Some(b) = badge {
        label = label.push(text(b).size(9).color(c.accent));
    }
    label = label.push(
        text(t.label(TAB_LABEL).into_owned())
            .size(theme::FONT_SIZE)
            .wrapping(Wrapping::None)
            .font(if t.preview { PREVIEW_FONT } else { iced::Font::DEFAULT })
            .color(if active && !dragged { c.text_bright } else { c.dim }),
    );

    // The press goes to the tab on mouse-*down* (it activates the tab and may start a drag);
    // the button around it only supplies the hover look, its own click never fires because
    // the inner area captures the press.
    let title = button(mouse_area(container(label).padding([7, 4]).width(Length::Fill)).on_press(Message::TabPressed(t.id)))
        .width(Length::Fill)
        .clip(true)
        .padding(0)
        .style(theme::flat(c, false))
        .on_press(Message::Noop);

    let (glyph, size, color) = if t.has_conflict() {
        ("!", 14, c.danger)
    } else if dirty {
        ("\u{25CF}", 10, c.dirty)
    } else {
        ("\u{00D7}", 15, c.dim)
    };
    let close = button(text(glyph).size(size).color(color))
        .padding([4, 6])
        .style(theme::flat(c, false))
        .on_press(Message::CloseTab(t.id));

    let accent = container(space().height(2.0))
        .width(Length::Fill)
        .style(theme::panel(c, if active { c.accent } else { iced::Color::TRANSPARENT }));

    let body = container(row![title, close].align_y(Alignment::Center))
        .padding([0, 4])
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Background::Color(if dragged { c.hover } else if active { c.bg } else { c.tabbar })),
            border: Border::default(),
            ..container::Style::default()
        });

    mouse_area(column![accent, body].width(tab_width(t)))
        .on_middle_press(Message::CloseTab(t.id))
        .on_right_press(Message::TabContext(t.id))
        .into()
}

/// Height of the name bar above each pane of a split tab.
pub const PANE_HEADER_H: f32 = 26.0;

/// The name bar of one pane of a split tab (like an editor group's tab in VS Code): the
/// file's name, a dirty mark, and a × that closes the pane. The focused pane's is lit.
pub fn pane_header(c: C, t: &Tab, pane: Pane, focused: bool) -> Element<'_, Message> {
    let mut name = Row::new().spacing(6).align_y(Alignment::Center).push(
        text(t.label(TAB_LABEL).into_owned())
            .size(theme::FONT_SIZE)
            .wrapping(Wrapping::None)
            .color(if focused { c.text_bright } else { c.dim }),
    );
    if t.is_dirty() {
        name = name.push(text("\u{25CF}").size(10).color(c.dirty));
    }
    let close = button(text("\u{00D7}").size(15).color(c.dim))
        .padding([0, 6])
        .style(theme::flat(c, false))
        .on_press(Message::ClosePane(t.id, pane));
    let accent = container(space().height(2.0))
        .width(Length::Fill)
        .style(theme::panel(c, if focused { c.accent } else { iced::Color::TRANSPARENT }));
    let chip = container(row![container(name).padding([0, 10]), close].align_y(Alignment::Center).height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Background::Color(if focused { c.bg } else { c.tabbar })),
            ..container::Style::default()
        });
    let chip = column![accent, chip].width(tab_width(t).max(MIN_W + 20.0));
    mouse_area(
        container(row![chip, space::horizontal()])
            .width(Length::Fill)
            .height(PANE_HEADER_H)
            .style(theme::panel(c, c.tabbar)),
    )
    .on_press(Message::FocusPane(t.id, pane))
    .on_right_press(Message::TabContext(t.id))
    .into()
}
