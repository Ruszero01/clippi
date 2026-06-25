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
        #[cfg(target_os = "windows")]
        let block_system_behaviors = app.settings.block_system_window_behaviors;
        #[cfg(target_os = "windows")]
        let hide_taskbar_icon = app.settings.hide_taskbar_icon;
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
        let container = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(8.))
            // --- Auto-start ---
            .child(self.render_toggle_row(
                I18nKey::SettingAutoStart,
                I18nKey::DescAutoStart,
                I18nKey::DescAutoStart,
                auto_start,
                window,
                cx,
                |state, _this, _window, _cx| {
                    let new_val = !state.read(_cx).settings.auto_start;
                    if let Err(e) = set_auto_start(new_val) {
                        log::error!("Failed to set auto-start: {e}");
                        return;
                    }
                    state.update(_cx, |s, _cx| {
                        s.settings.auto_start = new_val;
                        s.settings.save();
                    });
                },
            ))
            // --- Auto-hide ---
            .child({
                let wm = wm.clone();
                self.render_toggle_row(
                    I18nKey::SettingAutoHide,
                    I18nKey::DescAutoHide,
                    I18nKey::DescAutoHide,
                    auto_hide,
                    window,
                    cx,
                    move |state, _this, _window, _cx| {
                        let new_val = state.update(_cx, |s, _cx| {
                            s.settings.auto_hide = !s.settings.auto_hide;
                            s.settings.save();
                            s.settings.auto_hide
                        });
                        wm.update(_cx, |wm, _cx| wm.set_auto_hide(new_val));
                    },
                )
            })
            // --- Silent start ---
            .child(self.render_toggle_row(
                I18nKey::SettingSilentStart,
                I18nKey::DescSilentStart,
                I18nKey::DescSilentStart,
                silent_start,
                window,
                cx,
                |state, _this, _window, _cx| {
                    state.update(_cx, |s, _cx| {
                        s.settings.silent_start = !s.settings.silent_start;
                        s.settings.save();
                    });
                },
            ))
            // --- Quick paste window ---
            .child({
                let wm = wm.clone();
                let quick_enabled = self.state.read(cx).settings.quick_hotkey_enabled;
                self.render_toggle_row(
                    I18nKey::SettingQuickWindow,
                    I18nKey::DescQuickWindow,
                    I18nKey::DescQuickWindow,
                    quick_enabled,
                    window,
                    cx,
                    move |state, _this, _window, _cx| {
                        let new_val = !state.read(_cx).settings.quick_hotkey_enabled;
                        state.update(_cx, |s, _cx| {
                            s.settings.quick_hotkey_enabled = new_val;
                            if new_val && s.settings.quick_hotkey == s.settings.hotkey {
                                s.settings.quick_hotkey = "Alt+V".to_string();
                            }
                            s.settings.save();
                        });
                        wm.update(_cx, |wm, cx| {
                            if new_val {
                                wm.reload_quick_hotkey(cx);
                            } else {
                                wm.disable_quick_hotkey();
                            }
                        });
                    },
                )
            });

        // --- Hide taskbar icon (Windows only — macOS tray apps already hide from Dock) ---
        #[cfg(target_os = "windows")]
        let container = container.child({
            let wm = wm.clone();
            self.render_toggle_row(
                I18nKey::SettingHideTaskbar,
                I18nKey::DescHideTaskbar,
                I18nKey::DescHideTaskbar,
                hide_taskbar_icon,
                window,
                cx,
                move |state, _this, _window, _cx| {
                    let new_val = state.update(_cx, |s, _cx| {
                        s.settings.hide_taskbar_icon = !s.settings.hide_taskbar_icon;
                        s.settings.save();
                        s.settings.hide_taskbar_icon
                    });
                    wm.update(_cx, |wm, cx| wm.set_hide_taskbar_icon(new_val, cx));
                },
            )
        });

        // --- Block system window behaviors (Windows only) ---
        #[cfg(target_os = "windows")]
        let container = container.child({
            let wm = wm.clone();
            self.render_toggle_row(
                I18nKey::SettingBlockSysBehavior,
                I18nKey::DescBlockSysBehavior,
                I18nKey::DescBlockSysBehavior,
                block_system_behaviors,
                window,
                cx,
                move |state, _this, _window, _cx| {
                    let new_val = state.update(_cx, |s, _cx| {
                        s.settings.block_system_window_behaviors =
                            !s.settings.block_system_window_behaviors;
                        s.settings.save();
                        s.settings.block_system_window_behaviors
                    });
                    wm.update(_cx, |wm, cx| {
                        wm.set_block_system_window_behaviors(new_val, cx)
                    });
                },
            )
        });

        container
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
                        let new_lang = if key == "system" {
                            String::new()
                        } else {
                            key.to_string()
                        };
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
