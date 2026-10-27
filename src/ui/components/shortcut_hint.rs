//! Animated external shortcut hints with shared theme and transition behavior.

use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_transitions::WindowUseTransition;

use crate::ui::{font, theme::ClippiTheme};

const TRANSITION_MS: u64 = 160;

/// Stable keys preserve the current value when Alt is released mid-animation.
///
/// Callers that can hide their window must fold a per-hide generation into the
/// key: no frame is rendered after a hide, so a hint that was fully shown at
/// that moment would otherwise be painted at full brightness on the next show
/// and only then fade out.
pub fn shortcut_progress(
    id: impl Into<ElementId>,
    visible: bool,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    let transition = window
        .use_keyed_transition(id, cx, Duration::from_millis(TRANSITION_MS), |_, _| 0.0_f32)
        .with_easing(|t| 1.0 - (1.0 - t).powi(3));
    transition.update(cx, |value, _cx| {
        *value = if visible { 1.0 } else { 0.0 };
    });
    let progress = *transition.evaluate(window, cx);
    progress.clamp(0.0, 1.0)
}

#[derive(Clone, Copy)]
struct HintGeometry {
    width: f32,
    height: f32,
    font_size: f32,
}

impl HintGeometry {
    fn new(scale: f32) -> Self {
        let width = (13.0 * scale).clamp(12.0, 18.0);
        let font_size = (9.0 * scale).min(width - 2.0);
        Self {
            width,
            height: (font_size + 4.0).clamp(12.0, 18.0),
            font_size,
        }
    }

    fn above_top(self, progress: f32) -> f32 {
        -self.height - 2.0 + 6.0 * (1.0 - progress.clamp(0.0, 1.0))
    }
}

fn shortcut_foreground(accent: Rgba) -> Rgba {
    let linear = |channel: f32| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance =
        0.2126 * linear(accent.r) + 0.7152 * linear(accent.g) + 0.0722 * linear(accent.b);
    if luminance > 0.179 {
        rgb(0x000000)
    } else {
        rgb(0xffffff)
    }
}

fn keycap(key: SharedString, geometry: HintGeometry, theme: &ClippiTheme) -> Div {
    div()
        .flex_shrink_0()
        .w(px(geometry.width))
        .h(px(geometry.height))
        .rounded(px(3.))
        .bg(theme.accent)
        .font_family(font::system_family())
        .text_size(px(geometry.font_size))
        .line_height(px(geometry.height - 2.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(shortcut_foreground(theme.accent))
        .flex()
        .items_center()
        .justify_center()
        .child(key)
}

/// Keep the button centered and reveal its key above it.
/// The overlay is outside the button's clip and paints over the preceding row.
pub fn external_shortcut(
    id: impl Into<ElementId>,
    key: impl Into<SharedString>,
    visible: bool,
    button: impl IntoElement,
    theme: &ClippiTheme,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let progress = shortcut_progress(id, visible, window, cx);
    let geometry = HintGeometry::new(font::scale());
    div()
        .relative()
        .flex_1()
        .min_w(px(0.))
        .child(button)
        .when(progress > 0.001, |slot| {
            slot.child(
                div()
                    .absolute()
                    .top(px(geometry.above_top(progress)))
                    .left_0()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        deferred(keycap(key.into(), geometry, theme).opacity(progress))
                            .with_priority(1),
                    ),
            )
        })
}

/// A small deferred popover below a toolbar icon. Deferral paints it above the
/// following search row without increasing header height or moving the icon.
pub fn shortcut_popover(key: impl Into<SharedString>, progress: f32, theme: &ClippiTheme) -> Div {
    let geometry = HintGeometry::new(font::scale());
    div()
        .absolute()
        .top(px(24.0 + 3.0 * progress))
        .left(px((28.0 - geometry.width) / 2.0))
        .child(
            deferred(
                keycap(key.into(), geometry, theme)
                    .border_1()
                    .border_color(theme.accent)
                    .opacity(progress),
            )
            .with_priority(1),
        )
}

#[cfg(test)]
mod tests {
    use super::HintGeometry;

    #[test]
    fn external_tip_moves_up_and_finishes_fully_above_the_button() {
        for scale in [0.92, 1.0, 1.15, 1.3, 1.6] {
            let geometry = HintGeometry::new(scale);
            let start = geometry.above_top(0.0);
            let middle = geometry.above_top(0.5);
            let finish = geometry.above_top(1.0);
            assert!(start > middle && middle > finish);
            assert!(finish + geometry.height < 0.0);
        }
    }
}
