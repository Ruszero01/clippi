//! Shared fixed-width horizontal filter strip for picker categories and source apps.
use crate::core::i18n_keys::I18nKey;
use crate::ui::filter_bar::{filter_action_slot, filter_chip, filter_toolbar};
use crate::ui::font::fs;
use crate::ui::theme::ClippiTheme;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::tooltip::Tooltip;

pub(crate) fn scroll_edges(scroll: &ScrollHandle) -> (bool, bool) {
    let offset = f32::from(scroll.offset().x);
    let limit = f32::from(scroll.max_offset().width);
    (offset < -0.5, -offset < limit - 0.5)
}

/// Fixed-width choices shared by the emoji categories and source app scrollers.
pub(crate) fn scroll_filter_chip(
    id: impl Into<ElementId>,
    active: bool,
    theme: &ClippiTheme,
    width: f32,
) -> Stateful<Div> {
    filter_chip(id, active, theme)
        .border_1()
        .border_color(if active {
            Rgba {
                a: 0.45,
                ..theme.accent
            }
        } else {
            rgba(0x00000000)
        })
        .flex_none()
        .flex_basis(px(width))
        .w(px(width))
        .min_w(px(width))
}

fn scroll_target(current: f32, limit: f32, stride: f32, direction: f32) -> f32 {
    if stride <= 0. {
        return 0.;
    }
    (((current / stride).round() + direction).max(0.) * stride).clamp(0., limit.max(0.))
}

pub(crate) struct ScrollFilterLayout<'a> {
    pub chip_width: f32,
    pub chip_gap: f32,
    pub scroll_left: &'a std::cell::Cell<bool>,
}

/// Shared icon scroller with a pinned first choice, cell snapping, and a reversing endpoint button.
pub(crate) fn scroll_filter_bar(
    id: &'static str,
    theme: &ClippiTheme,
    scroll: &ScrollHandle,
    owner: EntityId,
    mut choices: Vec<Stateful<Div>>,
    layout: ScrollFilterLayout<'_>,
) -> Div {
    let pinned = if choices.is_empty() {
        None
    } else {
        Some(choices.remove(0))
    };
    let edges = scroll_edges(scroll);
    // Keep the current direction in the middle; reverse only at an endpoint.
    if !edges.0 {
        layout.scroll_left.set(false);
    } else if !edges.1 {
        layout.scroll_left.set(true);
    }
    let left = layout.scroll_left.get();
    let arrow = |left: bool, enabled: bool, width: f32| {
        let scroll = scroll.clone();
        let tooltip = if left {
            I18nKey::SourceAppScrollStart.text()
        } else {
            I18nKey::SourceAppScrollEnd.text()
        };
        filter_chip((id, if left { 0usize } else { 1usize }), false, theme)
            .flex_none()
            // flex_none does not reset the zero basis inherited from filter_chip.
            .flex_basis(px(width))
            .w(px(width))
            .min_w(px(width))
            .opacity(if enabled { 1. } else { 0.25 })
            .cursor(if enabled {
                CursorStyle::PointingHand
            } else {
                CursorStyle::Arrow
            })
            .when(enabled, |button| {
                button.hover(|style| style.bg(theme.btn_hover))
            })
            .tooltip(move |window, cx| {
                Tooltip::element(move |_, _| div().text_size(fs(10.)).child(tooltip))
                    .build(window, cx)
            })
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.stop_propagation();
                if enabled {
                    let limit = f32::from(scroll.max_offset().width);
                    scroll.set_offset(point(px(if left { 0. } else { -limit }), px(0.)));
                    cx.notify(owner);
                }
            })
            .child(
                svg()
                    .path(if left {
                        "picker-icons/chevron-left.svg"
                    } else {
                        "picker-icons/chevron-right.svg"
                    })
                    .size(fs(12.))
                    .text_color(theme.text_2),
            )
    };
    let wheel_scroll = scroll.clone();
    let stride = layout.chip_width + layout.chip_gap;
    let applications = div()
        .id(id)
        .flex()
        .flex_1()
        .min_w(px(0.))
        .h(px(22.))
        .overflow_x_scroll()
        .overflow_y_hidden()
        .track_scroll(scroll)
        .child(
            div()
                .id((id, 2usize))
                .flex()
                .flex_none()
                .h(px(22.))
                .items_center()
                .gap(px(layout.chip_gap))
                .children(choices)
                .on_scroll_wheel(move |event, _, cx| {
                    cx.stop_propagation();
                    let delta = event.delta.pixel_delta(px(22.));
                    let delta = if delta.x.abs() > delta.y.abs() {
                        f32::from(delta.x)
                    } else {
                        f32::from(delta.y)
                    };
                    if delta.abs() < 0.01 || stride <= 0. {
                        return;
                    }
                    let limit = f32::from(wheel_scroll.max_offset().width);
                    let current = -f32::from(wheel_scroll.offset().x);
                    let target = scroll_target(current, limit, stride, -delta.signum());
                    wheel_scroll.set_offset(point(px(-target), px(0.)));
                    cx.notify(owner);
                }),
        );
    let mut content = div()
        .flex()
        .flex_1()
        .min_w(px(0.))
        .items_center()
        .gap(px(layout.chip_gap));
    if let Some(pinned) = pinned {
        content = content.child(pinned);
    }
    content = content.child(applications);
    let scroll = scroll.clone();
    filter_toolbar()
        .relative()
        .gap(px(6.))
        .child(content)
        .child(
            div()
                .w(px(1.))
                .h(px(14.))
                .flex_shrink_0()
                .bg(if theme.bg == rgb(0x191a1b) {
                    rgba(0xffffff18)
                } else {
                    rgba(0x00000014)
                }),
        )
        .child(filter_action_slot().child(arrow(left, if left { edges.0 } else { edges.1 }, 24.)))
        .child(
            canvas(
                |_, _, _| (),
                move |_, _, _, cx| {
                    // A resize changes the column width. Re-align the saved offset
                    // after layout so it cannot leave the first visible icon sliced.
                    let current = -f32::from(scroll.offset().x);
                    let limit = f32::from(scroll.max_offset().width);
                    let aligned = scroll_target(current, limit, stride, 0.);
                    if (current - aligned).abs() > 0.1 {
                        scroll.set_offset(point(px(-aligned), px(0.)));
                        cx.notify(owner);
                    } else if scroll_edges(&scroll) != edges {
                        cx.notify(owner);
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
}

#[cfg(test)]
mod filter_scroll_tests {
    use super::scroll_target;

    #[test]
    fn wheel_moves_one_cell_and_stops_at_both_ends() {
        assert_eq!(scroll_target(0., 140., 35., 1.), 35.);
        assert_eq!(scroll_target(105., 140., 35., 1.), 140.);
        assert_eq!(scroll_target(140., 140., 35., 1.), 140.);
        assert_eq!(scroll_target(140., 140., 35., -1.), 105.);
        assert_eq!(scroll_target(0., 140., 35., -1.), 0.);
    }

    #[test]
    fn resize_realigns_offset_and_removed_items_clamp_it() {
        assert_eq!(scroll_target(70., 200., 45., 0.), 90.);
        assert_eq!(scroll_target(140., 35., 35., 0.), 35.);
        assert_eq!(scroll_target(140., 0., 35., 0.), 0.);
    }
}
