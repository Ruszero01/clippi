//! Shared full-window modal scrim and pointer isolation.

use gpui::*;

use crate::core::frontend::PANEL_RADIUS;

// A third-opacity black scrim emphasizes the dialog without obscuring the page.
const DIM_COLOR: u32 = 0x00000055;

/// Cover the entire content window while preserving outside-click handlers.
pub fn modal_overlay() -> Div {
    div()
        .absolute()
        .inset_0()
        .occlude()
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded(px(PANEL_RADIUS))
                .bg(rgba(DIM_COLOR)),
        )
}
