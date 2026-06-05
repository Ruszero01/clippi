//! Toast notification — brief overlay message that auto-dismisses.

use gpui::*;

#[derive(IntoElement)]
pub struct Toast {
    message: String,
}

impl Toast {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl RenderOnce for Toast {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .absolute()
            .bottom(px(16.))
            .left(px(16.))
            .right(px(16.))
            .px(px(16.))
            .py(px(10.))
            .bg(rgb(0x333438))
            .rounded(px(8.))
            .border_color(rgb(0x3d3e42))
            .border(px(1.))
            .shadow_md()
            .text_size(px(12.))
            .text_color(rgb(0xe0e0e0))
            .child(self.message)
    }
}
