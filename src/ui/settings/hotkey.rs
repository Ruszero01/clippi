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

use super::SettingsPanel;
use crate::core::i18n_keys::I18nKey;
use crate::core::settings::LatestHotkeyEntry;
use crate::state::app::AppState;
use crate::ui::theme::ClippiTheme;
use crate::ui::window_manager::{WinVTakeoverStatus, WindowManager};

/// Confirm action for hotkey settings operations.
/// Emitted by SettingsPanel, handled by RootView to show ConfirmDialog.
#[derive(Debug, Clone)]
pub enum HotkeyConfirmAction {
    /// Add app to hotkey blacklist.
    AddBlacklist {
        app_name: String,
    },
    /// Remove app from hotkey blacklist.
    RemoveBlacklist {
        app_name: String,
    },
    /// Add/update paste shortcut for an app.
    AddPasteShortcut {
        app_name: String,
        shortcut: String,
    },
    /// Remove paste shortcut for an app.
    RemovePasteShortcut {
        app_name: String,
    },
    AddClipboardBlacklist {
        app_name: String,
    },
    RemoveClipboardBlacklist {
        app_name: String,
    },
    /// Enable Win+V takeover (Windows only).
    WinVTakeoverEnable,
    /// Disable Win+V takeover (Windows only).
    WinVTakeoverDisable,
    /// Show manual setup instructions (Windows only).
    WinVManualSetup,
}

/// GPUI callback type alias for per-app list entries.
type AppCallback = Rc<dyn Fn(&mut Window, &mut App)>;
const LATEST_HOTKEY_POPUP_WIDTH: f32 = 304.;
const LATEST_HOTKEY_POPUP_HEIGHT: f32 = 316.;
const LATEST_HOTKEY_SLOT_WIDTH: f32 = 128.;
const LATEST_HOTKEY_COLUMN_GAP: f32 = 12.;

impl SettingsPanel {
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
        recording: bool,
        on_record: AppCallback,
        on_clear: Option<AppCallback>,
    ) -> impl IntoElement {
        let entry = latest.get(index);
        let hotkey = entry.map(|e| e.hotkey.as_str()).unwrap_or("");
        let has_hotkey = entry.is_some_and(|e| !e.hotkey.is_empty());

        let label = if recording {
            I18nKey::LatestHotkeyRecording.text().to_string()
        } else if has_hotkey {
            hotkey.to_string()
        } else {
            I18nKey::LatestHotkeyClickRecord.text().to_string()
        };
        let text_color = if has_hotkey || recording {
            theme.accent
        } else {
            theme.text_3
        };
        let border_color = if has_hotkey || recording {
            theme.accent
        } else {
            theme.divider
        };
        let bg_color = if has_hotkey || recording {
            theme.accent_soft
        } else {
            theme.surface
        };
        let badge_bg = if has_hotkey || recording {
            theme.accent
        } else {
            theme.titlebar_bg
        };
        let badge_text = if has_hotkey || recording {
            rgb(0xffffff)
        } else {
            theme.text_3
        };
        let hover_bg = if has_hotkey || recording {
            theme.accent_soft
        } else {
            theme.btn_hover
        };
        let danger = theme.danger;
        let rec = on_record.clone();

        div()
            .w(px(LATEST_HOTKEY_SLOT_WIDTH))
            .flex_shrink_0()
            .min_w(px(0.))
            .h(px(44.))
            .rounded(px(7.))
            .bg(bg_color)
            .border(px(if recording { 2. } else { 1. }))
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
        recording_slot: Option<usize>,
        theme: ClippiTheme,
        layout: (f32, f32, f32, f32, f32),
    ) -> impl IntoElement {
        let (opacity, scale, offset, viewport_width, viewport_height) = layout;
        let popup_width = LATEST_HOTKEY_POPUP_WIDTH * scale;
        let popup_height = LATEST_HOTKEY_POPUP_HEIGHT * scale;
        let main_width = (viewport_width - 36.).max(popup_width);
        let popup_left = ((main_width - popup_width) * 0.5).max(8.);
        let popup_top = ((viewport_height - popup_height) * 0.5).max(8.) + offset;
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
            .bg(rgba(0x00000033))
            .rounded(px(12.))
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
                    .absolute()
                    .left(px(popup_left))
                    .top(px(popup_top))
                    .w(px(popup_width))
                    .max_w(px(popup_width))
                    .rounded(px(8.))
                    .bg(theme.surface)
                    .border(px(1.))
                    .border_color(theme.divider)
                    .shadow_lg()
                    .p(px(12. * scale))
                    .occlude()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .cursor(CursorStyle::Arrow)
                    .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_1()
                                    .items_center()
                                    .gap(px(14.))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.text_1)
                                            .child(I18nKey::LatestHotkeysTitle.text()),
                                    )
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
                                    ),
                            )
                            .child({
                                let close = panel_entity.clone();
                                let close_text = theme.text_2;
                                div()
                                    .w(px(26.))
                                    .h(px(26.))
                                    .rounded(px(6.))
                                    .font_family("iconfont")
                                    .text_size(px(13.))
                                    .text_color(close_text)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(|style| style.bg(rgba(0xffffff0d)))
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                        cx.stop_propagation();
                                        close.update(cx, |panel, cx| {
                                            panel.latest_hotkeys_popup_open = false;
                                            cx.notify();
                                        });
                                    })
                                    .child("\u{e7b7}")
                            }),
                    )
                    .child(div().h(px(1.)).bg(theme.divider))
                    .child(
                        div().w_full().flex().justify_center().child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .w(px(2. * LATEST_HOTKEY_SLOT_WIDTH + LATEST_HOTKEY_COLUMN_GAP))
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
                                        .gap(px(LATEST_HOTKEY_COLUMN_GAP))
                                        .child({
                                            let panel_record = panel_l.clone();
                                            let rec: AppCallback = Rc::new(move |_window, cx| {
                                                wm_l.update(cx, |wm, cx| {
                                                    wm.start_latest_slot_recording(left, cx);
                                                });
                                                panel_record.update(cx, |_panel, cx| cx.notify());
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
                                                        st.settings.latest_hotkeys[left]
                                                            .hotkey
                                                            .clear();
                                                        st.settings.save();
                                                    }
                                                });
                                                panel.update(cx, |_panel, cx| cx.notify());
                                            });
                                            Self::render_latest_slot_cell(
                                                left,
                                                &latest_l,
                                                theme.clone(),
                                                recording_slot == Some(left),
                                                rec,
                                                Some(clr),
                                            )
                                        })
                                        .child({
                                            let panel_record = panel_r.clone();
                                            let rec: AppCallback = Rc::new(move |_window, cx| {
                                                wm_r.update(cx, |wm, cx| {
                                                    wm.start_latest_slot_recording(right, cx);
                                                });
                                                panel_record.update(cx, |_panel, cx| cx.notify());
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
                                                recording_slot == Some(right),
                                                rec,
                                                Some(clr),
                                            )
                                        })
                                })),
                        ),
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
        let blacklist = app.settings.hotkey_blacklist.clone();
        let paste_shortcuts = app.settings.paste_shortcuts.clone();
        #[cfg(target_os = "windows")]
        let _replace_system_win_v = app.settings.replace_system_win_v;
        // borrow released

        let takeover_status = wm.read(cx).win_v_takeover_status();
        let takeover_active =
            cfg!(target_os = "windows") && takeover_status != WinVTakeoverStatus::Disabled;

        let theme = &self.theme;

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(8.))
            // 1. Hotkey recording card (66px) — or managed card when takeover active
            .child({
                let state = state.clone();
                let wm = wm.clone();
                let this = this.clone();

                #[cfg(target_os = "windows")]
                if takeover_active {
                    // --- Managed by takeover: show "Win+V" fixed, disable recording ---
                    let theme_surface = theme.surface;
                    let theme_divider = theme.divider;
                    let theme_text_1 = theme.text_1;
                    let theme_text_3 = theme.text_3;
                    div()
                        .h(px(66.))
                        .rounded(px(10.))
                        .bg(theme_surface)
                        .border(px(1.))
                        .border_color(theme_divider)
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
                                        .text_color(theme_text_1)
                                        .child(I18nKey::HotkeyTabTitle.text()),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(theme_text_3)
                                        .child(I18nKey::WinVManagedByMode.text()),
                                ),
                        )
                        .child(
                            div()
                                .h(px(28.))
                                .w(px(80.))
                                .rounded(px(7.))
                                .bg(theme.accent_soft)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(theme.accent)
                                        .child("Win+V"),
                                ),
                        )
                        .into_any_element()
                } else {
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
                    .into_any_element()
                }

                #[cfg(not(target_os = "windows"))]
                {
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
                    .into_any_element()
                }
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
            // 1c. Win+V takeover toggle + status (Windows only)
            .when(cfg!(target_os = "windows"), {
                let takeover_active_val = takeover_active;
                let takeover_status_val = takeover_status;
                let this_winv = this.clone();
                let state_winv = state.clone();
                let wm_winv = wm.clone();
                move |parent| {
                    let status_text = match takeover_status_val {
                        WinVTakeoverStatus::Active => I18nKey::WinVStatusActive.text(),
                        WinVTakeoverStatus::HotkeyUnavailable => {
                            I18nKey::WinVStatusConflict.text()
                        }
                        WinVTakeoverStatus::RegistryUpdateRequired => {
                            I18nKey::WinVStatusUpdateRequired.text()
                        }
                        WinVTakeoverStatus::RegistryError => {
                            I18nKey::WinVStatusRegistryError.text()
                        }
                        WinVTakeoverStatus::Disabled => I18nKey::WinVTakeoverDesc.text(),
                    };
                    let show_recheck =
                        takeover_status_val == WinVTakeoverStatus::HotkeyUnavailable;
                    let show_manual = matches!(
                        takeover_status_val,
                        WinVTakeoverStatus::RegistryError
                            | WinVTakeoverStatus::RegistryUpdateRequired
                    );

                    parent.child(
                        // Child is a container div.
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child({
                                // Simple toggle row without animation.
                                let toggle_on = takeover_active_val;
                                let accent = theme.accent;
                                let divider = theme.divider;
                                let toggle_bg = if toggle_on { accent } else { divider };
                                let knob_x = if toggle_on { 20.0 } else { 2.0 };
                                let this_toggle = this_winv.clone();
                                let state_toggle = state_winv.clone();
                                div()
                                    .h(px(66.))
                                    .rounded(px(10.))
                                    .bg(theme.surface)
                                    .border(px(1.))
                                    .border_color(divider)
                                    .px(px(14.))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .flex_col()
                                            .gap(px(2.))
                                            .child(
                                                div()
                                                    .max_w_full()
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .whitespace_nowrap()
                                                    .text_size(px(12.))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(theme.text_1)
                                                    .child(I18nKey::WinVTakeoverLabel.text()),
                                            )
                                            .child(
                                                div()
                                                    .max_w_full()
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .whitespace_nowrap()
                                                    .text_size(px(10.))
                                                    .text_color(theme.text_3)
                                                    .child(status_text),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .w(px(40.))
                                            .h(px(22.))
                                            .rounded(px(11.))
                                            .bg(toggle_bg)
                                            .flex()
                                            .items_center()
                                            .cursor(CursorStyle::PointingHand)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                move |_ev, _window, cx| {
                                                    let current = state_toggle
                                                        .read(cx)
                                                        .settings
                                                        .replace_system_win_v;
                                                    if !current {
                                                        this_toggle.update(cx, |_panel, cx| {
                                                            cx.emit(super::SettingsEvent::ShowHotkeyConfirm(
                                                                HotkeyConfirmAction::WinVTakeoverEnable,
                                                            ));
                                                        });
                                                    } else {
                                                        this_toggle.update(cx, |_panel, cx| {
                                                            cx.emit(super::SettingsEvent::ShowHotkeyConfirm(
                                                                HotkeyConfirmAction::WinVTakeoverDisable,
                                                            ));
                                                        });
                                                    }
                                                },
                                            )
                                            .child(
                                                div()
                                                    .w(px(18.))
                                                    .h(px(18.))
                                                    .rounded(px(9.))
                                                    .bg(rgb(0xffffff))
                                                    .ml(px(knob_x)),
                                            ),
                                    )
                            })
                            .when(show_recheck || show_manual, move |parent| {
                                let accent = theme.accent;
                                let wm_recheck = wm_winv.clone();
                                let this_recheck = this_winv.clone();
                                let this_manual = this_winv.clone();
                                parent
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .justify_end()
                                            .px(px(4.))
                                            .when(show_recheck, |row| {
                                                row.child(
                                                    div()
                                                        .px(px(8.))
                                                        .py(px(3.))
                                                        .rounded(px(5.))
                                                        .bg(accent)
                                                        .text_size(px(10.))
                                                        .text_color(rgb(0xffffff))
                                                        .cursor(CursorStyle::PointingHand)
                                                        .hover(|style| style.opacity(0.85))
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            move |_ev, _window, cx| {
                                                                wm_recheck.update(cx, |wm, cx| {
                                                                    wm.recheck_win_v_takeover(cx);
                                                                });
                                                                this_recheck.update(cx, |_panel, cx| {
                                                                    cx.notify();
                                                                });
                                                            },
                                                        )
                                                        .child(I18nKey::WinVRecheckBtn.text()),
                                                )
                                            })
                                            .when(show_manual, |row| {
                                                let text_3 = theme.text_3;
                                                row.child(
                                                    div()
                                                        .px(px(6.))
                                                        .py(px(3.))
                                                        .text_size(px(10.))
                                                        .text_color(text_3)
                                                        .cursor(CursorStyle::PointingHand)
                                                        .hover(|style| style.opacity(0.7))
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            move |_ev, _window, cx| {
                                                                this_manual.update(cx, |_panel, cx| {
                                                                    cx.emit(super::SettingsEvent::ShowHotkeyConfirm(
                                                                        HotkeyConfirmAction::WinVManualSetup,
                                                                    ));
                                                                });
                                                            },
                                                        )
                                                        .child(I18nKey::WinVManualSetupHint.text()),
                                                )
                                            }),
                                    )
                            }),
                    )
                }
            })
            // 1d. Latest content hotkeys entry card
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
                                let open = !panel.latest_hotkeys_popup_open;
                                panel.close_app_list_popups();
                                panel.latest_hotkeys_popup_open = open;
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
                            .child("\u{e602}"), // arrow icon
                    )
            })
            // 3. Hotkey blacklist entry card
            .child({
                let this = this.clone();
                let count = blacklist.len();
                let desc = I18nKey::ClipboardAppBlacklistCount.fmt(&[&count.to_string()]);
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
                                panel.toggle_hotkey_blacklist_popup();
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
                                    .child(I18nKey::HotkeyBlacklist.text()),
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
                            .child("\u{e602}"),
                    )
            })
            // 4. Paste shortcut entry card (Windows only)
            .when(cfg!(target_os = "windows"), |root| {
                let count = paste_shortcuts.len();
                let desc = I18nKey::ClipboardAppBlacklistCount.fmt(&[&count.to_string()]);
                let this = this.clone();
                let theme_clone = theme.clone();
                root.child(
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
                                    panel.toggle_paste_shortcuts_popup();
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
                                        .child(I18nKey::HotkeyPasteShortcut.text()),
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
                                .child("\u{e602}"),
                        ),
                )
            })
    }
}
