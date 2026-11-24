//! Left sidebar: multi-root folder tree (virtualized, fixed-height rows).

use std::path::Path;

use iced::{
    Alignment, Element, Font, Length,
    font::Weight,
    widget::{Column, button, column, container, mouse_area, row, scrollable, space, text, text::Wrapping},
    mouse,
};

use super::{
    file_badge,
    theme::{self, C},
    virtual_list,
};
use crate::{
    git::status::{Change, Tree},
    message::Message,
    workspace::{Row, Workspace},
};

pub const DEFAULT_WIDTH: f32 = 260.0;
pub const MIN_WIDTH: f32 = 140.0;
pub const MAX_WIDTH: f32 = 640.0;
/// Space kept free on the right of the rows for the scrollbar (it overlays the content).
const SCROLLBAR_GAP: f32 = 12.0;
/// Stable id: lets the app restore the scroll offset after the sidebar was hidden.
pub const SCROLL_ID: &str = "veloedit-sidebar-scroll";
pub const ROW_H: f32 = 24.0;
/// Viewport height assumed until the first scroll event reports the real one.
pub const DEFAULT_VIEWPORT_H: f32 = 1200.0;

#[derive(Clone, Copy)]
pub struct Scroll {
    pub y: f32,
    pub height: f32,
}

pub fn view<'a>(c: C, ws: &'a Workspace, repo: &Tree, active: Option<&Path>, scroll: Scroll, width: f32) -> Element<'a, Message> {
    let header = container(
        row![
            text("FOLDERS").size(11).color(c.dim),
            space::horizontal(),
            button(text("+").size(16))
                .padding([0, 8])
                .style(theme::flat(c, false))
                .on_press(Message::PickFolder),
        ]
        .align_y(Alignment::Center),
    )
    .padding([6, 8]);

    let body: Element<'a, Message> = if ws.is_empty() {
        container(
            column![
                text("No folder open").size(12).color(c.dim),
                button(text("Add Folder...").size(12))
                    .padding([5, 12])
                    .style(theme::secondary_button(c))
                    .on_press(Message::PickFolder),
            ]
            .spacing(10)
            .align_x(Alignment::Center),
        )
        .center(Length::Fill)
        .into()
    } else {
        tree(c, ws, repo, active, scroll)
    };

    let panel = container(column![header, body])
        .width(width.clamp(MIN_WIDTH, MAX_WIDTH))
        .height(Length::Fill)
        .style(theme::panel(c, c.sidebar));
    // A 1 px line inside a 5 px grab area: drag it to resize the sidebar.
    let line = container(space().width(1.0).height(Length::Fill)).style(theme::panel(c, c.border));
    let handle = mouse_area(
        container(line)
            .padding(iced::Padding { left: 2.0, right: 2.0, ..iced::Padding::ZERO })
            .width(5.0)
            .height(Length::Fill),
    )
    .on_press(Message::SidebarResizeStart)
    .interaction(mouse::Interaction::ResizingHorizontally);
    row![panel, handle].into()
}

fn tree<'a>(c: C, ws: &'a Workspace, repo: &Tree, active: Option<&Path>, scroll: Scroll) -> Element<'a, Message> {
    let rows = ws.rows();
    let w = virtual_list::window(rows.len(), ROW_H, scroll.y, scroll.height, 4);

    let mut col = Column::new().push(space().height(w.top));
    for r in &rows[w.start..w.end] {
        col = col.push(tree_row(c, r, repo.change(&r.node.entry.path), active));
    }
    col = col.push(space().height(w.bottom));

    scrollable(container(col.width(Length::Fill)).padding(iced::Padding { right: SCROLLBAR_GAP, ..iced::Padding::ZERO }))
        .id(SCROLL_ID)
        .height(Length::Fill)
        .on_scroll(|v| Message::SidebarScrolled {
            y: v.absolute_offset().y,
            height: v.bounds().height,
        })
        .into()
}

fn tree_row<'a>(c: C, r: &Row<'a>, change: Option<Change>, active: Option<&Path>) -> Element<'a, Message> {
    let node = r.node;
    let entry = &node.entry;
    let is_root = r.depth == 0;

    let arrow = match (entry.is_dir, node.expanded) {
        (true, true) => "\u{25BE}",
        (true, false) => "\u{25B8}",
        _ => "",
    };
    let font = if is_root {
        Font { weight: Weight::Bold, ..Font::DEFAULT }
    } else {
        Font::DEFAULT
    };
    // Changed since the last commit: yellow (edited), green (new), red (conflict).
    let tint = change.map(|ch| change_color(c, ch));
    let color = tint.unwrap_or(if entry.is_dir { c.text_bright } else { c.text });

    let mut label = row![space().width(r.depth as f32 * 12.0 + 4.0), text(arrow).size(10).width(12).color(c.dim)];
    if !entry.is_dir {
        // A short colored tag by file type ("PHP", "JS", "MD", ...).
        let (tag, tag_color) = file_badge::badge(&entry.name, c.is_light());
        label = label.push(
            container(text(tag).size(9).font(Font { weight: Weight::Bold, ..Font::DEFAULT }).color(tag_color))
                .width(28)
                .align_x(Alignment::End),
        );
    }
    // The name takes what is left and is cut at its end, so the letter on the right always shows.
    let name = container(text(entry.name.as_str()).size(theme::FONT_SIZE).font(font).color(color).wrapping(Wrapping::None))
        .width(Length::Fill)
        .clip(true);
    let mut label = label.push(name);
    if let (Some(ch), Some(tint)) = (change, tint) {
        // Files get a letter (M, U, A, !), folders a dot: "something inside changed".
        let mark = if entry.is_dir { "\u{25CF}" } else { ch.letter() };
        label = label.push(text(mark).size(if entry.is_dir { 8 } else { 10 }).font(Font { weight: Weight::Bold, ..Font::DEFAULT }).color(tint));
    }
    let label = label.spacing(4).width(Length::Fill).height(Length::Fill).align_y(Alignment::Center);

    let selected = !entry.is_dir && active == Some(entry.path.as_path());
    // Fixed-height rows: a long name is cut at the edge, never wrapped into the next row.
    let main = button(label)
        .width(Length::Fill)
        .height(ROW_H)
        .clip(true)
        .padding([0, 6])
        .style(theme::flat(c, selected))
        .on_press(Message::TreeClicked {
            path: entry.path.clone(),
            is_dir: entry.is_dir,
        });

    let content: Element<'a, Message> = if is_root {
        row![
            main,
            button(text("\u{00D7}").size(14).color(c.dim))
                .height(ROW_H)
                .padding([0, 8])
                .style(theme::flat(c, false))
                .on_press(Message::RemoveRoot(entry.path.clone())),
        ]
        .into()
    } else {
        main.into()
    };
    mouse_area(content)
        .on_right_press(Message::TreeContext { path: entry.path.clone(), is_dir: entry.is_dir, is_root })
        .into()
}

/// The color of a changed name in the sidebar (the same colors as the git marks in the gutter).
fn change_color(c: C, change: Change) -> iced::Color {
    match change {
        Change::Modified => c.dirty,
        Change::Untracked | Change::Added => c.hex_ascii,
        Change::Conflict => c.danger,
    }
}
