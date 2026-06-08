//! --- Data settings tab — database path + max items. ---
//!
//! --- Mirrors the original Slint `SettingsTabData.slint` layout. ---
//! Includes the reset-data-directory dialog for portable mode.

use gpui::*;
use gpui_component::input::Input;

use crate::core::i18n_keys::I18nKey;
use crate::core::settings::migrate_database;
use crate::ui::settings::SettingsEvent;

use super::SettingsPanel;

/// Which storage mode the reset dialog should target.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StorageMode {
    /// Exe directory (portable / "green" mode).
    Portable,
    /// Platform data directory (system default).
    System,
}

/// State for the reset-data-directory dialog.
#[derive(Clone)]
pub struct ResetDataDirState {
    /// Currently selected storage mode.
    pub selected: StorageMode,
    /// Resolved portable path (exe_dir / clippi.db).
    pub portable_path: String,
    /// Resolved system path (app_data_dir / Clippi / clippi.db).
    pub system_path: String,
}

impl SettingsPanel {
    /// Enter editing mode for the max-items field.
    fn start_edit_max_items(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.state.read(cx).settings.max_items;
        let text = if current == 0 {
            String::new()
        } else {
            current.to_string()
        };
        self.max_items_input.update(cx, |input, cx| {
            input.set_value(&text, window, cx);
        });
        self.editing_max_items = true;
        cx.notify();
    }

    /// Save the max-items value and exit editing mode.
    fn save_max_items(&mut self, cx: &mut Context<Self>) {
        let text = self.max_items_input.read(cx).value().to_string();
        let n: u32 = text.trim().parse().unwrap_or(0);
        self.state.update(cx, |s, _cx| {
            s.settings.max_items = n;
            s.settings.save();
        });
        self.editing_max_items = false;
        cx.notify();
    }

    /// Render the Data settings tab content.
    pub fn render_data_tab(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let this = cx.entity().clone();

        // --- Snapshot current values. ---
        let app = self.state.read(cx);
        let db_path_display = app.settings.resolve_db_path();
        let db_path_str = db_path_display.to_string_lossy().to_string();
        // --- borrow released ---

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(8.))
            // --- ── Database path row (76px, sub-row layout) ── ---
            .child({
                let state = state.clone();
                let this = this.clone();
                let db_path_str = db_path_str.clone();

                let surface = self.theme.surface;
                let divider = self.theme.divider;
                let text_1 = self.theme.text_1;
                let text_2 = self.theme.text_2;
                let text_3 = self.theme.text_3;
                let accent = self.theme.accent;

                div()
                    .h(px(76.))
                    .rounded(px(10.))
                    .bg(surface)
                    .border(px(1.))
                    .border_color(divider)
                    .px(px(14.))
                    .pt(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    // --- Title ---
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(I18nKey::SettingDbPath.text()),
                    )
                    // --- Path display + buttons row ---
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(6.))
                            // --- Path display (flex_1, left-elided via overflow) ---
                            .child({
                                div()
                                    .flex_1()
                                    .h(px(28.))
                                    .rounded(px(7.))
                                    .bg(if self.theme.bg == rgb(0x191a1b) {
                                        rgb(0x191a1b)
                                    } else {
                                        rgb(0xf2f3f8)
                                    })
                                    .px(px(10.))
                                    .flex()
                                    .items_center()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(text_2)
                                            .whitespace_nowrap()
                                            .child(db_path_str.clone()),
                                    )
                            })
                            // --- Change button ---
                            .child({
                                let state = state.clone();
                                let this = this.clone();
                                div()
                                    .h(px(28.))
                                    .px(px(10.))
                                    .rounded(px(7.))
                                    .bg(accent)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(move |s| s.opacity(0.85))
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, _cx| {
                                        let result = rfd::FileDialog::new()
                                            .set_file_name("clippi.db")
                                            .save_file();
                                        if let Some(new_path) = result {
                                            let path_str = new_path.to_string_lossy().to_string();
                                            let old = state.read(_cx).settings.resolve_db_path();
                                            if old == new_path {
                                                return;
                                            }
                                            // --- Checkpoint DB before migration ---
                                            {
                                                let s = state.read(_cx);
                                                if let Err(e) = s.db.checkpoint() {
                                                    log::error!(
                                                        "checkpoint failed before migration: {e}"
                                                    );
                                                }
                                            }
                                            match migrate_database(&old, &new_path) {
                                                Ok(()) => {
                                                    state.update(_cx, |s, _cx| {
                                                        s.settings.db_path = path_str;
                                                        s.settings.save();
                                                    });
                                                    crate::core::settings::spawn_new_process();
                                                    _cx.shutdown();
                                                }
                                                Err(e) => {
                                                    this.update(_cx, |_panel, cx| {
                                                        cx.emit(SettingsEvent::DataError(e));
                                                    });
                                                }
                                            }
                                        }
                                    })
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0xffffff))
                                            .child(I18nKey::BtnChange.text()),
                                    )
                            })
                            // --- Reset button ---
                            .child({
                                let state = state.clone();
                                let this = this.clone();
                                div()
                                    .h(px(28.))
                                    .px(px(10.))
                                    .rounded(px(7.))
                                    .bg(rgba(0x00000000))
                                    .border(px(1.))
                                    .border_color(text_3)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(move |s| s.bg(rgba(0xffffff10)))
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, _cx| {
                                        if crate::core::paths::is_portable_mode() {
                                            this.update(_cx, |panel, cx| {
                                                panel.show_reset_data_dialog(cx);
                                            });
                                        } else {
                                            let old = state.read(_cx).settings.resolve_db_path();
                                            let default_db =
                                                crate::core::paths::resolve_db_path("");
                                            if old == default_db {
                                                return;
                                            }
                                            {
                                                let s = state.read(_cx);
                                                if let Err(e) = s.db.checkpoint() {
                                                    log::error!(
                                                        "checkpoint failed before reset: {e}"
                                                    );
                                                }
                                            }
                                            match migrate_database(&old, &default_db) {
                                                Ok(()) => {
                                                    state.update(_cx, |s, _cx| {
                                                        s.settings.db_path = String::new();
                                                        s.settings.save();
                                                    });
                                                    crate::core::settings::spawn_new_process();
                                                    _cx.shutdown();
                                                }
                                                Err(e) => {
                                                    this.update(_cx, |_panel, cx| {
                                                        cx.emit(SettingsEvent::DataError(e));
                                                    });
                                                }
                                            }
                                        }
                                    })
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(text_3)
                                            .child(I18nKey::BtnReset.text()),
                                    )
                            }),
                    )
            })
            // --- ── Max items row (66px, standard row) ── ---
            .child({
                let surface = self.theme.surface;
                let divider = self.theme.divider;
                let text_1 = self.theme.text_1;
                let text_3 = self.theme.text_3;
                let input_bg = if self.theme.bg == rgb(0x191a1b) {
                    rgb(0x191a1b)
                } else {
                    rgb(0xf2f3f8)
                };

                div()
                    .h(px(66.))
                    .rounded(px(10.))
                    .bg(surface)
                    .border(px(1.))
                    .border_color(divider)
                    .px(px(14.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    // --- Left: label + description ---
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(text_1)
                                    .child(I18nKey::SettingMaxItems.text()),
                            )
                            .child(div().text_size(px(10.)).text_color(text_3).child(I18nKey::DescMaxItems.text())),
                    )
                    // --- Right: max-items value (button or Input) ---
                    .child({
                        let this = this.clone();

                        if self.editing_max_items {
                            // --- ── Editing: Input with Enter to save, blur auto-saves ── ---
                            div()
                                .w(px(80.))
                                .h(px(28.))
                                .rounded(px(7.))
                                .bg(input_bg)
                                .border(px(1.))
                                .border_color(self.theme.accent)
                                .px(px(6.))
                                .flex()
                                .items_center()
                                .child(
                                    Input::new(&self.max_items_input)
                                        .appearance(false)
                                        .bordered(false)
                                        .focus_bordered(false)
                                        .w_full()
                                        .h(px(20.))
                                        .text_size(px(12.))
                                        .text_color(text_1),
                                )
                                // --- Enter key saves (same pattern as tag_filter) ---
                                .on_key_down({
                                    move |ev: &KeyDownEvent, _window, cx| {
                                        if ev.keystroke.key.as_str() == "enter" {
                                            cx.stop_propagation();
                                            this.update(cx, |panel, cx| {
                                                panel.save_max_items(cx);
                                            });
                                        }
                                    }
                                })
                        } else {
                            // --- ── Normal: clickable value button ── ---
                            let val = self.state.read(cx).settings.max_items;
                            let label = if val == 0 {
                                I18nKey::Unlimited.text().to_string()
                            } else {
                                val.to_string()
                            };
                            div()
                                .w(px(80.))
                                .h(px(28.))
                                .rounded(px(7.))
                                .bg(input_bg)
                                .border(px(1.))
                                .border_color(divider)
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor(CursorStyle::PointingHand)
                                .on_mouse_down(MouseButton::Left, {
                                    let this = this.clone();
                                    move |_ev, _window, cx| {
                                        cx.stop_propagation();
                                        this.update(cx, |panel, cx| {
                                            panel.start_edit_max_items(_window, cx);
                                        });
                                    }
                                })
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(if val == 0 { text_3 } else { text_1 })
                                        .child(label),
                                )
                        }
                    })
            })
    }

    /// Show the reset-data-directory dialog. Only called in portable mode.
    pub fn show_reset_data_dialog(&mut self, cx: &mut Context<Self>) {
        let portable_path = crate::core::paths::resolve_db_path("")
            .to_string_lossy()
            .to_string();
        let system_path = {
            let data_dir = dirs::data_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("Clippi")
                .join("clippi.db");
            data_dir.to_string_lossy().to_string()
        };

        let app = self.state.read(cx);
        let currently_portable = app.settings.db_path.is_empty();

        self.reset_data_dialog = Some(ResetDataDirState {
            selected: if currently_portable {
                StorageMode::Portable
            } else {
                StorageMode::System
            },
            portable_path,
            system_path,
        });
        cx.notify();
    }

    /// Dismiss the reset-data-directory dialog.
    pub fn dismiss_reset_dialog(&mut self, cx: &mut Context<Self>) {
        self.reset_data_dialog = None;
        cx.notify();
    }

    /// Apply the selected reset target: migrate DB, update settings, restart.
    pub fn apply_reset_data_dir(&mut self, cx: &mut Context<Self>) {
        let dialog = match self.reset_data_dialog.take() {
            Some(d) => d,
            None => return,
        };

        let target_path = match dialog.selected {
            StorageMode::Portable => crate::core::paths::resolve_db_path(""),
            StorageMode::System => dirs::data_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("Clippi")
                .join("clippi.db"),
        };

        let old_path = self.state.read(cx).settings.resolve_db_path();
        if old_path == target_path {
            cx.notify();
            return;
        }

        {
            let s = self.state.read(cx);
            if let Err(e) = s.db.checkpoint() {
                log::error!("checkpoint failed before reset: {e}");
            }
        }

        match migrate_database(&old_path, &target_path) {
            Ok(()) => {
                let new_db_path = match dialog.selected {
                    StorageMode::Portable => String::new(),
                    StorageMode::System => target_path.to_string_lossy().to_string(),
                };
                self.state.update(cx, |s, _cx| {
                    s.settings.db_path = new_db_path;
                    s.settings.save();
                });
                crate::core::settings::spawn_new_process();
                cx.shutdown();
            }
            Err(e) => {
                cx.emit(SettingsEvent::DataError(e));
            }
        }
    }

    /// Render the reset-data-directory dialog overlay (portable mode only).
    pub fn render_reset_data_dialog(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let dialog = match &self.reset_data_dialog {
            Some(d) => d.clone(),
            None => return div().into_any_element(),
        };

        let surface = self.theme.surface;
        let accent = self.theme.accent;
        let accent_soft = self.theme.accent_soft;
        let text_1 = self.theme.text_1;
        let text_2 = self.theme.text_2;
        let text_3 = self.theme.text_3;
        let divider = self.theme.divider;

        let this = cx.entity().clone();

        div()
            .absolute()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, {
                let this = this.clone();
                move |_ev, _window, cx| {
                    cx.stop_propagation();
                    this.update(cx, |panel, cx| {
                        panel.dismiss_reset_dialog(cx);
                    });
                }
            })
            .child(
                div()
                    .w(px(300.))
                    .bg(surface)
                    .rounded(px(12.))
                    .border(px(1.))
                    .border_color(divider)
                    .p(px(16.))
                    .occlude()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    // --- Title ---
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(I18nKey::BtnResetDataDir.text()),
                    )
                    // --- Description ---
                    .child(div().text_size(px(12.)).text_color(text_3).child(I18nKey::DescStorageChoose.text()))
                    // --- Option: Portable ---
                    .child({
                        let selected = dialog.selected == StorageMode::Portable;
                        let this = this.clone();
                        div()
                            .rounded(px(8.))
                            .border(px(1.))
                            .border_color(if selected { accent } else { divider })
                            .bg(if selected {
                                accent_soft
                            } else {
                                rgba(0x00000000)
                            })
                            .p(px(10.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                cx.stop_propagation();
                                this.update(cx, |panel, cx| {
                                    if let Some(ref mut d) = panel.reset_data_dialog {
                                        d.selected = StorageMode::Portable;
                                    }
                                    cx.notify();
                                });
                            })
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(text_1)
                                    .child(I18nKey::StoragePortable.text()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(text_3)
                                    .child(dialog.portable_path.clone()),
                            )
                    })
                    // --- Option: System ---
                    .child({
                        let selected = dialog.selected == StorageMode::System;
                        let this = this.clone();
                        div()
                            .rounded(px(8.))
                            .border(px(1.))
                            .border_color(if selected { accent } else { divider })
                            .bg(if selected {
                                accent_soft
                            } else {
                                rgba(0x00000000)
                            })
                            .p(px(10.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                cx.stop_propagation();
                                this.update(cx, |panel, cx| {
                                    if let Some(ref mut d) = panel.reset_data_dialog {
                                        d.selected = StorageMode::System;
                                    }
                                    cx.notify();
                                });
                            })
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(text_1)
                                    .child(I18nKey::SystemDefault.text()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(text_3)
                                    .child(dialog.system_path.clone()),
                            )
                    })
                    // --- Button row ---
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.))
                            .mt(px(4.))
                            // --- Cancel ---
                            .child({
                                let this = this.clone();
                                div()
                                    .h(px(24.))
                                    .px(px(12.))
                                    .rounded(px(4.))
                                    .text_size(px(12.))
                                    .text_color(text_2)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(|s| s.bg(rgba(0xffffff10)))
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                        cx.stop_propagation();
                                        this.update(cx, |panel, cx| {
                                            panel.dismiss_reset_dialog(cx);
                                        });
                                    })
                                    .child(I18nKey::BtnCancel.text())
                            })
                            // --- Apply ---
                            .child({
                                let this = this.clone();
                                div()
                                    .h(px(24.))
                                    .px(px(12.))
                                    .rounded(px(4.))
                                    .text_size(px(12.))
                                    .text_color(rgb(0xffffff))
                                    .bg(accent)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(|s| s.opacity(0.85))
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                        cx.stop_propagation();
                                        this.update(cx, |panel, cx| {
                                            panel.apply_reset_data_dir(cx);
                                        });
                                    })
                                    .child(I18nKey::BtnApply.text())
                            }),
                    ),
            )
            .into_any_element()
    }
}
