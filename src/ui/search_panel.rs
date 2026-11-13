//! Find in Files (Ctrl+Shift+F): query + options, file filter, folder scope and
//! the results grouped per file. Uses the palette's state (`Mode::Search`), but
//! has its own layout: results are a virtualised, scrollable list.

use iced::{
    Alignment, Background, Border, Element, Font, Length, border,
    font::Weight,
    widget::{
        Column, button, column, container, mouse_area, opaque, row, scrollable, space, text, text::Wrapping,
        text_input,
    },
};

use super::{
    palette::INPUT_ID,
    theme::{self, C},
    virtual_list,
};
use crate::{
    message::{FindFlag, Message},
    palette::{Palette, SearchRow},
    project_search::Hit,
};

pub const FILTER_ID: &str = "veloedit-search-filter";
pub const SCROLL_ID: &str = "veloedit-search-scroll";
pub const ROW_H: f32 = 24.0;
const MONO: Font = Font::MONOSPACE;

pub fn view<'a>(c: C, p: &'a Palette, no_roots: bool) -> Element<'a, Message> {
    let toggle = |label: &'static str, on: bool, flag: FindFlag| -> Element<'a, Message> {
        button(text(label).size(12))
            .padding([5, 9])
            .style(theme::flat(c, on))
            .on_press(Message::PaletteToggle(flag))
            .into()
    };

    let title = row![
        text("Find in Files").size(12).color(c.dim),
        space::horizontal(),
        button(text("\u{00D7}").size(15).color(c.dim))
            .padding([0, 8])
            .style(theme::flat(c, false))
            .on_press(Message::ClosePalette),
    ]
    .align_y(Alignment::Center);

    let query = row![
        text_input("Find", &p.query)
            .id(INPUT_ID)
            .on_input(Message::PaletteQuery)
            .on_submit(Message::PaletteSubmit)
            .size(15)
            .padding([8, 12])
            .style(theme::input(c)),
        toggle("Aa", p.case_sensitive, FindFlag::Case),
        toggle("\\b", p.whole_word, FindFlag::Word),
        toggle(".*", p.regex, FindFlag::Regex),
    ]
    .spacing(4)
    .align_y(Alignment::Center);

    let filter = text_input("Files:  *.php, *.js, !vendor   (empty = every file)", &p.filter)
        .id(FILTER_ID)
        .on_input(Message::SearchFilter)
        .on_submit(Message::PaletteSubmit)
        .size(13)
        .padding([6, 12])
        .style(theme::input(c));

    let scope_text = match &p.scope {
        Some(path) => format!("In: {}", shorten_left(&path.display().to_string(), 38)),
        None => "In: all folders".to_owned(),
    };
    let mut scope = row![
        container(text(scope_text).size(12).color(if p.scope.is_some() { c.text_bright } else { c.dim }))
            .padding([6, 10])
            .style(theme::bordered(c, c.bg)),
        button(text("Choose\u{2026}").size(12))
            .padding([5, 10])
            .style(theme::secondary_button(c))
            .on_press(Message::PickSearchScope),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    if p.scope.is_some() {
        scope = scope.push(
            button(text("All folders").size(12))
                .padding([5, 10])
                .style(theme::flat(c, false))
                .on_press(Message::SearchScope(None)),
        );
    }
    let options = row![filter, scope].spacing(8).align_y(Alignment::Center);

    let status_text = if no_roots && p.scope.is_none() {
        "No folder open \u{2013} add one with Ctrl+Shift+O".to_owned()
    } else {
        p.status.clone()
    };
    let status = container(text(status_text).size(11).color(c.dim)).padding([2, 4]);

    let results: Element<'a, Message> = if p.rows.is_empty() {
        space().height(0.0).into()
    } else {
        list(c, p)
    };

    let card = container(column![title, query, options, status, results].spacing(6))
        .padding(10)
        .width(Length::Fill)
        .max_width(1000)
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
            .padding(iced::Padding { top: 40.0, bottom: 40.0, left: 24.0, right: 24.0 }),
    )
    .on_press(Message::ClosePalette)
    .into()
}

fn list<'a>(c: C, p: &'a Palette) -> Element<'a, Message> {
    let Some(outcome) = &p.results else { return space().height(0.0).into() };
    let w = virtual_list::window(p.rows.len(), ROW_H, p.scroll_y, p.scroll_h, 6);
    let mut col = Column::new().push(space().height(w.top));
    for row in &p.rows[w.start..w.end] {
        col = col.push(match *row {
            SearchRow::File(first, count) => file_row(c, &outcome.hits[first], first, count),
            SearchRow::Hit(i) => hit_row(c, &outcome.hits[i], i, i == p.selected),
        });
    }
    col = col.push(space().height(w.bottom));

    scrollable(col.width(Length::Fill))
        .id(SCROLL_ID)
        .on_scroll(|v| Message::SearchScrolled { y: v.absolute_offset().y, height: v.bounds().height })
        .into()
}

fn file_row<'a>(c: C, first: &'a Hit, index: usize, count: usize) -> Element<'a, Message> {
    let (dir, name) = match first.rel.rsplit_once('/') {
        Some((d, n)) => (format!("{d}/"), n),
        None => (String::new(), first.rel.as_str()),
    };
    let bold = Font { weight: Weight::Bold, ..Font::DEFAULT };
    let line = row![
        text(name).size(theme::FONT_SIZE).font(bold).color(c.text_bright).wrapping(Wrapping::None),
        text(dir).size(12).color(c.dim).wrapping(Wrapping::None),
        space::horizontal(),
        text(count.to_string()).size(11).color(c.dim),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .height(Length::Fill);
    button(line)
        .width(Length::Fill)
        .height(ROW_H)
        .padding(iced::Padding { left: 10.0, right: 18.0, ..iced::Padding::ZERO })
        .clip(true)
        .style(move |t, status| {
            let mut s = theme::flat(c, false)(t, status);
            s.background = Some(Background::Color(c.hover));
            s
        })
        .on_press(Message::PaletteClick(index))
        .into()
}

fn hit_row<'a>(c: C, hit: &'a Hit, index: usize, selected: bool) -> Element<'a, Message> {
    let (pre, mid, post) = split_preview(hit);
    let mono = |s: &'a str| text(s).font(MONO).size(13).wrapping(Wrapping::None);
    let line = row![
        space().width(14.0),
        text((hit.line + 1).to_string())
            .font(MONO)
            .size(12)
            .color(c.dim)
            .width(46)
            .align_x(Alignment::End)
            .wrapping(Wrapping::None),
        space().width(10.0),
        mono(pre).color(c.text),
        mono(mid).color(c.accent),
        mono(post).color(c.text),
    ]
    .align_y(Alignment::Center)
    .height(Length::Fill);
    button(line)
        .width(Length::Fill)
        .height(ROW_H)
        .padding(iced::Padding { left: 10.0, right: 18.0, ..iced::Padding::ZERO })
        .clip(true)
        .style(theme::flat(c, selected))
        .on_press(Message::PaletteClick(index))
        .into()
}

/// The preview as (before the match, the match, after it).
pub fn split_preview(hit: &Hit) -> (&str, &str, &str) {
    let p = hit.preview.as_str();
    let at = |chars: usize| p.char_indices().nth(chars).map_or(p.len(), |(i, _)| i);
    let (start, end) = (at(hit.mark.0), at(hit.mark.0 + hit.mark.1));
    (&p[..start], &p[start..end], &p[end..])
}

/// Keeps the tail of a long path ("…/app/Http/Controllers").
fn shorten_left(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_owned();
    }
    format!("\u{2026}{}", s.chars().skip(n - (max - 1)).collect::<String>())
}
