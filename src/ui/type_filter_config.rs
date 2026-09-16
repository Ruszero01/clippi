//! Type filter config panel — floating panel for configuring which
//! content-type filter buttons are visible and their display order.
//!
//! --- Right-click the filter bar to open. ---
//! --- Each row: circular checkbox (show/hide) + icon + label + drag handle. ---

use gpui::prelude::*;
use gpui::*;

use crate::core::i18n_keys::I18nKey;
use crate::state::app::AppState;

use super::components::reorder::{
    drag_grip, drag_handle, drop_target, track_drag_bounds, ReorderDrag, ReorderKey,
};
use super::filter_bar::{filter_type_display, FilterBar};
use super::theme::ClippiTheme;

pub struct TypeFilterConfigPanel {
    state: Entity<AppState>,
    filter_bar: Entity<FilterBar>,
}

impl TypeFilterConfigPanel {
    pub fn new(
        state: Entity<AppState>,
        filter_bar: Entity<FilterBar>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self { state, filter_bar }
    }

    pub fn close(&self, cx: &mut App) {
        self.filter_bar
            .update(cx, |bar, cx| bar.close_filter_config(cx));
    }

    fn toggle_visible(&self, key: &str, cx: &mut App) {
        self.state.update(cx, |s, _cx| {
            if let Some(entry) = s
                .settings
                .type_filter_config
                .iter_mut()
                .find(|e| e.key == key)
            {
                let was_visible = entry.visible;
                entry.visible = !entry.visible;
                // If hiding a currently active filter, deactivate it
                if was_visible && !entry.visible && s.filters.is_type_active(key) {
                    s.filters.toggle_type(key);
                }
                s.settings.save();
                s.reload_items();
            }
        });
        self.filter_bar.update(cx, |_b, cx| cx.notify());
    }
}

impl Render for TypeFilterConfigPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app_state = self.state.read(cx);
        let theme = ClippiTheme::from_setting(&app_state.settings.theme, Some(window.appearance()));
        let config = app_state.settings.type_filter_config.clone();
        let _ = app_state;

        let text_1 = theme.text_1;
        let text_2 = theme.text_2;
        let surface = theme.panel_surface;
        let sep_line = theme.panel_sep_line;
        let btn_hover = theme.btn_hover;
        let panel_border = if theme.bg == rgb(0x191a1b) {
            rgba(0xffffff14)
        } else {
            rgba(0x00000012)
        };
        let this_entity = cx.entity().clone();

        div()
            .flex()
            .flex_col()
            .w(px(240.))
            .bg(surface)
            .border_color(panel_border)
            .border(px(1.))
            .rounded(px(8.))
            .shadow_lg()
            .p(px(8.))
            .gap(px(4.))
            // --- Title row ---
            .child({
                let this = this_entity.clone();
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(24.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(I18nKey::FilterConfigTitle.text()),
                    )
                    .child(
                        div()
                            .w(px(22.))
                            .h(px(22.))
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(|el| el.bg(btn_hover))
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                cx.stop_propagation();
                                this.update(cx, |panel, cx| panel.close(cx));
                            })
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_family("iconfont")
                                    .text_color(text_2)
                                    .child("\u{e7b7}"),
                            ),
                    )
            })
            // --- Separator ---
            .child(div().w_full().h(px(1.)).bg(sep_line))
            // --- Item list ---
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(config.iter().map(|entry| {
                        let key = entry.key.clone();
                        let visible = entry.visible;

                        let preview_key = key.clone();
                        let preview_theme = theme.clone();
                        let drag = ReorderDrag::new(ReorderKey::Filter(key.clone()), move || {
                            filter_row(&preview_key, visible, &preview_theme)
                                .child(drag_grip(&preview_theme))
                                .into_any_element()
                        });
                        let row = filter_row(&key, visible, &theme)
                            .id(SharedString::from(format!("type-filter-{key}")))
                            .cursor(CursorStyle::PointingHand)
                            .hover(move |style| style.bg(btn_hover))
                            .on_click({
                                let this = this_entity.clone();
                                let key = key.clone();
                                move |_, _, cx| {
                                    cx.stop_propagation();
                                    this.update(cx, |panel, cx| panel.toggle_visible(&key, cx));
                                }
                            })
                            .child(drag_handle(drag.clone(), &theme));
                        let row = track_drag_bounds(row, &drag);
                        let this = this_entity.clone();
                        drop_target(
                            row,
                            ReorderKey::Filter(key.clone()),
                            false,
                            &theme,
                            move |source, after, _, cx| {
                                if let ReorderKey::Filter(source) = source {
                                    this.update(cx, |panel, cx| {
                                        panel.state.update(cx, |state, cx| {
                                            state.reorder_type_filter(source, &key, after);
                                            cx.notify();
                                        });
                                        panel.filter_bar.update(cx, |_, cx| cx.notify());
                                        cx.notify();
                                    });
                                }
                            },
                        )
                    })),
            )
    }
}

/// Passive row visuals shared with the drag preview; interaction stays on the live row.
fn filter_row(key: &str, visible: bool, theme: &ClippiTheme) -> Div {
    let (icon, label) = filter_type_display(key).unwrap_or(("\u{e606}", key.to_string()));
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .h(px(28.))
        .px(px(4.))
        .rounded(px(4.))
        .child(
            div()
                .text_size(px(12.))
                .font_family("iconfont")
                .text_color(if visible { theme.accent } else { theme.text_3 })
                .flex_shrink_0()
                .child(if visible { "\u{e61f}" } else { "\u{e831}" }),
        )
        .child(
            div()
                .ml(px(6.))
                .text_size(px(12.))
                .font_family("iconfont")
                .text_color(if visible { theme.text_2 } else { theme.text_3 })
                .child(icon.to_string()),
        )
        .child(
            div()
                .ml(px(4.))
                .text_size(px(11.))
                .flex_1()
                .text_color(if visible { theme.text_1 } else { theme.text_3 })
                .child(label),
        )
}
