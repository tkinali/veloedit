//! Pure view functions: `&State -> Element<Message>`. No state lives here.

pub mod editor_view;
pub mod file_badge;
pub mod find_bar;
pub mod hex_view;
pub mod image_view;
pub mod markdown_view;
pub mod menu;
pub mod menu_bar;
pub mod palette;
pub mod search_panel;
pub mod settings_panel;
pub mod sidebar;
pub mod status_bar;
pub mod tab_bar;
pub mod text_view;
pub mod theme;
pub mod virtual_list;

use iced::keyboard::{Key, key::{Code, Physical}};

/// The key a widget should act on. iced reports `key` *without* modifiers, and for the
/// numpad that is the NumLock-off key (KP_1 is "End", KP_4 is "←", KP_0 is "Insert") even
/// while NumLock is on and the key types a digit. The numpad's digit and decimal keys
/// therefore go by the `logical` key, which has NumLock (and Shift) applied.
pub fn effective_key<'a>(key: &'a Key, logical: &'a Key, physical: Physical) -> &'a Key {
    match physical {
        Physical::Code(
            Code::Numpad0
            | Code::Numpad1
            | Code::Numpad2
            | Code::Numpad3
            | Code::Numpad4
            | Code::Numpad5
            | Code::Numpad6
            | Code::Numpad7
            | Code::Numpad8
            | Code::Numpad9
            | Code::NumpadDecimal,
        ) => logical,
        _ => key,
    }
}
