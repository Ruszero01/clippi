//! Add backend panel — floating panel for adding sync backends.

use gpui::*;

#[derive(IntoElement)]
pub struct AddBackendPanel;

impl AddBackendPanel {
    pub fn new() -> Self {
        Self
    }
}

impl RenderOnce for AddBackendPanel {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .min_w(px(280.))
            .bg(rgb(0x25262a))
            .border_color(rgb(0x3d3e42))
            .border(px(1.))
            .rounded(px(8.))
            .shadow_md()
            .p(px(16.))
            .child(
                div()
                    .text_size(px(14.))
                    .text_color(rgb(0xe0e0e0))
                    .mb(px(12.))
                    .child("Add Sync Backend"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(0x888888))
                    .child("Local Folder or WebDAV configuration coming soon..."),
            )
    }
}
