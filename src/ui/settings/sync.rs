//! Sync settings tab.

use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::scroll::ScrollableElement;
use gpui_transitions::WindowUseTransition;

use crate::services::gpui_sync::format_last_sync;
use crate::state::sync::BackendStatus;
use crate::ui::components::toggle::render_toggle;

use super::{BackendCollapseState, SettingsPanel};

const COLLAPSE_DURATION: Duration = Duration::from_millis(300);

impl SettingsPanel {
    pub fn render_sync_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sync = self.state.read(cx).sync.clone();
        let wm = self.window_manager.clone();
        let backend_panel = self.backend_panel();
        let surface = self.theme.surface;
        let divider = self.theme.divider;
        let accent = self.theme.accent;
        let accent_soft = self.theme.accent_soft;
        let text_1 = self.theme.text_1;
        let text_2 = self.theme.text_2;

        let backend_cards: Vec<AnyElement> = sync
            .backends
            .iter()
            .map(|backend| {
                self.render_backend_card(backend, window, cx)
                    .into_any_element()
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(8.))
            .child(
                div()
                    .rounded(px(10.))
                    .bg(surface)
                    .border(px(1.))
                    .border_color(divider)
                    .overflow_hidden()
                    .child(
                        div()
                            .h(px(38.))
                            .px(px(14.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(text_1)
                                    .child("Sync"),
                            )
                            .child(render_toggle(
                                sync.auto_enabled,
                                "sync-auto-enabled",
                                accent,
                                divider,
                                &mut self.toggle_states,
                                window,
                                cx,
                                {
                                    let wm = wm.clone();
                                    move |_window, cx| {
                                        wm.update(cx, |wm, cx| {
                                            wm.toggle_sync_auto_enabled(cx);
                                        });
                                    }
                                },
                            )),
                    )
                    .when(sync.auto_enabled, |card| {
                        card.child(div().h(px(1.)).bg(divider)).child(
                            div()
                                .h(px(38.))
                                .px(px(14.))
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(text_1)
                                        .child("Favorites only"),
                                )
                                .child(render_toggle(
                                    sync.favorites_only,
                                    "sync-favorites-only",
                                    accent,
                                    divider,
                                    &mut self.toggle_states,
                                    window,
                                    cx,
                                    {
                                        let wm = wm.clone();
                                        move |_window, cx| {
                                            wm.update(cx, |wm, cx| {
                                                wm.toggle_sync_favorites_only(cx);
                                            });
                                        }
                                    },
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .rounded(px(10.))
                    .bg(surface)
                    .border(px(1.))
                    .border_color(divider)
                    .overflow_hidden()
                    .child(
                        div()
                            .h(px(40.))
                            .px(px(14.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .cursor(CursorStyle::PointingHand)
                            .hover(move |style| style.bg(accent_soft))
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                let _ = backend_panel.update(cx, |panel, cx| {
                                    panel.open_add(_window, cx);
                                });
                            })
                            .child(
                                div()
                                    .font_family("iconfont")
                                    .text_size(px(13.))
                                    .text_color(accent)
                                    .child("\u{e6df}"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(text_2)
                                    .child("Add backend"),
                            ),
                    )
                    .when(!backend_cards.is_empty(), |card| {
                        card.child(div().h(px(1.)).bg(divider)).child(
                            div()
                                .max_h(px(270.))
                                .overflow_y_scrollbar()
                                .p(px(8.))
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .children(backend_cards),
                        )
                    }),
            )
    }

    fn render_backend_card(
        &mut self,
        backend: &BackendStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = backend.config.id.clone();
        let enabled = backend.config.enabled;
        let (previous, generation, changed) = match self.backend_collapse_states.get_mut(&id) {
            Some(state) => {
                let previous = state.enabled;
                let changed = previous != enabled;
                if changed {
                    state.enabled = enabled;
                    state.generation = state.generation.wrapping_add(1);
                }
                (previous, state.generation, changed)
            }
            None => {
                self.backend_collapse_states.insert(
                    id.clone(),
                    BackendCollapseState {
                        enabled,
                        generation: 0,
                    },
                );
                (enabled, 0, false)
            }
        };
        let key = hash_key(&id).wrapping_add(generation << 32);
        let height = transition_f32(
            window,
            cx,
            ("sync-backend-height", key),
            if changed {
                card_height(previous)
            } else {
                card_height(enabled)
            },
            card_height(enabled),
        );
        let footer_opacity = transition_f32(
            window,
            cx,
            ("sync-backend-footer-opacity", key),
            if changed {
                bool_f32(previous)
            } else {
                bool_f32(enabled)
            },
            bool_f32(enabled),
        );
        let content_opacity = transition_f32(
            window,
            cx,
            ("sync-backend-content-opacity", key),
            if changed {
                main_opacity(previous)
            } else {
                main_opacity(enabled)
            },
            main_opacity(enabled),
        );

        let surface = self.theme.titlebar_bg;
        let divider = self.theme.divider;
        let accent = self.theme.accent;
        let accent_soft = self.theme.accent_soft;
        let text_1 = self.theme.text_1;
        let text_2 = self.theme.text_2;
        let text_3 = self.theme.text_3;
        let danger = self.theme.danger;
        let wm = self.window_manager.clone();
        let backend_panel = self.backend_panel();
        let status_color = match backend.status.as_str() {
            "online" => rgb(0x4caf50),
            "syncing" => accent,
            "error" => danger,
            _ => rgb(0x9e9e9e),
        };
        let interval = backend.config.sync_interval_secs.unwrap_or_else(|| {
            if backend.config.backend_type == "webdav" {
                600
            } else {
                60
            }
        });
        let stats = format!(
            "{} · {} items · {} tags",
            format_last_sync(&backend.config.last_sync_at),
            backend.config.last_item_count,
            backend.config.last_tag_count
        );

        div()
            .h(px(height))
            .rounded(px(8.))
            .border(px(1.))
            .border_color(divider)
            .bg(surface)
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(52.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .opacity(content_opacity)
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .w(px(12.))
                                            .h(px(12.))
                                            .rounded(px(6.))
                                            .border(px(1.))
                                            .border_color(status_color)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                div()
                                                    .w(px(5.))
                                                    .h(px(5.))
                                                    .rounded(px(3.))
                                                    .bg(status_color),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .max_w(px(110.))
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .text_size(px(12.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(text_1)
                                            .child(backend.config.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .h(px(16.))
                                            .px(px(5.))
                                            .rounded(px(3.))
                                            .bg(accent_soft)
                                            .text_size(px(10.))
                                            .text_color(accent)
                                            .flex()
                                            .items_center()
                                            .child(backend.service_label.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(text_3)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(stats),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(
                                div()
                                    .opacity(content_opacity)
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(icon_button("\u{e648}", text_3, accent, {
                                        let config = backend.config.clone();
                                        let backend_panel = backend_panel.clone();
                                        move |window, cx| {
                                            let _ = backend_panel.update(cx, |panel, cx| {
                                                panel.open_edit(&config, window, cx);
                                            });
                                        }
                                    }))
                                    .child(icon_button("\u{e8b6}", text_3, danger, {
                                        let id = id.clone();
                                        let wm = wm.clone();
                                        move |_window, cx| {
                                            wm.update(cx, |wm, cx| {
                                                wm.remove_sync_backend(&id, cx);
                                            });
                                        }
                                    })),
                            )
                            .child(render_toggle(
                                enabled,
                                &format!("sync-backend-{id}"),
                                accent,
                                divider,
                                &mut self.toggle_states,
                                window,
                                cx,
                                {
                                    let id = id.clone();
                                    let wm = wm.clone();
                                    move |_window, cx| {
                                        wm.update(cx, |wm, cx| {
                                            wm.toggle_sync_backend(&id, cx);
                                        });
                                    }
                                },
                            )),
                    ),
            )
            .child(
                div()
                    .h(px(31.))
                    .opacity(footer_opacity)
                    .border_t(px(1.))
                    .border_color(divider)
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .children(
                        [(30, "30s"), (60, "1m"), (600, "10m"), (1800, "30m")]
                            .into_iter()
                            .map(|(secs, label)| {
                                let selected = interval == secs;
                                let id = id.clone();
                                let wm = wm.clone();
                                div()
                                    .flex_1()
                                    .h(px(20.))
                                    .rounded(px(6.))
                                    .bg(if selected { accent } else { rgba(0x00000000) })
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(if selected { rgb(0xffffff) } else { text_2 })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(move |style| {
                                        if selected {
                                            style.opacity(0.88)
                                        } else {
                                            style.bg(accent_soft)
                                        }
                                    })
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                        wm.update(cx, |wm, cx| {
                                            wm.set_backend_sync_interval(&id, secs, cx);
                                        });
                                    })
                                    .child(label)
                            }),
                    )
                    .child(
                        div()
                            .w(px(62.))
                            .h(px(20.))
                            .rounded(px(6.))
                            .bg(accent)
                            .text_size(px(10.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0xffffff))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(|style| style.opacity(0.85))
                            .on_mouse_down(MouseButton::Left, {
                                let id = id.clone();
                                let wm = wm.clone();
                                move |_ev, _window, cx| {
                                    wm.update(cx, |wm, cx| {
                                        wm.sync_backend_now(&id, cx);
                                    });
                                }
                            })
                            .child(if backend.syncing {
                                "Syncing"
                            } else {
                                "Sync now"
                            }),
                    ),
            )
    }
}

fn icon_button(
    icon: &'static str,
    color: Rgba,
    hover_color: Rgba,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .w(px(24.))
        .h(px(24.))
        .rounded(px(5.))
        .font_family("iconfont")
        .text_size(px(12.))
        .text_color(color)
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::PointingHand)
        .hover(move |style| style.text_color(hover_color).bg(rgba(0xffffff0d)))
        .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            on_click(window, cx);
        })
        .child(icon)
}

fn transition_f32(
    window: &mut Window,
    cx: &mut App,
    key: (&'static str, u64),
    initial: f32,
    target: f32,
) -> f32 {
    let transition = window
        .use_keyed_transition(key, cx, COLLAPSE_DURATION, move |_, _| initial)
        .with_easing(ease_out);
    transition.update(cx, |value, cx| {
        *value = target;
        cx.notify();
    });
    let value = *transition.evaluate(window, cx);
    value
}

fn hash_key(value: &str) -> u64 {
    value.bytes().fold(0u64, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as u64)
    })
}

fn card_height(enabled: bool) -> f32 {
    if enabled {
        83.0
    } else {
        52.0
    }
}

fn bool_f32(value: bool) -> f32 {
    if value {
        1.0
    } else {
        0.0
    }
}

fn main_opacity(enabled: bool) -> f32 {
    if enabled {
        1.0
    } else {
        0.45
    }
}

fn ease_out(delta: f32) -> f32 {
    1.0 - (1.0 - delta).powi(3)
}
