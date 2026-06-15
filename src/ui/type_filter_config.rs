//! Type filter config panel — floating panel for configuring which
//! content-type filter buttons are visible and their display order.
//!
//! --- Right-click the filter bar to open. ---
//! --- Each row: circular checkbox (show/hide) + icon + label + up/down arrows. ---

use gpui::prelude::*;
use gpui::*;

use crate::core::i18n_keys::I18nKey;
use crate::state::app::AppState;

use super::search_bar::{filter_type_display, SearchBar};
use super::theme::ClippiTheme;

pub struct TypeFilterConfigPanel {
    state: Entity<AppState>,
    search_bar: Entity<SearchBar>,
}

impl TypeFilterConfigPanel {
    pub fn new(
        state: Entity<AppState>,
        search_bar: Entity<SearchBar>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self { state, search_bar }
    }

    pub fn close(&self, cx: &mut App) {
        self.search_bar
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
        self.search_bar.update(cx, |_b, cx| cx.notify());
    }

    fn move_up(&self, index: usize, cx: &mut App) {
        if index == 0 {
            return;
        }
        self.state.update(cx, |s, _cx| {
            s.settings.type_filter_config.swap(index, index - 1);
            s.settings.save();
            s.reload_items();
        });
        self.search_bar.update(cx, |_b, cx| cx.notify());
    }

    fn move_down(&self, index: usize, cx: &mut App) {
        self.state.update(cx, |s, _cx| {
            let len = s.settings.type_filter_config.len();
            if index + 1 >= len {
                return;
            }
            s.settings.type_filter_config.swap(index, index + 1);
            s.settings.save();
            s.reload_items();
        });
        self.search_bar.update(cx, |_b, cx| cx.notify());
    }
}

impl Render for TypeFilterConfigPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app_state = self.state.read(cx);
        let theme = ClippiTheme::from_setting(&app_state.settings.theme, Some(window.appearance()));
        let config = app_state.settings.type_filter_config.clone();
        let _ = app_state;

        let accent = theme.accent;
        let text_1 = theme.text_1;
        let text_2 = theme.text_2;
        let text_3 = theme.text_3;
        let surface = theme.panel_surface;
        let sep_line = theme.panel_sep_line;
        let btn_hover = theme.btn_hover;
        let panel_border = if theme.bg == rgb(0x191a1b) {
            rgba(0xffffff14)
        } else {
            rgba(0x00000012)
        };
        let is_dark = theme.bg == rgb(0x191a1b);
        let arrow_bg = if is_dark {
            rgba(0xffffff0a)
        } else {
            rgba(0x00000008)
        };
        let arrow_hover = if is_dark {
            rgba(0xffffff18)
        } else {
            rgba(0x00000014)
        };

        let this_entity = cx.entity().clone();
        let len = config.len();

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
                    .children(config.iter().enumerate().map(|(i, entry)| {
                        let key = entry.key.clone();
                        let visible = entry.visible;
                        let is_first = i == 0;
                        let is_last = i + 1 >= len;

                        // Look up display info from FILTER_TYPES
                        let (icon, label) =
                            filter_type_display(&key).unwrap_or(("\u{e60e}", key.clone()));

                        let key_for_toggle = key.clone();

                        // Radio-style icon: \u{e831} = empty circle, \u{e61f} = filled circle
                        let checkbox = div()
                            .text_size(px(12.))
                            .font_family("iconfont")
                            .text_color(if visible { accent } else { text_3 })
                            .flex_shrink_0()
                            .child(if visible { "\u{e61f}" } else { "\u{e831}" });

                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .h(px(28.))
                            .px(px(4.))
                            .rounded(px(4.))
                            .cursor(CursorStyle::PointingHand)
                            .hover(|el| el.bg(btn_hover))
                            .on_mouse_down(MouseButton::Left, {
                                let this = this_entity.clone();
                                let k = key_for_toggle.clone();
                                move |_ev, _window, cx| {
                                    cx.stop_propagation();
                                    this.update(cx, |panel, cx| panel.toggle_visible(&k, cx));
                                }
                            })
                            // Checkbox
                            .child(checkbox)
                            // Icon
                            .child(
                                div()
                                    .ml(px(6.))
                                    .text_size(px(12.))
                                    .font_family("iconfont")
                                    .text_color(if visible { text_2 } else { text_3 })
                                    .child(icon.to_string()),
                            )
                            // Label
                            .child(
                                div()
                                    .ml(px(4.))
                                    .text_size(px(11.))
                                    .flex_1()
                                    .text_color(if visible { text_1 } else { text_3 })
                                    .child(label),
                            )
                            // Up arrow
                            .child({
                                let this = this_entity.clone();
                                div()
                                    .w(px(22.))
                                    .h(px(22.))
                                    .rounded(px(4.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(arrow_bg)
                                    .cursor(CursorStyle::PointingHand)
                                    .when(is_first, |el| el.opacity(0.2))
                                    .when(!is_first, |el| {
                                        el.hover(|el| el.bg(arrow_hover))
                                            .on_mouse_down(MouseButton::Left, {
                                                let this = this.clone();
                                                move |_ev, _window, cx| {
                                                    cx.stop_propagation();
                                                    this.update(cx, |panel, cx| {
                                                        panel.move_up(i, cx);
                                                    });
                                                }
                                            })
                                    })
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_family("iconfont")
                                            .text_color(text_2)
                                            .child("\u{e665}"),
                                    )
                            })
                            // Down arrow
                            .child({
                                let this = this_entity.clone();
                                div()
                                    .ml(px(2.))
                                    .w(px(22.))
                                    .h(px(22.))
                                    .rounded(px(4.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(arrow_bg)
                                    .cursor(CursorStyle::PointingHand)
                                    .when(is_last, |el| el.opacity(0.2))
                                    .when(!is_last, |el| {
                                        el.hover(|el| el.bg(arrow_hover))
                                            .on_mouse_down(MouseButton::Left, {
                                                let this = this.clone();
                                                move |_ev, _window, cx| {
                                                    cx.stop_propagation();
                                                    this.update(cx, |panel, cx| {
                                                        panel.move_down(i, cx);
                                                    });
                                                }
                                            })
                                    })
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_family("iconfont")
                                            .text_color(text_2)
                                            .child("\u{e666}"),
                                    )
                            })
                    })),
            )
    }
}
