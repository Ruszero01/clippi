//! General settings tab — language, startup, theme, position.
//!

use gpui::*;

use crate::core;
use crate::core::frontend::PositionMode;
use crate::core::i18n_keys::I18nKey;
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

        // --- Snapshot current values from AppState ---
        let app = self.state.read(cx);
        let auto_start = app.settings.auto_start;
        let auto_hide = app.settings.auto_hide;
        let silent_start = app.settings.silent_start;
        let theme_str = app.settings.theme.clone();
        let position_mode = app.settings.window_position_mode.clone();
        let lang = app.settings.language.clone();
        // --- borrow released here — `app` is a &AppState reference ---

        // --- Derive display indices from string settings ---
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
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(8.))
            // --- Auto-start ---
            .child({
                let state = state.clone();
                let this = this.clone();
                self.setting_row_with_toggle(
                    I18nKey::SettingAutoStart.text(),
                    I18nKey::DescAutoStart.text(),
                    auto_start,
                    window,
                    cx,
                    move |_window, _cx| {
                        let new_val = !state.read(_cx).settings.auto_start;
                        if let Err(e) = set_auto_start(new_val) {
                            log::error!("Failed to set auto-start: {e}");
                            return;
                        }
                        state.update(_cx, |s, _cx| {
                            s.settings.auto_start = new_val;
                            s.settings.save();
                        });
                        this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // --- Auto-hide ---
            .child({
                let state = state.clone();
                let wm = wm.clone();
                let this = this.clone();
                self.setting_row_with_toggle(
                    I18nKey::SettingAutoHide.text(),
                    I18nKey::DescAutoHide.text(),
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
                        this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // --- Silent start ---
            .child({
                let state = state.clone();
                let this = this.clone();
                self.setting_row_with_toggle(
                    I18nKey::SettingSilentStart.text(),
                    I18nKey::DescSilentStart.text(),
                    silent_start,
                    window,
                    cx,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.silent_start = !s.settings.silent_start;
                            s.settings.save();
                        });
                        this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // --- Theme ---
            .child({
                let state = state.clone();
                let this = this.clone();
                self.setting_row_with_options(
                    I18nKey::SettingTheme.text(),
                    I18nKey::DescTheme.text(),
                    &[
                        ("system", I18nKey::ThemeSystem.text()),
                        ("dark", I18nKey::ThemeDark.text()),
                        ("light", I18nKey::ThemeLight.text()),
                    ],
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
                        this.update(_cx, |_panel, cx| {
                            cx.emit(SettingsEvent::ThemeChanged(theme_str));
                            cx.notify();
                        });
                    },
                )
            })
            // --- Language ---
            .child({
                let state = state.clone();
                let this = this.clone();
                let wm = wm.clone();
                self.setting_row_with_options(
                    I18nKey::SettingLanguage.text(),
                    I18nKey::DescLanguage.text(),
                    &[
                        ("system", I18nKey::LangSystem.text()),
                        ("zh_CN", I18nKey::LangZh.text()),
                        ("en", I18nKey::LangEn.text()),
                    ],
                    if lang.is_empty() { "system" } else { &lang },
                    move |key, _window, _cx| {
                        let new_lang = if key == "system" { String::new() } else { key.to_string() };
                        let effective = if new_lang.is_empty() {
                            core::settings::detect_system_language()
                        } else {
                            new_lang.clone()
                        };
                        core::i18n::set_language(&effective);
                        state.update(_cx, |s, _cx| {
                            s.settings.language = new_lang;
                            s.settings.save();
                        });
                        wm.update(_cx, |wm, _cx| wm.update_tray_language());
                        this.update(_cx, |_, cx| cx.notify());
                    },
                )
            })
            // --- Window position ---
            .child({
                let state = state.clone();
                let wm = wm.clone();
                let this = this.clone();
                self.setting_row_with_options(
                    I18nKey::SettingPosition.text(),
                    I18nKey::DescPosition.text(),
                    &[
                        ("center", I18nKey::PosCenter.text()),
                        ("follow", I18nKey::PosFollow.text()),
                        ("remember", I18nKey::PosRemember.text()),
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
                        this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
    }
}
