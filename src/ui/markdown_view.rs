//! The rendered Markdown pane shown next to the source of `.md` files.
//!
//! Ordinary Markdown is drawn by iced's `markdown` widget (through [`MdViewer`],
//! which fixes emoji); tables are drawn here: columns share the width, cell
//! text wraps, header and zebra rows - like a browser, not a one-line grid.

use std::sync::Arc;

use iced::{
    Alignment, Background, Border, Color, Element, Font, Length, Padding,
    font::Weight,
    widget::{
        column, container, markdown, row, rich_text, scrollable, space, text,
        text::Span,
    },
};

use super::theme::{self, C};
use crate::{
    editor::preview::{Align, Block, Preview, Table},
    message::Message,
};

pub const SCROLL_ID: &str = "veloedit-md-preview";
/// Horizontal space the pane itself uses (padding + scrollbar).
const PANE_CHROME: f32 = 22.0 + 26.0 + 12.0;
/// Narrowest a prose column may get before the table scrolls sideways instead.
const MIN_TEXT_COL: f32 = 120.0;

/// `pane_w`: width of the pane this is drawn in (tables decide their layout from it).
pub fn view<'a>(c: C, font_size: f32, preview: Option<&'a Preview>, pane_w: f32) -> Element<'a, Message> {
    let mut style = markdown::Style::from_palette(iced::theme::Palette {
        background: c.bg,
        text: c.text,
        primary: c.accent,
        success: c.hex_ascii,
        warning: c.dirty,
        danger: c.danger,
    });
    style.inline_code_color = c.text_bright;
    style.inline_code_highlight = markdown::Highlight {
        background: c.sidebar.into(),
        border: Border { radius: 3.0.into(), ..Border::default() },
    };
    let settings = markdown::Settings::with_text_size(font_size.max(12.0), style);
    let viewer = MdViewer { c };

    let body: Element<'a, Message> = match preview {
        Some(p) => {
            let blocks = column(p.blocks.iter().map(|b| match b {
                Block::Markdown(items) => markdown::view_with(items, settings, &viewer),
                Block::Table(t) => table(c, &viewer, settings, t, pane_w - PANE_CHROME),
            }))
            .spacing(settings.spacing.0);
            match p.window {
                // A big document: say which part this is, so the cut is not a surprise.
                Some(w) => column![
                    text(format!(
                        "Large document \u{2013} showing lines {}\u{2013}{} of {}; the preview follows the editor.",
                        w.first_line, w.last_line, w.total_lines
                    ))
                    .size(12)
                    .color(c.dim),
                    blocks,
                ]
                .spacing(14)
                .into(),
                None => blocks.into(),
            }
        }
        None => text("").into(),
    };
    container(
        scrollable(container(body).padding(Padding { top: 14.0, right: 26.0, bottom: 24.0, left: 22.0 }))
            .id(SCROLL_ID)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::panel(c, c.bg))
    .into()
}

// -------------------------------------------------------------------- tables

fn table<'a>(c: C, viewer: &MdViewer, settings: markdown::Settings, t: &'a Table, avail: f32) -> Element<'a, Message> {
    let n = t.aligns.len();
    let size = settings.text_size.0;
    let pad = Padding { top: 7.0, right: 10.0, bottom: 7.0, left: 10.0 };

    // Column widths: short columns (IDs, status marks) get what their widest
    // cell needs; the others share the rest in proportion to their typical length.
    let mut widest = vec![0usize; n];
    let mut total = vec![0usize; n];
    for r in std::iter::once(&t.header).chain(&t.rows) {
        for (i, cell) in r.iter().enumerate().take(n) {
            widest[i] = widest[i].max(cell.chars);
            total[i] += cell.chars.min(300);
        }
    }
    let char_w = size * 0.62;
    let typical = |i: usize| total[i] / (t.rows.len() + 1);
    // Short columns (IDs, status marks, dates) keep a fixed width that fits what is
    // typically in them; a fill column has no minimum width, so it must never be one.
    let short: Vec<Option<f32>> = (0..n)
        .map(|i| {
            (widest[i] <= 16 || typical(i) <= 14).then(|| {
                let chars = if widest[i] <= 16 { widest[i] } else { typical(i).max(4) };
                (chars as f32 * char_w + pad.left + pad.right + 6.0).clamp(58.0, 190.0)
            })
        })
        .collect();
    let fixed_sum: f32 = short.iter().flatten().sum();
    let prose = short.iter().filter(|w| w.is_none()).count();
    // Not enough room for readable prose columns: show every row as a small card
    // (label above value) instead of a grid - no sideways scrolling, any width works.
    let tight = prose > 0 && (avail - fixed_sum) / (prose as f32) < MIN_TEXT_COL;
    let widths: Vec<Length> = (0..n)
        .map(|i| match short[i] {
            Some(w) => Length::Fixed(w),
            None => Length::FillPortion(typical(i).clamp(20, 200) as u16),
        })
        .collect();

    let bold = Font { weight: Weight::Bold, ..Font::DEFAULT };
    let mut head_settings = settings;
    head_settings.style.font = bold;

    let make_row = |cells: &'a [crate::editor::preview::Cell], s: markdown::Settings, bg: Option<Color>| {
        let cells = cells.iter().enumerate().map(|(i, cell)| {
            let align = match t.aligns[i] {
                Align::Left => Alignment::Start,
                Align::Center => Alignment::Center,
                Align::Right => Alignment::End,
            };
            container(markdown::items(viewer, s, &cell.items)).width(widths[i]).padding(pad).align_x(align)
        });
        container(row(cells.map(Element::from)).width(Length::Fill)).style(move |_| container::Style {
            background: bg.map(Background::Color),
            ..container::Style::default()
        })
    };
    let rule = |color: Color, h: f32| {
        container(space().height(h).width(Length::Fill)).style(move |_| container::Style {
            background: Some(Background::Color(color)),
            ..container::Style::default()
        })
    };

    let zebra = Color { a: 0.45, ..c.sidebar };
    let mut rows = column![make_row(&t.header, head_settings, Some(c.hover)), rule(c.border, 2.0)];
    for (i, r) in t.rows.iter().enumerate() {
        rows = rows.push(make_row(r, settings, (i % 2 == 1).then_some(zebra)));
        rows = rows.push(rule(Color { a: 0.5, ..c.border }, 1.0));
    }
    if tight {
        return cards(c, viewer, settings, t, &short);
    }
    container(rows)
        .width(Length::Fill)
        .clip(true)
        .style(move |_| container::Style {
            border: Border { color: c.border, width: 1.0, radius: 6.0.into() },
            ..container::Style::default()
        })
        .into()
}

/// One card per table row: the short columns (id, status) on a title line, the
/// prose columns below, each under its header.
fn cards<'a>(c: C, viewer: &MdViewer, settings: markdown::Settings, t: &'a Table, short: &[Option<f32>]) -> Element<'a, Message> {
    let dim = Color { a: 0.85, ..c.dim };
    let header_text = |i: usize| -> Element<'a, Message> {
        // The header cell's own Markdown, small and dim.
        let mut small = settings;
        small.text_size = (settings.text_size.0 * 0.82).into();
        small.style.font = Font { weight: Weight::Bold, ..Font::DEFAULT };
        container(markdown::items(viewer, small, &t.header[i].items))
            .style(move |_| container::Style { text_color: Some(dim), ..container::Style::default() })
            .into()
    };

    let mut list = column![].spacing(0);
    for (r, cells) in t.rows.iter().enumerate() {
        let mut title = row![].spacing(18).align_y(Alignment::Center);
        let mut body = column![].spacing(8);
        for (i, cell) in cells.iter().enumerate() {
            if cell.chars == 0 {
                continue;
            }
            if short[i].is_some() {
                title = title.push(row![header_text(i), markdown::items(viewer, settings, &cell.items)].spacing(6).align_y(Alignment::Center));
            } else {
                body = body.push(column![header_text(i), markdown::items(viewer, settings, &cell.items)].spacing(2));
            }
        }
        let bg = if r % 2 == 1 { Some(Color { a: 0.45, ..c.sidebar }) } else { None };
        list = list.push(
            container(column![title, body].spacing(8))
                .width(Length::Fill)
                .padding(Padding { top: 10.0, right: 12.0, bottom: 12.0, left: 12.0 })
                .style(move |_| container::Style { background: bg.map(Background::Color), ..container::Style::default() }),
        );
        list = list.push(container(space().height(1.0).width(Length::Fill)).style(move |_| container::Style {
            background: Some(Background::Color(Color { a: 0.5, ..c.border })),
            ..container::Style::default()
        }));
    }
    container(list)
        .width(Length::Fill)
        .clip(true)
        .style(move |_| container::Style {
            border: Border { color: c.border, width: 1.0, radius: 6.0.into() },
            ..container::Style::default()
        })
        .into()
}

// -------------------------------------------------------------------- viewer

/// iced's default Markdown look, plus link clicks as our `Message` and emoji
/// that actually show up (see [`emoji`]).
#[derive(Clone, Copy)]
struct MdViewer {
    #[allow(dead_code)]
    c: C,
}

impl<'a> markdown::Viewer<'a, Message, iced::Theme, iced::Renderer> for MdViewer {
    fn on_link_click(url: markdown::Uri) -> Message {
        Message::OpenLink(url)
    }

    fn paragraph(&self, settings: markdown::Settings, text: &markdown::Text) -> Element<'a, Message> {
        rich_text(emoji(text.spans(settings.style)))
            .size(settings.text_size)
            .on_link_click(Message::OpenLink)
            .into()
    }

    fn heading(
        &self,
        settings: markdown::Settings,
        level: &'a markdown::HeadingLevel,
        text: &'a markdown::Text,
        index: usize,
    ) -> Element<'a, Message> {
        use markdown::HeadingLevel::*;
        let size = match level {
            H1 => settings.h1_size,
            H2 => settings.h2_size,
            H3 => settings.h3_size,
            H4 => settings.h4_size,
            H5 => settings.h5_size,
            H6 => settings.h6_size,
        };
        let top = if index > 0 { settings.text_size.0 / 2.0 } else { 0.0 };
        container(rich_text(emoji(text.spans(settings.style))).on_link_click(Message::OpenLink).size(size))
            .padding(Padding { top, ..Padding::ZERO })
            .into()
    }
}

/// Color emoji (Noto Color Emoji is a COLRv1 font) are not drawn by iced's text
/// renderer: they come out blank. The common status marks are swapped for
/// monochrome symbols in the emoji's color.
fn emoji(spans: Arc<[Span<'static, markdown::Uri>]>) -> Arc<[Span<'static, markdown::Uri>]> {
    if !spans.iter().any(|s| s.text.chars().any(|c| substitute(c).is_some() || c == '\u{FE0F}')) {
        return spans;
    }
    let mut out: Vec<Span<'static, markdown::Uri>> = Vec::with_capacity(spans.len() + 4);
    for span in spans.iter() {
        let mut plain = String::new();
        let flush = |plain: &mut String, out: &mut Vec<Span<'static, markdown::Uri>>| {
            if !plain.is_empty() {
                out.push(Span { text: std::mem::take(plain).into(), ..span.clone() });
            }
        };
        for ch in span.text.chars() {
            match substitute(ch) {
                Some((symbol, color)) => {
                    flush(&mut plain, &mut out);
                    out.push(Span { text: symbol.to_string().into(), color: Some(color), ..span.clone() });
                }
                None if ch == '\u{FE0F}' => {} // emoji presentation selector: nothing to show
                None => plain.push(ch),
            }
        }
        flush(&mut plain, &mut out);
    }
    out.into()
}

/// `(symbol, color)` standing in for an emoji.
fn substitute(c: char) -> Option<(char, Color)> {
    let rgb = |r: u8, g: u8, b: u8| Color::from_rgb8(r, g, b);
    let (green, red, amber, blue) = (rgb(0x3f, 0xb9, 0x50), rgb(0xf8, 0x51, 0x49), rgb(0xe3, 0xb3, 0x41), rgb(0x58, 0xa6, 0xff));
    Some(match c {
        '\u{2705}' => ('\u{2714}', green),              // ✅
        '\u{274C}' | '\u{274E}' => ('\u{2718}', red),   // ❌ ❎
        '\u{26A0}' => ('\u{26A0}', amber),              // ⚠
        '\u{1F7E1}' => ('\u{25CF}', amber),             // 🟡
        '\u{1F7E2}' => ('\u{25CF}', green),             // 🟢
        '\u{1F534}' => ('\u{25CF}', red),               // 🔴
        '\u{1F535}' => ('\u{25CF}', blue),              // 🔵
        '\u{1F7E0}' => ('\u{25CF}', rgb(0xf0, 0x88, 0x3e)), // 🟠
        '\u{1F7E3}' => ('\u{25CF}', rgb(0xbc, 0x8c, 0xff)), // 🟣
        '\u{26AA}' => ('\u{25CB}', rgb(0xc0, 0xc6, 0xd0)),  // ⚪
        '\u{26AB}' => ('\u{25CF}', rgb(0x8b, 0x93, 0xa1)),  // ⚫
        '\u{2B50}' | '\u{1F31F}' => ('\u{2605}', amber), // ⭐ 🌟
        '\u{2728}' => ('\u{2726}', amber),              // ✨
        '\u{2139}' => ('\u{2139}', blue),               // ℹ
        '\u{2757}' | '\u{2755}' => ('!', red),          // ❗ ❕
        '\u{2753}' | '\u{2754}' => ('?', amber),        // ❓ ❔
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str) -> Span<'static, markdown::Uri> {
        Span::new(text.to_owned())
    }

    #[test]
    fn status_emoji_become_colored_symbols_and_other_text_is_untouched() {
        let untouched: Arc<[_]> = vec![span("no emoji \u{2192} here")].into();
        assert!(Arc::ptr_eq(&untouched, &emoji(untouched.clone())), "no allocation without emoji");

        let spans: Arc<[_]> = vec![span("ok \u{2705} and \u{1F7E1}\u{FE0F} done")].into();
        let out = emoji(spans);
        let texts: Vec<&str> = out.iter().map(|s| s.text.as_ref()).collect();
        assert_eq!(texts, vec!["ok ", "\u{2714}", " and ", "\u{25CF}", " done"], "VS16 dropped, text split around symbols");
        assert!(out[1].color.is_some() && out[3].color.is_some() && out[0].color.is_none());
        assert_ne!(out[1].color, out[3].color, "green check vs yellow dot");
    }
}
