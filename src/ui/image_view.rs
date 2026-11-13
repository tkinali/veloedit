//! Read-only image preview, centred and scaled down to fit (never up-scaled).

use iced::{
    ContentFit, Element, Length,
    widget::{container, image},
};

use super::theme::{self, C};
use crate::{editor::ImageDoc, message::Message};

pub fn view(c: C, doc: &ImageDoc) -> Element<'_, Message> {
    container(
        image(doc.handle.clone())
            .content_fit(ContentFit::ScaleDown)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .padding(16)
    .center(Length::Fill)
    .style(theme::panel(c, c.bg))
    .into()
}
