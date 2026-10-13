//! Shared modal shell: main-panel dimming and pointer isolation.

use gpui::*;

const PANEL_CORNER_RADIUS: f32 = 12.;
// A third-opacity black scrim emphasizes the dialog without obscuring the page.
const DIM_COLOR: u32 = 0x00000055;

/// Preserve popup coordinates while blocking the transparent sidebar gutter.
/// Only the main panel is painted; no screenshots, images or blur work is needed.
pub fn modal_overlay(inset: f32) -> Div {
    div()
        .absolute()
        .left(px(inset))
        .right_0()
        .top_0()
        .bottom_0()
        .occlude()
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        // Occlude only the gutter: a full-window child hitbox would also block
        // the shell's outside-click handlers on the main panel.
        .child(
            div()
                .absolute()
                .left(px(-inset))
                .w(px(inset))
                .top_0()
                .bottom_0()
                .occlude(),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded(px(PANEL_CORNER_RADIUS))
                .bg(rgba(DIM_COLOR)),
        )
}
