//! Text editor view: the custom [`EditorView`] widget wired to one document.

use iced::{Element, widget::mouse_area};

use super::{editor_view::EditorView, theme::C};
use crate::{
    editor::{Pane, TabId, TextDoc, state::EditorState},
    message::Message,
};

/// Stable widget id so the editor can be focused programmatically.
pub const EDITOR_ID: &str = "veloedit-editor";
/// Id of the second pane of a split tab.
pub const EDITOR2_ID: &str = "veloedit-editor-2";

/// What the widget needs besides the document: which state to draw (a split tab has two,
/// one per pane) and which pane it is (`None` = the tab is not split).
pub struct Pick<'a> {
    pub state: &'a EditorState,
    pub pane: Option<Pane>,
    pub minimap: bool,
}

pub fn view<'a>(
    c: C,
    font_size: f32,
    modal: bool,
    wrap: bool,
    id: TabId,
    doc: &'a TextDoc,
    pick: Pick<'a>,
    matches: Option<&'a [(usize, usize)]>,
) -> Element<'a, Message> {
    let pane = pick.pane;
    let editor = EditorView::new(pick.state, c, font_size, move |action| match pane {
        None => Message::Editor(id, action),
        Some(p) => Message::EditorPane(id, p, action),
    })
    .id(if pane == Some(Pane::Second) { EDITOR2_ID } else { EDITOR_ID })
    .spans(doc.hl.spans.as_deref())
    .git(doc.git.marks.as_deref())
    .matches(matches)
    .modal(modal)
    .minimap(pick.minimap)
    .wrap(wrap);
    mouse_area(editor).on_right_press(Message::EditorContext).into()
}
