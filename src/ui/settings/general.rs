//! General settings tab — language, startup, theme, position.
//!
//! Matches the original Slint `SettingsTabGeneral.slint` layout.
//! Language selector is rendered as UI but wired as a no-op pending
//! GPUI i18n implementation.

use gpui::*;

use crate::core::frontend::PositionMode;
use crate::core::settings::set_auto_start;
use crate::ui::settings::SettingsEvent;

use super::SettingsPanel;

impl SettingsPanel {
    pub fn render_general_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let wm = self.window_manager.clone();
        let this = cx.entity().clone();

        // Snapshot current values from AppState
        let app = self.state.read(cx);
        let auto_start = app.settings.auto_start;
        let auto_hide = app.settings.auto_hide;
        let silent_start = app.settings.silent_start;
        let theme_str = app.settings.theme.clone();
        let position_mode = app.settings.window_position_mode.clone();
        let language = app.settings.language.clone();
        // borrow released here — `app` is a &AppState reference

        // Derive display indices from string settings
        let theme_idx = match theme_str.as_str() {
            "dark" => 1,
            "light" => 2,
            _ => 0,
        };
        let position_idx = match position_mode.as_str() {
            "follow" => 1,
            "remember" => 2,
            _ => 0,
        };
        let lang_idx: i32 = if language == "en" { 1 } else { 0 };

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(8.))
            // ── Language (placeholder — no-op callback) ──
            .child(self.setting_row_with_options(
                "Language",
                "Interface language",
                &[("zh", "\u{4e2d}\u{6587}"), ("en", "English")],
                if lang_idx == 1 { "en" } else { "zh" },
                {
                    // TODO: Wire up when GPUI i18n is implemented.
                    move |_key, _window, _cx| {}
                },
            ))
            // ── Auto-start ──
            .child({
                let state = state.clone();
                self.setting_row_with_toggle(
                    "Auto-start",
                    "Run on system startup",
                    auto_start,
                    window,
                    cx,
                    move |_window, _cx| {
                        let new_val = state.update(_cx, |s, _cx| {
                            s.settings.auto_start = !s.settings.auto_start;
                            s.settings.auto_start
                        });
                        if let Err(e) = set_auto_start(new_val) {
                            log::error!("Failed to set auto-start: {e}");
                        }
                        state.update(_cx, |s, _cx| s.settings.save());
                    },
                )
            })
            // ── Auto-hide ──
            .child({
                let state = state.clone();
                let wm = wm.clone();
                let this = this.clone();
                self.setting_row_with_toggle(
                    "Auto-hide",
                    "Hide on focus loss",
                    auto_hide,
                    window,
                    cx,
                    move |_window, _cx| {
                        let new_val = state.update(_cx, |s, _cx| {
                            s.settings.auto_hide = !s.settings.auto_hide;
                            s.settings.save();
                            s.settings.auto_hide
                        });
                        wm.update(_cx, |wm, _cx| wm.set_auto_hide(new_val));
                        let _ = this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // ── Silent start ──
            .child({
                let state = state.clone();
                self.setting_row_with_toggle(
                    "Silent start",
                    "Start silently in tray",
                    silent_start,
                    window,
                    cx,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.silent_start = !s.settings.silent_start;
                            s.settings.save();
                        });
                    },
                )
            })
            // ── Theme ──
            .child({
                let state = state.clone();
                let this = this.clone();
                self.setting_row_with_options(
                    "Theme",
                    "Select theme",
                    &[("system", "Auto"), ("dark", "Dark"), ("light", "Light")],
                    match theme_idx {
                        1 => "dark",
                        2 => "light",
                        _ => "system",
                    },
                    move |key, _window, _cx| {
                        let theme_str = key.to_string();
                        state.update(_cx, |s, _cx| {
                            s.settings.theme = theme_str.clone();
                            s.settings.save();
                        });
                        let _ = this.update(_cx, |_panel, cx| {
                            cx.emit(SettingsEvent::ThemeChanged(theme_str));
                            cx.notify();
                        });
                    },
                )
            })
            // ── Window position ──
            .child({
                let state = state.clone();
                let wm = wm.clone();
                self.setting_row_with_options(
                    "Position",
                    "Popup position",
                    &[
                        ("center", "Center"),
                        ("follow", "Follow"),
                        ("remember", "Pin"),
                    ],
                    match position_idx {
                        1 => "follow",
                        2 => "remember",
                        _ => "center",
                    },
                    move |key, _window, _cx| {
                        let mode = PositionMode::from_str(key);
                        state.update(_cx, |s, _cx| {
                            s.settings.window_position_mode = key.to_string();
                            s.settings.save();
                        });
                        wm.update(_cx, |wm, _cx| wm.set_position_mode(mode));
                    },
                )
            })
    }
}
