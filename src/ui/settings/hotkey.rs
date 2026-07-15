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

use super::SettingsPanel;
use crate::core::i18n_keys::I18nKey;
use crate::core::settings::LatestHotkeyEntry;
use crate::state::app::AppState;
use crate::ui::theme::ClippiTheme;
use crate::ui::window_manager::WindowManager;

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

        let entry_border = if self.is_recording_target {
            theme.accent
        } else {
            theme.divider
        };
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
                                    if let Some(ref cb) = on_cancel {
                                        cb(window, cx);
                                    }
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
                                        d.on_mouse_down(
                                            MouseButton::Left,
                                            move |_ev, window, cx| {
                                                cb(window, cx);
                                            },
                                        )
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
        let card_border = if is_recording_paste {
            theme.accent
        } else {
            theme.divider
        };

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
                            d.child(gpui::img(std::path::Path::new(path)).w(px(20.)).h(px(20.)))
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
                        .when(
                            cfg!(target_os = "windows") && !is_recording_paste,
                            |buttons| {
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
                                                div().text_size(px(10.)).child(
                                                    I18nKey::HotkeyPasteShortcutEmptyHint.text(),
                                                )
                                            })
                                            .build(window, cx)
                                        })
                                        .child(
                                            div()
                                                .font_family("iconfont")
                                                .text_size(px(13.))
                                                .text_color(ps_color)
                                                .child("\u{e66b}"),
                                        ),
                                )
                            },
                        )
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

    /// Shared recording card: title + description + record button.
    /// Used for both the main hotkey and quick hotkey cards.
    fn render_recording_card(
        title: I18nKey,
        hotkey_display: SharedString,
        recording: bool,
        theme: &ClippiTheme,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
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
                            .child(title.text()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(desc_color)
                            .child(desc_text),
                    ),
            )
            .child(
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
                    .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                        if recording {
                            return;
                        }
                        on_click(window, cx);
                    })
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(recording_btn_text)
                            .child(hotkey_display),
                    ),
            )
    }

    /// Render a single latest-hotkey slot cell in the popup grid.
    fn render_latest_slot_cell(
        index: usize,
        latest: &[LatestHotkeyEntry],
        theme: ClippiTheme,
        on_record: AppCallback,
        on_clear: Option<AppCallback>,
    ) -> impl IntoElement {
        let entry = latest.get(index);
        let hotkey = entry.map(|e| e.hotkey.as_str()).unwrap_or("");
        let has_hotkey = entry.is_some_and(|e| !e.hotkey.is_empty());

        let label = if has_hotkey {
            hotkey.to_string()
        } else {
            I18nKey::LatestHotkeyClickRecord.text().to_string()
        };
        let text_color = if has_hotkey {
            theme.accent
        } else {
            theme.text_3
        };
        let border_color = if has_hotkey {
            theme.accent
        } else {
            theme.divider
        };
        let bg_color = if has_hotkey {
            theme.accent_soft
        } else {
            theme.surface
        };
        let badge_bg = if has_hotkey {
            theme.accent
        } else {
            theme.titlebar_bg
        };
        let badge_text = if has_hotkey {
            rgb(0xffffff)
        } else {
            theme.text_3
        };
        let hover_bg = if has_hotkey {
            theme.accent_soft
        } else {
            theme.btn_hover
        };
        let danger = theme.danger;
        let rec = on_record.clone();

        div()
            .flex_1()
            .min_w(px(0.))
            .h(px(44.))
            .rounded(px(7.))
            .bg(bg_color)
            .border(px(1.))
            .border_color(border_color)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .px(px(9.))
            .cursor(CursorStyle::PointingHand)
            .hover(move |style| style.bg(hover_bg))
            .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                rec(window, cx);
            })
            .child(
                div()
                    .w(px(20.))
                    .h(px(20.))
                    .rounded(px(10.))
                    .bg(badge_bg)
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(badge_text)
                            .child(format!("{}", index + 1)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(px(11.))
                    .font_weight(if has_hotkey {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(text_color)
                    .child(label),
            )
            .when_some(on_clear.filter(|_| has_hotkey), move |d, clr| {
                d.child(
                    div()
                        .w(px(22.))
                        .h(px(22.))
                        .rounded(px(5.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor(CursorStyle::PointingHand)
                        .hover(move |style| style.bg(danger).opacity(0.15))
                        .on_mouse_down(MouseButton::Left, {
                            let clr = clr.clone();
                            move |_ev, window, cx| {
                                cx.stop_propagation();
                                clr(window, cx);
                            }
                        })
                        .child(
                            div()
                                .font_family("iconfont")
                                .text_size(px(9.))
                                .text_color(danger)
                                .child("\u{e7b7}"),
                        ),
                )
            })
    }

    pub fn render_latest_hotkeys_popup_overlay(
        panel_entity: Entity<SettingsPanel>,
        state: Entity<AppState>,
        wm: Entity<WindowManager>,
        latest: Vec<LatestHotkeyEntry>,
        theme: ClippiTheme,
        motion: (f32, f32, f32),
    ) -> impl IntoElement {
        let (opacity, scale, offset) = motion;
        let configured = latest
            .iter()
            .filter(|entry| !entry.hotkey.is_empty())
            .count();
        let close_panel = panel_entity.clone();

        div()
            .absolute()
            .left(px(36.))
            .right(px(0.))
            .top(px(0.))
            .bottom(px(0.))
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000033))
            .opacity(opacity)
            .cursor(CursorStyle::PointingHand)
            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                close_panel.update(cx, |panel, cx| {
                    panel.latest_hotkeys_popup_open = false;
                    cx.notify();
                });
            })
            .child(
                div()
                    .w(px(424. * scale))
                    .mt(px(offset))
                    .rounded(px(12.))
                    .bg(theme.panel_surface)
                    .border(px(1.))
                    .border_color(theme.panel_sep_line)
                    .shadow_lg()
                    .p(px(14. * scale))
                    .occlude()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .cursor(CursorStyle::Arrow)
                    .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .font_family("iconfont")
                                            .text_size(px(14.))
                                            .text_color(theme.accent)
                                            .child("\u{e6a8}"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.text_1)
                                            .child(I18nKey::LatestHotkeysTitle.text()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .h(px(22.))
                                            .px(px(8.))
                                            .rounded(px(11.))
                                            .bg(theme.accent_soft)
                                            .border(px(1.))
                                            .border_color(theme.accent)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(px(10.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.accent)
                                            .child(format!("{}/10", configured)),
                                    )
                                    .child({
                                        let close = panel_entity.clone();
                                        let close_hover = theme.danger;
                                        let close_text = theme.text_2;
                                        div()
                                            .w(px(24.))
                                            .h(px(24.))
                                            .rounded(px(5.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .cursor(CursorStyle::PointingHand)
                                            .hover(move |style| style.bg(close_hover).opacity(0.12))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                move |_ev, _window, cx| {
                                                    cx.stop_propagation();
                                                    close.update(cx, |panel, cx| {
                                                        panel.latest_hotkeys_popup_open = false;
                                                        cx.notify();
                                                    });
                                                },
                                            )
                                            .child(
                                                div()
                                                    .font_family("iconfont")
                                                    .text_size(px(13.))
                                                    .text_color(close_text)
                                                    .child("\u{e7b7}"),
                                            )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .children((0..5).map(move |row| {
                                let left = row;
                                let right = row + 5;
                                let latest_l = latest.clone();
                                let latest_r = latest.clone();
                                let wm_l = wm.clone();
                                let wm_r = wm.clone();
                                let panel_l = panel_entity.clone();
                                let panel_r = panel_entity.clone();
                                let state_l = state.clone();
                                let state_r = state.clone();
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap(px(8.))
                                    .child({
                                        let rec: AppCallback = Rc::new(move |_window, cx| {
                                            wm_l.update(cx, |wm, cx| {
                                                wm.start_latest_slot_recording(left, cx);
                                            });
                                        });
                                        let panel = panel_l.clone();
                                        let state = state_l.clone();
                                        let wm_clear = wm.clone();
                                        let clr: AppCallback = Rc::new(move |_window, cx| {
                                            wm_clear.update(cx, |wm, _cx| {
                                                wm.unregister_latest_slot_hotkey(left);
                                            });
                                            state.update(cx, |st, _cx| {
                                                if left < st.settings.latest_hotkeys.len() {
                                                    st.settings.latest_hotkeys[left].hotkey.clear();
                                                    st.settings.save();
                                                }
                                            });
                                            panel.update(cx, |_panel, cx| cx.notify());
                                        });
                                        Self::render_latest_slot_cell(
                                            left,
                                            &latest_l,
                                            theme.clone(),
                                            rec,
                                            Some(clr),
                                        )
                                    })
                                    .child({
                                        let rec: AppCallback = Rc::new(move |_window, cx| {
                                            wm_r.update(cx, |wm, cx| {
                                                wm.start_latest_slot_recording(right, cx);
                                            });
                                        });
                                        let panel = panel_r.clone();
                                        let state = state_r.clone();
                                        let wm_clear = wm.clone();
                                        let clr: AppCallback = Rc::new(move |_window, cx| {
                                            wm_clear.update(cx, |wm, _cx| {
                                                wm.unregister_latest_slot_hotkey(right);
                                            });
                                            state.update(cx, |st, _cx| {
                                                if right < st.settings.latest_hotkeys.len() {
                                                    st.settings.latest_hotkeys[right]
                                                        .hotkey
                                                        .clear();
                                                    st.settings.save();
                                                }
                                            });
                                            panel.update(cx, |_panel, cx| cx.notify());
                                        });
                                        Self::render_latest_slot_cell(
                                            right,
                                            &latest_r,
                                            theme.clone(),
                                            rec,
                                            Some(clr),
                                        )
                                    })
                            })),
                    ),
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

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(8.))
            // 1. Hotkey recording card (66px)
            .child({
                let state = state.clone();
                let wm = wm.clone();
                let this = this.clone();
                Self::render_recording_card(
                    I18nKey::HotkeyTabTitle,
                    hotkey_display.clone().into(),
                    recording,
                    theme,
                    move |_window, cx| {
                        wm.update(cx, |wm, cx| wm.start_hotkey_recording(cx));
                        state.update(cx, |s, _cx| s.hotkey_recording = true);
                        this.update(cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // 1b. Quick hotkey recording card (only visible when enabled)
            .when(self.state.read(cx).settings.quick_hotkey_enabled, {
                let quick_hotkey = self.state.read(cx).settings.quick_hotkey.clone();
                let quick_recording = self.state.read(cx).recording_quick_hotkey;
                let state = state.clone();
                let wm = wm.clone();
                let this = this.clone();
                let theme = theme.clone();
                move |parent| {
                    parent.child(Self::render_recording_card(
                        I18nKey::QuickHotkeyLabel,
                        quick_hotkey.into(),
                        quick_recording,
                        &theme,
                        move |_window, cx| {
                            wm.update(cx, |wm, cx| wm.start_quick_hotkey_recording(cx));
                            state.update(cx, |s, _cx| s.recording_quick_hotkey = true);
                            this.update(cx, |_panel, cx| cx.notify());
                        },
                    ))
                }
            })
            // 1c. Latest content hotkeys entry card
            .child({
                let this = this.clone();
                let configured = app
                    .settings
                    .latest_hotkeys
                    .iter()
                    .filter(|e| !e.hotkey.is_empty())
                    .count();
                let desc = format!("已设置 {}/10", configured);
                let theme_clone = theme.clone();
                div()
                    .h(px(66.))
                    .rounded(px(10.))
                    .bg(theme.surface)
                    .border(px(1.))
                    .border_color(theme.divider)
                    .px(px(14.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .cursor(CursorStyle::PointingHand)
                    .hover(move |style| style.bg(theme_clone.titlebar_bg))
                    .on_mouse_down(MouseButton::Left, {
                        let this_click = this.clone();
                        move |_ev, _window, cx| {
                            this_click.update(cx, |panel, cx| {
                                panel.latest_hotkeys_popup_open = !panel.latest_hotkeys_popup_open;
                                cx.notify();
                            });
                        }
                    })
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
                                    .child(I18nKey::LatestHotkeysTitle.text()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(theme.text_3)
                                    .child(desc),
                            ),
                    )
                    .child(
                        div()
                            .font_family("iconfont")
                            .text_size(px(14.))
                            .text_color(theme.text_2)
                            .child("\u{e620}"), // arrow icon
                    )
            })
            // 2. Shared foreground app info bar (always shows current foreground)
            .child({
                // When recording was triggered from the bar button, show recording
                // effect on the bar itself (matches foreground app).
                let recording_on_bar = is_recording_paste
                    && self
                        .recording_paste_shortcut
                        .as_deref()
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
                            panel.window_manager.update(cx, |wm, cx| {
                                wm.start_paste_shortcut_recording(app_name_ps.clone(), cx);
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
            .when(cfg!(target_os = "windows"), |root| {
                root.child({
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
                                && self
                                    .recording_paste_shortcut
                                    .as_deref()
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
                                        panel.window_manager.update(cx, |wm, cx| {
                                            wm.start_paste_shortcut_recording(name_re.clone(), cx);
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
                    Self::render_per_app_list_section(&ps_title, &ps_hint, &entries, theme)
                })
            })
    }
}
