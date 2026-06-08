//! Hotkey settings tab — recording + blacklist management.
//!
//! Matches the original Slint `SettingsTabHotkey.slint` layout:
//! - Hotkey recording card (66px): label + current hotkey button
//! - Blacklist section: foreground app info bar + scrollable blacklist

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::scroll::ScrollableElement;

use super::SettingsPanel;

/// Confirm action for hotkey blacklist operations.
/// Emitted by SettingsPanel, handled by RootView to show ConfirmDialog.
#[derive(Debug, Clone)]
pub enum HotkeyConfirmAction {
    Add { app_name: String },
    Remove { app_name: String },
}

impl SettingsPanel {
    pub fn render_hotkey_tab(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let wm = self.window_manager.clone();
        let this = cx.entity().clone();

        // Snapshot current values from AppState
        let app = self.state.read(cx);
        let hotkey_display = app.settings.hotkey.clone();
        let recording = app.hotkey_recording;
        let fg_app_name = app.foreground_app_name.clone();
        let fg_window_title = app.foreground_window_title.clone();
        let _fg_icon_base64 = app.foreground_app_icon_base64.clone();
        let blacklist = app.settings.hotkey_blacklist.clone();
        // borrow released — `app` goes out of scope here

        let theme = &self.theme;

        // Recording state colours
        let recording_border = if recording {
            theme.accent
        } else {
            theme.divider
        };
        let recording_btn_bg = if recording {
            theme.accent_soft
        } else {
            theme.accent
        };
        let recording_btn_text = if recording {
            theme.accent
        } else {
            rgb(0xffffff)
        };

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(8.))
            // ── 1. Hotkey recording card (66px) ──
            .child(
                div()
                    .h(px(66.))
                    .rounded(px(10.))
                    .bg(theme.surface)
                    .border(px(1.))
                    .border_color(recording_border)
                    .px(px(14.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    // Left: label + description
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.text_1)
                                    .child("Hotkey"),
                            )
                            .child({
                                let desc_color = if recording {
                                    theme.accent
                                } else {
                                    theme.text_3
                                };
                                let desc_text = if recording {
                                    "Press new hotkey..."
                                } else {
                                    "Click to start recording"
                                };
                                div()
                                    .text_size(px(10.))
                                    .text_color(desc_color)
                                    .child(desc_text.to_string())
                            }),
                    )
                    // Right: hotkey button (80×28)
                    .child({
                        let state = state.clone();
                        let wm = wm.clone();
                        let this = this.clone();
                        div()
                            .h(px(28.))
                            .w(px(80.))
                            .rounded(px(7.))
                            .bg(recording_btn_bg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(!recording, |d| {
                                d.cursor(CursorStyle::PointingHand)
                                    .hover(move |style| style.opacity(0.85))
                            })
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                if recording {
                                    return;
                                }
                                // Start recording via WindowManager
                                wm.update(cx, |wm, _cx| {
                                    wm.start_hotkey_recording();
                                });
                                state.update(cx, |s, _cx| {
                                    s.hotkey_recording = true;
                                });
                                let _ = this.update(cx, |_panel, cx| cx.notify());
                            })
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(recording_btn_text)
                                    .child(hotkey_display.clone()),
                            )
                    }),
            )
            // ── 2. Blacklist section ──
            .child(
                div()
                    .rounded(px(10.))
                    .bg(theme.surface)
                    .border(px(1.))
                    .border_color(theme.divider)
                    .flex()
                    .flex_col()
                    .gap(px(0.))
                    // ── 2a. Foreground app info bar (44px) ──
                    .child({
                        let has_app = !fg_app_name.is_empty();
                        let icon_path = if has_app {
                            Some(crate::core::paths::app_icon_path(&fg_app_name))
                        } else {
                            None
                        };

                        div()
                            .h(px(44.))
                            .rounded(px(10.))
                            .bg(theme.titlebar_bg)
                            .px(px(12.))
                            .pr(px(6.))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            // Left: icon + app name + window title
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap(px(8.))
                                    .items_center()
                                    .flex_1()
                                    .overflow_hidden()
                                    // App icon (20×20)
                                    .when(has_app, |d| {
                                        if let Some(ref path) = icon_path {
                                            d.child(
                                                gpui::img(std::path::Path::new(path))
                                                    .w(px(20.))
                                                    .h(px(20.)),
                                            )
                                        } else {
                                            d
                                        }
                                    })
                                    // App name + window title
                                    .when(has_app, |d| {
                                        d.child(
                                            div()
                                                .flex()
                                                .flex_row()
                                                .gap(px(0.))
                                                .items_center()
                                                .overflow_hidden()
                                                .flex_1()
                                                .child(
                                                    div()
                                                        .text_size(px(12.))
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(theme.text_1)
                                                        .child(fg_app_name.clone()),
                                                )
                                                .when(!fg_window_title.is_empty(), |d| {
                                                    d.child(
                                                        div()
                                                            .text_size(px(11.))
                                                            .text_color(theme.text_3)
                                                            .overflow_hidden()
                                                            .text_ellipsis()
                                                            .flex_1()
                                                            .child(format!(
                                                                " \u{2014} {}",
                                                                fg_window_title
                                                            )),
                                                    )
                                                }),
                                        )
                                    })
                                    // No foreground app
                                    .when(!has_app, |d| {
                                        d.child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(theme.text_3)
                                                .child("No foreground app"),
                                        )
                                    }),
                            )
                            // Right: block button (26×26)
                            .when(has_app, |d| {
                                let app_name = fg_app_name.clone();
                                let this = this.clone();
                                d.child(
                                    div()
                                        .w(px(26.))
                                        .h(px(26.))
                                        .rounded(px(6.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor(CursorStyle::PointingHand)
                                        .hover(|style| style.bg(theme.danger).opacity(0.12))
                                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                            let _ = this.update(cx, |_panel, cx| {
                                                cx.emit(super::SettingsEvent::ShowHotkeyConfirm(
                                                    HotkeyConfirmAction::Add {
                                                        app_name: app_name.clone(),
                                                    },
                                                ));
                                            });
                                        })
                                        .child(
                                            div()
                                                .font_family("iconfont")
                                                .text_size(px(14.))
                                                .text_color(theme.text_2)
                                                .child("\u{e6a7}"),
                                        ),
                                )
                            })
                    })
                    // ── Divider (only when blacklist is non-empty) ──
                    .when(!blacklist.is_empty(), |d| {
                        d.child(
                            div()
                                .w_full()
                                .h(px(1.))
                                .bg(theme.divider),
                        )
                    })
                    // ── 2b. Blacklist entries (scrollable, 160px max) ──
                    .when(!blacklist.is_empty(), |d| {
                        d.child(
                            div()
                                .max_h(px(160.))
                                .w_full()
                                .overflow_y_scrollbar()
                                .p(px(8.))
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .children(blacklist.iter().map(|app_name| {
                                    let icon_path =
                                        crate::core::paths::app_icon_path(app_name);
                                    let name = app_name.clone();
                                    let this = this.clone();

                                    div()
                                        .h(px(36.))
                                        .rounded(px(8.))
                                        .bg(theme.titlebar_bg)
                                        .border(px(1.))
                                        .border_color(theme.divider)
                                        .px(px(10.))
                                        .pr(px(6.))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .justify_between()
                                        // Left: icon + app name
                                        .child(
                                            div()
                                                .flex()
                                                .flex_row()
                                                .gap(px(8.))
                                                .items_center()
                                                .overflow_hidden()
                                                .flex_1()
                                                .child(
                                                    gpui::img(std::path::Path::new(
                                                        &icon_path,
                                                    ))
                                                    .w(px(20.))
                                                    .h(px(20.)),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(12.))
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(theme.text_1)
                                                        .overflow_hidden()
                                                        .text_ellipsis()
                                                        .child(name.clone()),
                                                ),
                                        )
                                        // Right: delete button (24×24)
                                        .child(
                                            div()
                                                .w(px(24.))
                                                .h(px(24.))
                                                .rounded(px(5.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .cursor(CursorStyle::PointingHand)
                                                .hover(|style| {
                                                    style.bg(theme.danger).opacity(0.12)
                                                })
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    {
                                                        let name = name.clone();
                                                        let this = this.clone();
                                                        move |_ev, _window, cx| {
                                                            let _ = this.update(
                                                                cx,
                                                                |_panel, cx| {
                                                                    cx.emit(super::SettingsEvent::ShowHotkeyConfirm(
                                                                        HotkeyConfirmAction::Remove {
                                                                            app_name: name
                                                                                .clone(),
                                                                        },
                                                                    ));
                                                                },
                                                            );
                                                        }
                                                    },
                                                )
                                                .child(
                                                    div()
                                                        .font_family("iconfont")
                                                        .text_size(px(14.))
                                                        .text_color(theme.text_2)
                                                        .child("\u{e8b6}"),
                                                ),
                                        )
                                })),
                        )
                    })
                    // ── 2c. Empty state ──
                    .when(blacklist.is_empty(), |d| {
                        d.child(
                            div()
                                .h(px(40.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child({
                                    let msg = if fg_app_name.is_empty() {
                                        "No blacklisted apps"
                                    } else {
                                        "No blacklisted apps. Click  to add current app"
                                    };
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme.text_3)
                                        .child(msg.to_string())
                                }),
                        )
                    }),
            )
    }
}
