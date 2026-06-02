//! Edit panel — full-text editor for clipboard items.
//!
//! Displays and allows editing of clipboard content, notes, and metadata.

use gpui::*;

/// Edit panel entity for viewing and editing a clipboard item.
pub struct EditPanel {
    item_text: String,
    note_text: String,
}

impl EditPanel {
    pub fn new(item_text: String, note_text: String) -> Self {
        Self { item_text, note_text }
    }
}

impl Render for EditPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x1e1f23))
            .p(px(16.))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(0x888888))
                    .mb(px(8.))
                    .child("CONTENT"),
            )
            .child(
                div()
                    .flex_1()
                    .bg(rgb(0x25262a))
                    .rounded(px(8.))
                    .p(px(12.))
                    .text_size(px(13.))
                    .text_color(rgb(0xe0e0e0))
                    .child(self.item_text.clone()),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(0x888888))
                    .mt(px(16.))
                    .mb(px(8.))
                    .child("NOTE"),
            )
            .child(
                div()
                    .h(px(60.))
                    .bg(rgb(0x25262a))
                    .rounded(px(8.))
                    .p(px(12.))
                    .text_size(px(12.))
                    .text_color(rgb(0x999999))
                    .child({
                        let note = self.note_text.clone();
                        if note.is_empty() {
                            "Add a note...".to_string()
                        } else {
                            note
                        }
                    }),
            )
    }
}
