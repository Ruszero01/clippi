//! General settings tab — language, startup, theme, position.
//!

use gpui::*;

use crate::core;
use crate::core::frontend::PositionMode;
use crate::core::i18n_keys::I18nKey;
use crate::core::settings::{set_auto_start, AutoStartChange};
use crate::ui::components::slider::SliderDetent;
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
        let always_reset = app.settings.always_reset_to_clipboard;
        #[cfg(target_os = "windows")]
        let block_system_behaviors = app.settings.block_system_window_behaviors;
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let hide_taskbar_icon = app.settings.hide_taskbar_icon;
        let theme_str = app.settings.theme.clone();
        let position_mode = app.settings.window_position_mode.clone();
        let lang = app.settings.language.clone();
        let font_size_level = app.settings.font_size_level.clone();
        let font_size_scale = app
            .settings
            .font_size_scale
            .unwrap_or_else(|| crate::ui::font::level_scale(&font_size_level));
        let font_family = app.settings.font_family.clone();
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
        let mut container = div().flex().flex_col().gap(px(14.)).pt(px(8.));
        // --- Startup ---
        // Elevated builds auto-start through a logon task (a `Run` value cannot
        // elevate at logon), so the row says which mechanism is in use.
        let auto_start_desc_on = if auto_start
            && core::settings::auto_start_method() == core::settings::AutoStartMethod::ElevatedTask
        {
            I18nKey::DescAutoStartTask
        } else {
            I18nKey::DescAutoStart
        };
        let startup_rows: Vec<AnyElement> = vec![
            self.render_toggle_row(
                I18nKey::SettingAutoStart,
                auto_start_desc_on,
                I18nKey::DescAutoStart,
                auto_start,
                window,
                cx,
                |state, _this, _window, _cx| {
                    let new_val = !state.read(_cx).settings.auto_start;
                    let outcome = match set_auto_start(new_val) {
                        Ok(outcome) => outcome,
                        Err(e) => {
                            log::error!("Failed to set auto-start: {e}");
                            state.update(_cx, |s, _cx| {
                                s.show_warning_toast(I18nKey::ToastAutoStartFailed.text());
                            });
                            return;
                        }
                    };
                    state.update(_cx, |s, _cx| {
                        s.settings.auto_start = new_val;
                        s.settings.save();
                        // The elevated logon task is the only registration that
                        // works for an elevated build, so report both when it
                        // failed and when a leftover task could not be cleared.
                        match outcome {
                            AutoStartChange::Applied => {}
                            AutoStartChange::AppliedWithoutElevation => {
                                s.show_warning_toast(I18nKey::ToastAutoStartWithoutElevation.text())
                            }
                            AutoStartChange::KeptElevatedTask => {
                                s.show_warning_toast(I18nKey::ToastAutoStartKeptElevatedTask.text())
                            }
                            AutoStartChange::LeftoverElevatedTask => {
                                s.show_warning_toast(I18nKey::ToastAutoStartLeftoverTask.text())
                            }
                        }
                    });
                },
            )
            .into_any_element(),
            self.render_toggle_row(
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
            )
            .into_any_element(),
        ];
        container =
            container.child(self.settings_group(I18nKey::GroupStartup.text(), startup_rows));

        // --- Window behaviour ---
        let mut window_rows: Vec<AnyElement> = vec![
            {
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
            }
            .into_any_element(),
            self.render_toggle_row(
                I18nKey::SettingAlwaysResetToClipboard,
                I18nKey::DescAlwaysResetToClipboard,
                I18nKey::DescAlwaysResetToClipboard,
                always_reset,
                window,
                cx,
                |state, _this, _window, _cx| {
                    state.update(_cx, |s, _cx| {
                        s.settings.always_reset_to_clipboard =
                            !s.settings.always_reset_to_clipboard;
                        s.settings.save();
                    });
                },
            )
            .into_any_element(),
        ];
        #[cfg(target_os = "windows")]
        window_rows.push(
            {
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
            }
            .into_any_element(),
        );
        #[cfg(target_os = "macos")]
        window_rows.push(
            {
                let wm = wm.clone();
                self.render_toggle_row(
                    I18nKey::SettingHideDockIcon,
                    I18nKey::DescHideDockIcon,
                    I18nKey::DescHideDockIcon,
                    hide_taskbar_icon,
                    window,
                    cx,
                    move |state, _this, _window, _cx| {
                        let hide_dock_icon = state.update(_cx, |s, _cx| {
                            s.settings.hide_taskbar_icon = !s.settings.hide_taskbar_icon;
                            s.settings.save();
                            s.settings.hide_taskbar_icon
                        });
                        wm.update(_cx, |wm, cx| wm.set_hide_taskbar_icon(hide_dock_icon, cx));
                    },
                )
            }
            .into_any_element(),
        );
        #[cfg(target_os = "windows")]
        window_rows.push(
            {
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
            }
            .into_any_element(),
        );
        let quick_rows: Vec<AnyElement> = vec![{
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
        }
        .into_any_element()];
        container = container.child(self.settings_group(I18nKey::GroupWindow.text(), window_rows));

        // --- Quick paste window ---
        container =
            container.child(self.settings_group(I18nKey::GroupQuickWindow.text(), quick_rows));

        // --- Appearance ---
        let appearance_rows: Vec<AnyElement> = vec![
            {
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
            }
            .into_any_element(),
            {
                let this = this.clone();
                let display = if font_family.is_empty() {
                    I18nKey::FontFamilySystem.text().to_string()
                } else {
                    font_family.clone()
                };
                self.setting_row_with_value(
                    I18nKey::SettingFontFamily.text(),
                    I18nKey::DescFontFamily.text(),
                    display,
                    move |_window, _cx| {
                        this.update(_cx, |panel, cx| {
                            panel.open_font_picker(cx);
                        });
                    },
                )
            }
            .into_any_element(),
            {
                let state = state.clone();
                let this = this.clone();
                let this_preview = this.clone();
                // 4 labelled major detents with an unlabelled minor stop between
                // each pair, so the size can be nudged in finer increments.
                let major_labels = [
                    I18nKey::FontSizeCompact.text(),
                    I18nKey::FontSizeStandard.text(),
                    I18nKey::FontSizeLarge.text(),
                    I18nKey::FontSizeXLarge.text(),
                ];
                let detents: Vec<SliderDetent> = (0..crate::ui::font::FONT_SIZE_DETENTS.len())
                    .map(|i| {
                        match crate::ui::font::FONT_SIZE_MAJOR_DETENTS
                            .iter()
                            .position(|major| *major == i)
                        {
                            Some(pos) => SliderDetent::major(major_labels[pos]),
                            None => SliderDetent::minor(),
                        }
                    })
                    .collect();
                let active = crate::ui::font::detent_for_scale(font_size_scale);
                self.setting_row_with_slider(
                    I18nKey::SettingFontSize.text(),
                    I18nKey::DescFontSize.text(),
                    detents,
                    active,
                    "font-size-slider",
                    move |index, _window, _cx| {
                        let scale = crate::ui::font::FONT_SIZE_DETENTS
                            [index.min(crate::ui::font::FONT_SIZE_DETENTS.len() - 1)];
                        state.update(_cx, |s, _cx| {
                            s.settings.font_size_scale = Some(scale);
                            s.settings.save();
                        });
                        crate::ui::font::apply_scale(scale);
                        crate::ui::font::apply_to_global_theme(_cx);
                        this.update(_cx, |_panel, cx| {
                            cx.emit(SettingsEvent::FontChanged);
                            cx.notify();
                        });
                    },
                    move |_window, _cx| {
                        // Visual-only: repaint so the knob follows the pointer.
                        this_preview.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            }
            .into_any_element(),
        ];
        container =
            container.child(self.settings_group(I18nKey::GroupAppearance.text(), appearance_rows));

        // --- Language ---
        let language_rows: Vec<AnyElement> = vec![{
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
                    crate::ui::i18n::set_language(&effective);
                    state.update(_cx, |s, _cx| {
                        s.settings.language = new_lang;
                        s.settings.save();
                    });
                    wm.update(_cx, |wm, _cx| wm.update_tray_language());
                    this.update(_cx, |_, cx| cx.notify());
                },
            )
        }
        .into_any_element()];
        container =
            container.child(self.settings_group(I18nKey::GroupLanguage.text(), language_rows));

        // --- Window position ---
        let position_rows: Vec<AnyElement> = vec![{
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
        }
        .into_any_element()];
        container =
            container.child(self.settings_group(I18nKey::GroupPosition.text(), position_rows));

        container
    }
}
