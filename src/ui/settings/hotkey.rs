//! Hotkey settings tab — recording + blacklist management + paste shortcuts.
//!
//! --- Layout (top to bottom): ---
//! --- - Hotkey recording card (66px): label + current hotkey button ---
//! --- - Foreground app info bar (44px): icon + app name + title + paste/blacklist buttons ---
//! --- - Blacklist section: label + scrollable list box ---
//! --- - Paste shortcut section: label + scrollable list box ---

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::scroll::ScrollableElement;
use gpui_component::tooltip::Tooltip;

use crate::core::i18n_keys::I18nKey;
use crate::ui::theme::ClippiTheme;
use super::SettingsPanel;

/// Confirm action for hotkey settings operations.
/// Emitted by SettingsPanel, handled by RootView to show ConfirmDialog.
#[derive(Debug, Clone)]
pub enum HotkeyConfirmAction {
    /// Add app to hotkey blacklist.
    AddBlacklist { app_name: String },
    /// Remove app from hotkey blacklist.
    RemoveBlacklist { app_name: String },
    /// Add/update paste shortcut for an app.
    AddPasteShortcut { app_name: String, shortcut: String },
    /// Remove paste shortcut for an app.
    RemovePasteShortcut { app_name: String },
}

/// GPUI callback type alias for per-app list entries.
type AppCallback = Rc<dyn Fn(&mut Window, &mut App)>;

/// Entry in a per-app list (blacklist or paste shortcut).
struct AppListEntry {
    app_name: String,
    /// For paste shortcut entries, the recorded shortcut string.
    shortcut: Option<String>,
    /// Delete callback: emits remove event.
    on_delete: AppCallback,
    /// For paste shortcut entries: callback when shortcut label is clicked (re-record).
    on_shortcut_click: Option<AppCallback>,
    /// Cancel callback shown when this entry is the recording target.
    on_cancel_recording: Option<AppCallback>,
    /// Whether this entry is currently the recording target (accent border).
    is_recording_target: bool,
}

impl AppListEntry {
    fn render(&self, theme: &ClippiTheme) -> impl IntoElement {
        let icon_path = crate::core::paths::app_icon_path(&self.app_name);

        let entry_border = if self.is_recording_target { theme.accent } else { theme.divider };
        div()
            .h(px(32.))
            .rounded(px(6.))
            .bg(theme.titlebar_bg)
            .border(px(1.))
            .border_color(entry_border)
            .px(px(8.))
            .pr(px(4.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            // Left: icon + app name
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(6.))
                    .items_center()
                    .overflow_hidden()
                    .flex_1()
                    .child(
                        gpui::img(std::path::Path::new(&icon_path))
                            .w(px(18.))
                            .h(px(18.)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.text_1)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(self.app_name.clone()),
                    ),
            )
            // Right: shortcut label or cancel button (when recording) + delete button
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(4.))
                    .items_center()
                    // Recording cancel button (replaces shortcut label)
                    .when(self.is_recording_target, |d| {
                        let on_cancel = self.on_cancel_recording.clone();
                        d.child(
                            div()
                                .id("entry-cancel-btn")
                                .w(px(22.))
                                .h(px(22.))
                                .rounded(px(5.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor(CursorStyle::PointingHand)
                                .hover(|style| style.bg(theme.danger).opacity(0.12))
                                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                    if let Some(ref cb) = on_cancel { cb(window, cx); }
                                })
                                .child(
                                    div()
                                        .font_family("iconfont")
                                        .text_size(px(13.))
                                        .text_color(theme.danger)
                                        .child("\u{e7b7}"),
                                ),
                        )
                    })
                    // Shortcut label (not recording)
                    .when(!self.is_recording_target, |d| {
                        d.when_some(self.shortcut.as_ref(), |d, sc| {
                            let sc = sc.clone();
                            let on_click = self.on_shortcut_click.clone();
                            d.child(
                                div()
                                    .h(px(22.))
                                    .rounded(px(5.))
                                    .px(px(6.))
                                    .bg(theme.accent_soft)
                                    .flex()
                                    .items_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(|style| style.opacity(0.8))
                                    .when_some(on_click, |d, cb| {
                                        d.on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                            cb(window, cx);
                                        })
                                    })
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.accent)
                                            .child(sc),
                                    ),
                            )
                        })
                    })
                    .child({
                        let on_delete = self.on_delete.clone();
                        div()
                            .w(px(24.))
                            .h(px(24.))
                            .rounded(px(5.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(|style| style.bg(theme.danger).opacity(0.12))
                            .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                on_delete(window, cx);
                            })
                            .child(
                                div()
                                    .font_family("iconfont")
                                    .text_size(px(14.))
                                    .text_color(theme.text_2)
                                    .child("\u{e8b6}"),
                            )
                    }),
            )
    }
}

impl SettingsPanel {
    /// Render the shared foreground app info bar.
    ///
    /// Layout: [app icon] AppName — WindowTitle [⊞ paste btn] [⊘ blacklist btn]
    #[allow(clippy::too_many_arguments)]
    fn render_foreground_app_bar(
        fg_app_name: &str,
        fg_window_title: &str,
        theme: &ClippiTheme,
        has_app: bool,
        is_recording_paste: bool,
        on_paste_shortcut: impl Fn(&mut Window, &mut App) + 'static,
        on_cancel_recording: impl Fn(&mut Window, &mut App) + 'static,
        on_blacklist: impl Fn(&mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let icon_path = if has_app {
            Some(crate::core::paths::app_icon_path(fg_app_name))
        } else {
            None
        };
        let card_h = if is_recording_paste { px(52.) } else { px(44.) };
        let card_border = if is_recording_paste { theme.accent } else { theme.divider };

        div()
            .h(card_h)
            .rounded(px(10.))
            .bg(theme.surface)
            .border(px(1.))
            .border_color(card_border)
            .px(px(12.))
            .pr(px(6.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            // Left: icon + text block (flex_col when recording for second line)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(8.))
                    .items_center()
                    .flex_1()
                    .overflow_hidden()
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
                    .when(has_app, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(1.))
                                .overflow_hidden()
                                .flex_1()
                                // Row 1: app name + window title
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .gap(px(0.))
                                        .items_center()
                                        .overflow_hidden()
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(theme.text_1)
                                                .child(fg_app_name.to_string()),
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
                                // Row 2: recording hint (only when recording)
                                .when(is_recording_paste, |d| {
                                    d.child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(theme.accent)
                                            .child(I18nKey::HotkeyPasteShortcutRecording.text()),
                                    )
                                }),
                        )
                    })
                    .when(!has_app, |d| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.text_3)
                                .child(I18nKey::HotkeyNoForeground.text()),
                        )
                    }),
            )
            // Right: buttons
            .when(has_app, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(2.))
                        .items_center()
                        // Cancel button (replaces paste shortcut button when recording)
                        .when(is_recording_paste, |buttons| {
                            let on_cancel = Rc::new(on_cancel_recording);
                            buttons.child(
                                div()
                                    .id("cancel-recording-btn")
                                    .w(px(22.))
                                    .h(px(22.))
                                    .rounded(px(5.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(|style| style.bg(theme.danger).opacity(0.12))
                                    .on_mouse_down(MouseButton::Left, {
                                        let on_cancel = on_cancel.clone();
                                        move |_ev, window, cx| on_cancel(window, cx)
                                    })
                                    .tooltip(move |window, cx| {
                                        Tooltip::element(move |_window, _cx| {
                                            div()
                                                .text_size(px(10.))
                                                .child(I18nKey::BtnCancel.text())
                                        })
                                        .build(window, cx)
                                    })
                                    .child(
                                        div()
                                            .font_family("iconfont")
                                            .text_size(px(13.))
                                            .text_color(theme.danger)
                                            .child("\u{e7b7}"),
                                    ),
                            )
                        })
                        // Paste shortcut button (Windows only; macOS uses Cmd+V consistently).
                        .when(cfg!(target_os = "windows") && !is_recording_paste, |buttons| {
                            let on_ps = Rc::new(on_paste_shortcut);
                            let ps_color = theme.text_2;
                            buttons.child(
                                div()
                                    .id("paste-shortcut-btn")
                                    .w(px(22.))
                                    .h(px(22.))
                                    .rounded(px(5.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(|style| style.bg(theme.accent).opacity(0.12))
                                    .on_mouse_down(MouseButton::Left, {
                                        let on_ps = on_ps.clone();
                                        move |_ev, window, cx| on_ps(window, cx)
                                    })
                                .tooltip(move |window, cx| {
                                    Tooltip::element(move |_window, _cx| {
                                        div()
                                            .text_size(px(10.))
                                            .child(I18nKey::HotkeyPasteShortcutEmptyHint.text())
                                    })
                                    .build(window, cx)
                                })
                                .child(
                                    div()
                                        .font_family("iconfont")
                                        .text_size(px(13.))
                                        .text_color(ps_color)
                                        .child("\u{e66b}"),
                                )
                            )
                        })
                        // Blacklist button
                        .child({
                            let on_bl = Rc::new(on_blacklist);
                            div()
                                .id("blacklist-btn")
                                .w(px(22.))
                                .h(px(22.))
                                .rounded(px(5.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor(CursorStyle::PointingHand)
                                .hover(|style| style.bg(theme.danger).opacity(0.12))
                                .on_mouse_down(MouseButton::Left, {
                                    let on_bl = on_bl.clone();
                                    move |_ev, window, cx| on_bl(window, cx)
                                })
                                .tooltip(move |window, cx| {
                                    Tooltip::element(move |_window, _cx| {
                                        div()
                                            .text_size(px(10.))
                                            .child(I18nKey::HotkeyBlacklistEmptyHint.text())
                                    })
                                    .build(window, cx)
                                })
                                .child(
                                    div()
                                        .font_family("iconfont")
                                        .text_size(px(13.))
                                        .text_color(theme.text_2)
                                        .child("\u{e6a7}"),
                                )
                        }),
                )
            })
    }

    /// Render a labeled list section with dynamic-height scrollable list box.
    fn render_per_app_list_section(
        title: &str,
        empty_hint: &str,
        entries: &[AppListEntry],
        theme: &ClippiTheme,
    ) -> impl IntoElement {
        let has_entries = !entries.is_empty();
        let list_height = if has_entries {
            (entries.len() as f32 * 38.0 + 8.0).min(160.0)
        } else {
            40.0
        };

        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            // Section label
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_3)
                    .px(px(2.))
                    .child(title.to_string()),
            )
            // List box
            .child(
                div()
                    .h(px(list_height))
                    .rounded(px(8.))
                    .bg(theme.surface)
                    .border(px(1.))
                    .border_color(theme.divider)
                    .overflow_y_scrollbar()
                    .when(has_entries, |d| {
                        d.child(
                            div()
                                .p(px(4.))
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .children(entries.iter().map(|e| e.render(theme))),
                        )
                    })
                    .when(!has_entries, |d| {
                        d.child(
                            div()
                                .h(px(list_height))
                                .w_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme.text_3)
                                        .child(empty_hint.to_string()),
                                ),
                        )
                    }),
            )
    }

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
        let is_recording_paste = self.recording_paste_shortcut.is_some();
        let fg_app_name = app.foreground_app_name.clone();
        let fg_window_title = app.foreground_window_title.clone();
        let blacklist = app.settings.hotkey_blacklist.clone();
        let paste_shortcuts = app.settings.paste_shortcuts.clone();
        // borrow released

        let theme = &self.theme;
        let has_fg = !fg_app_name.is_empty();

        // Recording state colors — only for global hotkey (paste shortcut
        // recording has its own dedicated panel below the foreground app bar).
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
            // 1. Hotkey recording card (66px)
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
                                    .child(I18nKey::HotkeyTabTitle.text()),
                            )
                            .child({
                                let desc_color = if recording {
                                    theme.accent
                                } else {
                                    theme.text_3
                                };
                                let desc_text = if recording {
                                    I18nKey::HotkeyPressToRecord.text()
                                } else {
                                    I18nKey::HotkeyRecordingIdle.text()
                                };
                                div()
                                    .text_size(px(10.))
                                    .text_color(desc_color)
                                    .child(desc_text)
                            }),
                    )
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
                                wm.update(cx, |wm, _cx| {
                                    wm.start_hotkey_recording();
                                });
                                state.update(cx, |s, _cx| {
                                    s.hotkey_recording = true;
                                });
                                this.update(cx, |_panel, cx| cx.notify());
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
            // 2. Shared foreground app info bar (always shows current foreground)
            .child({
                // When recording was triggered from the bar button, show recording
                // effect on the bar itself (matches foreground app).
                let recording_on_bar = is_recording_paste
                    && self.recording_paste_shortcut.as_deref()
                        .map(|a| a.eq_ignore_ascii_case(&fg_app_name))
                        .unwrap_or(false);
                let app_name_ps = fg_app_name.clone();
                let app_name_bl = fg_app_name.clone();
                let this_ps = this.clone();
                let this_cancel = this.clone();
                let this_bl = this.clone();
                let wm_cancel = wm.clone();
                Self::render_foreground_app_bar(
                    &fg_app_name,
                    &fg_window_title,
                    theme,
                    has_fg,
                    recording_on_bar,
                    // on_paste_shortcut: start paste shortcut recording
                    move |_window, cx| {
                        this_ps.update(cx, |panel, cx| {
                            panel.recording_paste_shortcut = Some(app_name_ps.clone());
                            panel.window_manager.update(cx, |wm, _cx| {
                                wm.start_paste_shortcut_recording(app_name_ps.clone());
                            });
                            cx.notify();
                        });
                    },
                    // on_cancel_recording
                    move |_window, cx| {
                        wm_cancel.update(cx, |wm, _cx| {
                            wm.cancel_paste_shortcut_recording();
                        });
                        this_cancel.update(cx, |panel, cx| {
                            panel.clear_paste_shortcut_state(cx);
                        });
                    },
                    // on_blacklist: emit confirm dialog
                    move |_window, cx| {
                        this_bl.update(cx, |_panel, cx| {
                            cx.emit(super::SettingsEvent::ShowHotkeyConfirm(
                                HotkeyConfirmAction::AddBlacklist {
                                    app_name: app_name_bl.clone(),
                                },
                            ));
                        });
                    },
                )
            })
            // 3. Blacklist section
            .child({
                let this = this.clone();
                let entries: Vec<AppListEntry> = blacklist
                    .iter()
                    .map(|name| {
                        let name_clone = name.clone();
                        let this_clone = this.clone();
                        AppListEntry {
                            app_name: name.clone(),
                            shortcut: None,
                            on_cancel_recording: None,
                            is_recording_target: false,
                            on_delete: Rc::new(move |_window, cx| {
                                this_clone.update(cx, |_panel, cx| {
                                    cx.emit(super::SettingsEvent::ShowHotkeyConfirm(
                                        HotkeyConfirmAction::RemoveBlacklist {
                                            app_name: name_clone.clone(),
                                        },
                                    ));
                                });
                            }),
                            on_shortcut_click: None,
                        }
                    })
                    .collect();
                let blacklist_title = I18nKey::HotkeyBlacklist.text();
                let blacklist_hint = I18nKey::HotkeyBlacklistEmptyHint.text();
                #[allow(clippy::needless_borrow)]
                Self::render_per_app_list_section(
                    &blacklist_title,
                    &blacklist_hint,
                    &entries,
                    theme,
                )
            })
            // 4. Paste shortcut section (Windows only)
            .when(cfg!(target_os = "windows"), |root| root.child({
                let this = this.clone();
                let wm_ps = wm.clone();
                let this_ps_cancel = this.clone();
                let entries: Vec<AppListEntry> = paste_shortcuts
                    .iter()
                    .map(|entry| {
                        let name = entry.app_name.clone();
                        let sc = entry.shortcut.clone();
                        let this_del = this.clone();
                        let this_re = this.clone();
                        let name_re = name.clone();
                        let is_target = is_recording_paste
                            && self.recording_paste_shortcut.as_deref()
                                .map(|a| a.eq_ignore_ascii_case(&name))
                                .unwrap_or(false);
                        let wm_c = wm_ps.clone();
                        let this_c = this_ps_cancel.clone();
                        AppListEntry {
                            app_name: name.clone(),
                            shortcut: Some(sc.clone()),
                            is_recording_target: is_target,
                            on_cancel_recording: if is_target {
                                Some(Rc::new(move |_window, cx| {
                                    wm_c.update(cx, |wm, _cx| {
                                        wm.cancel_paste_shortcut_recording();
                                    });
                                    this_c.update(cx, |panel, cx| {
                                        panel.clear_paste_shortcut_state(cx);
                                    });
                                }))
                            } else {
                                None
                            },
                            on_delete: Rc::new(move |_window, cx| {
                                this_del.update(cx, |_panel, cx| {
                                    cx.emit(super::SettingsEvent::ShowHotkeyConfirm(
                                        HotkeyConfirmAction::RemovePasteShortcut {
                                            app_name: name.clone(),
                                        },
                                    ));
                                });
                            }),
                            on_shortcut_click: Some(Rc::new(move |_window, cx| {
                                this_re.update(cx, |panel, cx| {
                                    panel.recording_paste_shortcut = Some(name_re.clone());
                                    panel.window_manager.update(cx, |wm, _cx| {
                                        wm.start_paste_shortcut_recording(name_re.clone());
                                    });
                                    cx.notify();
                                });
                            })),
                        }
                    })
                    .collect();
                let ps_title = I18nKey::HotkeyPasteShortcut.text();
                let ps_hint = I18nKey::HotkeyPasteShortcutEmptyHint.text();
                #[allow(clippy::needless_borrow)]
                Self::render_per_app_list_section(
                    &ps_title,
                    &ps_hint,
                    &entries,
                    theme,
                )
            }))
    }
}
