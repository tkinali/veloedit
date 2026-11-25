//! Color schemes and shared widget styles. A scheme is a plain `&'static Colors`
//! that the app passes into every view function (no globals).

use iced::{
    Background, Border, Color, Theme, border,
    theme::Palette,
    widget::{button, container, text_input},
};

use crate::syntax::Group;

/// The fixed-width font of the editor, hex view and result lists. The system's generic
/// "monospace" everywhere but on Windows and macOS, where that maps to Courier New and Courier:
/// Consolas ships with every Windows, Menlo with every Mac.
pub const MONO: iced::Font = if cfg!(windows) {
    iced::Font::with_name("Consolas")
} else if cfg!(target_os = "macos") {
    iced::Font::with_name("Menlo")
} else {
    iced::Font::MONOSPACE
};

pub const FONT_SIZE: f32 = 13.0;

/// Every color the UI uses. Cheap to share: always handled as [`C`].
pub struct Colors {
    pub name: &'static str,
    pub bg: Color,
    pub sidebar: Color,
    pub tabbar: Color,
    pub status: Color,
    pub border: Color,
    pub hover: Color,
    pub selected: Color,
    pub text: Color,
    pub text_bright: Color,
    pub dim: Color,
    pub accent: Color,
    pub on_accent: Color,
    pub dirty: Color,
    pub hex_addr: Color,
    pub hex_ascii: Color,
    pub danger: Color,
    /// Indexed by `Group as usize`.
    pub syntax: [Color; Group::COUNT],
}

pub type C = &'static Colors;

impl Colors {
    /// A light scheme (dark text on a light background).
    pub fn is_light(&self) -> bool {
        self.bg.r + self.bg.g + self.bg.b > 1.5
    }

    pub fn syntax(&self, group: Group) -> Color {
        self.syntax[group as usize]
    }
}

const fn rgb(hex: u32) -> Color {
    Color {
        r: ((hex >> 16) & 0xFF) as f32 / 255.0,
        g: ((hex >> 8) & 0xFF) as f32 / 255.0,
        b: (hex & 0xFF) as f32 / 255.0,
        a: 1.0,
    }
}

pub static DARK: Colors = Colors {
    name: "dark",
    bg: rgb(0x1e2127),
    sidebar: rgb(0x181a1f),
    tabbar: rgb(0x14161a),
    status: rgb(0x111316),
    border: rgb(0x2c313a),
    hover: rgb(0x262a31),
    selected: rgb(0x2f3541),
    text: rgb(0xabb2bf),
    text_bright: rgb(0xdfe3ea),
    dim: rgb(0x6b7280),
    accent: rgb(0x61afef),
    on_accent: rgb(0x10131a),
    dirty: rgb(0xe5c07b),
    hex_addr: rgb(0x5c6370),
    hex_ascii: rgb(0x98c379),
    danger: rgb(0xe06c75),
    // Keyword, Function, Type, String, Comment, Number, Constant, Operator,
    // Property, Punctuation, Attribute, Tag, Escape
    syntax: [
        rgb(0xc678dd), rgb(0x61afef), rgb(0xe5c07b), rgb(0x98c379), rgb(0x5c6370),
        rgb(0xd19a66), rgb(0xd19a66), rgb(0x56b6c2), rgb(0xe06c75), rgb(0x8b93a1),
        rgb(0xe5c07b), rgb(0xe06c75), rgb(0x56b6c2),
    ],
};

pub static LIGHT: Colors = Colors {
    name: "light",
    bg: rgb(0xfafafa),
    sidebar: rgb(0xf0f1f3),
    tabbar: rgb(0xe6e8eb),
    status: rgb(0xdfe2e6),
    border: rgb(0xd0d4da),
    hover: rgb(0xe4e7ec),
    selected: rgb(0xd3dcec),
    text: rgb(0x383a42),
    text_bright: rgb(0x1d1f24),
    dim: rgb(0x8a8f98),
    accent: rgb(0x2f6fd6),
    on_accent: rgb(0xffffff),
    dirty: rgb(0xb8860b),
    hex_addr: rgb(0x9aa0a8),
    hex_ascii: rgb(0x3a7d2c),
    danger: rgb(0xc0392b),
    syntax: [
        rgb(0xa626a4), rgb(0x4078f2), rgb(0xc18401), rgb(0x50a14f), rgb(0xa0a1a7),
        rgb(0x986801), rgb(0x986801), rgb(0x0184bc), rgb(0xe45649), rgb(0x6b6f78),
        rgb(0xc18401), rgb(0xe45649), rgb(0x0184bc),
    ],
};

/// All built-in schemes (palette: "Theme: ...").
pub const ALL: [C; 2] = [&DARK, &LIGHT];

pub fn by_name(name: &str) -> Option<C> {
    ALL.into_iter().find(|c| c.name.eq_ignore_ascii_case(name))
}

/// iced `Theme` derived from a scheme (drives default scrollbars etc.).
pub fn theme(c: C) -> Theme {
    Theme::custom(
        format!("VeloEdit {}", c.name),
        Palette {
            background: c.bg,
            text: c.text,
            primary: c.accent,
            success: c.hex_ascii,
            warning: c.dirty,
            danger: c.danger,
        },
    )
}

pub fn panel(c: C, bg: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(bg)),
        text_color: Some(c.text),
        ..container::Style::default()
    }
}

pub fn bordered(c: C, bg: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(bg)),
        text_color: Some(c.text),
        border: Border {
            color: c.border,
            width: 1.0,
            radius: border::Radius::from(0),
        },
        ..container::Style::default()
    }
}

/// Flat button: transparent, highlights on hover / when `selected`.
pub fn flat(c: C, selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let bg = match status {
            _ if selected => Some(c.selected),
            button::Status::Hovered | button::Status::Pressed => Some(c.hover),
            _ => None,
        };
        button::Style {
            background: bg.map(Background::Color),
            text_color: if selected { c.text_bright } else { c.text },
            border: Border::default(),
            ..button::Style::default()
        }
    }
}

pub fn primary_button(c: C) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let bg = match status {
            button::Status::Hovered | button::Status::Pressed => Color {
                r: (c.accent.r + 0.08).min(1.0),
                g: (c.accent.g + 0.08).min(1.0),
                b: (c.accent.b + 0.08).min(1.0),
                a: 1.0,
            },
            _ => c.accent,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: c.on_accent,
            border: border::rounded(4),
            ..button::Style::default()
        }
    }
}

pub fn secondary_button(c: C) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let bg = match status {
            button::Status::Hovered | button::Status::Pressed => c.selected,
            _ => c.hover,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: c.text_bright,
            border: border::rounded(4),
            ..button::Style::default()
        }
    }
}

pub fn input(c: C) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    move |_, status| text_input::Style {
        background: Background::Color(c.bg),
        border: Border {
            color: if matches!(status, text_input::Status::Focused { .. }) { c.accent } else { c.border },
            width: 1.0,
            radius: 6.0.into(),
        },
        icon: c.dim,
        placeholder: c.dim,
        value: c.text_bright,
        selection: Color { a: 0.35, ..c.accent },
    }
}
