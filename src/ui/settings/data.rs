//! --- Data settings tab — database path, max items, retention days. ---
//!
//! --- Mirrors the original Slint `SettingsTabData.slint` layout. ---
//! Includes the reset-data-directory dialog for portable mode.

use chrono::Datelike;
use gpui::prelude::FluentBuilder;
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

    /// Enter editing mode for the retention-days field.
    fn start_edit_retention_days(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.state.read(cx).settings.retention_days;
        let text = if current == 0 {
            String::new()
        } else {
            current.to_string()
        };
        self.retention_days_input.update(cx, |input, cx| {
            input.set_value(&text, window, cx);
        });
        self.editing_retention_days = true;
        cx.notify();
    }

    /// Save the retention-days value and exit editing mode.
    fn save_retention_days(&mut self, cx: &mut Context<Self>) {
        let text = self.retention_days_input.read(cx).value().to_string();
        let n: u32 = text.trim().parse().unwrap_or(0);
        self.state.update(cx, |s, _cx| {
            s.settings.retention_days = n;
            s.settings.save();
        });
        self.editing_retention_days = false;
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
                                        let dialog = rfd::AsyncFileDialog::new()
                                            .set_file_name("clippi.db")
                                            .save_file();
                                        let state = state.clone();
                                        let task_panel = this.clone();
                                        let task = _cx.spawn(async move |cx| {
                                            let Some(new_path) = dialog.await else {
                                                return;
                                            };
                                            let new_path = new_path.path().to_path_buf();
                                            let path_str = new_path.to_string_lossy().to_string();
                                            let old = match cx.read_entity(&state, |s, _cx| {
                                                s.settings.resolve_db_path()
                                            }) {
                                                Ok(old) => old,
                                                Err(e) => {
                                                    log::error!("failed to read settings: {e}");
                                                    return;
                                                }
                                            };
                                            if old == new_path {
                                                return;
                                            }
                                            // --- Checkpoint DB before migration ---
                                            match cx.read_entity(&state, |s, _cx| s.db.checkpoint())
                                            {
                                                Ok(Err(e)) => {
                                                    log::error!(
                                                        "checkpoint failed before migration: {e}"
                                                    );
                                                }
                                                Err(e) => {
                                                    log::error!("failed to read database: {e}");
                                                    return;
                                                }
                                                Ok(Ok(())) => {}
                                            }
                                            match migrate_database(&old, &new_path) {
                                                Ok(()) => {
                                                    let _ = state.update(cx, |s, _cx| {
                                                        s.settings.db_path = path_str;
                                                        s.settings.save();
                                                    });
                                                    crate::core::settings::spawn_new_process();
                                                    let _ = cx.update(|cx| cx.quit());
                                                }
                                                Err(e) => {
                                                    let _ = task_panel.update(cx, |_panel, cx| {
                                                        cx.emit(SettingsEvent::DataError(e));
                                                    });
                                                }
                                            }
                                        });
                                        this.update(_cx, |panel, _cx| {
                                            panel._db_path_dialog_task = Some(task);
                                        });
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
                                        this.update(_cx, |panel, cx| {
                                            panel.show_reset_data_dialog(cx);
                                        });
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
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(text_3)
                                    .child(I18nKey::DescMaxItems.text()),
                            ),
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
                            div()
                                .w(px(90.))
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
                                .child(if val == 0 {
                                    div()
                                        .text_size(px(12.))
                                        .text_color(text_3)
                                        .child(I18nKey::Unlimited.text())
                                        .into_any_element()
                                } else {
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(2.))
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(text_1)
                                                .child(val.to_string()),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(text_3)
                                                .child(I18nKey::UnitItems.text()),
                                        )
                                        .into_any_element()
                                })
                        }
                    })
            })
            // --- ── Retention days row (66px, standard row) ── ---
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
                                    .child(I18nKey::SettingRetentionDays.text()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(text_3)
                                    .child(I18nKey::DescRetentionDays.text()),
                            ),
                    )
                    // --- Right: retention-days value (button or Input) ---
                    .child({
                        let this = this.clone();

                        if self.editing_retention_days {
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
                                    Input::new(&self.retention_days_input)
                                        .appearance(false)
                                        .bordered(false)
                                        .focus_bordered(false)
                                        .w_full()
                                        .h(px(20.))
                                        .text_size(px(12.))
                                        .text_color(text_1),
                                )
                                .on_key_down({
                                    move |ev: &KeyDownEvent, _window, cx| {
                                        if ev.keystroke.key.as_str() == "enter" {
                                            cx.stop_propagation();
                                            this.update(cx, |panel, cx| {
                                                panel.save_retention_days(cx);
                                            });
                                        }
                                    }
                                })
                        } else {
                            let val = self.state.read(cx).settings.retention_days;
                            div()
                                .w(px(90.))
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
                                            panel.start_edit_retention_days(_window, cx);
                                        });
                                    }
                                })
                                .child(if val == 0 {
                                    div()
                                        .text_size(px(12.))
                                        .text_color(text_3)
                                        .child(I18nKey::Unlimited.text())
                                        .into_any_element()
                                } else {
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(2.))
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(text_1)
                                                .child(val.to_string()),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(text_3)
                                                .child(I18nKey::UnitDays.text()),
                                        )
                                        .into_any_element()
                                })
                        }
                    })
            })
            // --- ── Cache cleanup card ── ---
            .child({
                let state = self.state.clone();
                let this = cx.entity().clone();
                let cleanup_interval = self.state.read(cx).settings.cleanup_interval.clone();

                let surface = self.theme.surface;
                let divider = self.theme.divider;
                let text_1 = self.theme.text_1;
                let text_2 = self.theme.text_2;
                let text_3 = self.theme.text_3;
                let accent = self.theme.accent;

                div()
                    .rounded(px(10.))
                    .bg(surface)
                    .border(px(1.))
                    .border_color(divider)
                    .px(px(14.))
                    .pt(px(14.))
                    .pb(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    // --- Label + description ---
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
                                    .child(I18nKey::SettingCleanup.text()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(text_3)
                                    .child(I18nKey::DescCleanup.text()),
                            ),
                    )
                    // --- Frequency buttons + clean-now button ---
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            // Frequency selector buttons
                            .child({
                                let state = state.clone();
                                let options: &[(&str, &str)] = &[
                                    ("never", I18nKey::CleanupIntervalNever.text()),
                                    ("daily", I18nKey::CleanupIntervalDaily.text()),
                                    ("weekly", I18nKey::CleanupIntervalWeekly.text()),
                                ];
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap(px(4.))
                                    .children(options.iter().map({
                                        let cleanup_interval = cleanup_interval.clone();
                                        move |(key, label)| {
                                            let selected = *key == cleanup_interval;
                                            let btn_bg =
                                                if selected { accent } else { rgba(0x00000000) };
                                            let btn_text =
                                                if selected { rgb(0xffffff) } else { text_2 };
                                            let btn_weight = if selected {
                                                FontWeight::BOLD
                                            } else {
                                                FontWeight::default()
                                            };
                                            let key = *key;
                                            let state = state.clone();

                                            div()
                                                .h(px(26.))
                                                .rounded(px(7.))
                                                .px(px(8.))
                                                .bg(btn_bg)
                                                .when(!selected, |d| {
                                                    d.border(px(1.)).border_color(divider)
                                                })
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .cursor(CursorStyle::PointingHand)
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    move |_ev, _window, cx| {
                                                        cx.stop_propagation();
                                                        state.update(cx, |s, _cx| {
                                                            s.settings.cleanup_interval =
                                                                key.to_string();
                                                            s.settings.save();
                                                        });
                                                    },
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(11.))
                                                        .font_weight(btn_weight)
                                                        .text_color(btn_text)
                                                        .child(*label),
                                                )
                                        }
                                    }))
                            })
                            // Clean now button
                            .child({
                                let state = state.clone();
                                let this = this.clone();
                                div()
                                    .h(px(26.))
                                    .px(px(10.))
                                    .rounded(px(7.))
                                    .bg(accent)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(|s| s.opacity(0.85))
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                        cx.stop_propagation();
                                        let settings = state.read(cx).settings.clone();
                                        let db_path = settings.resolve_db_path();
                                        let retention_days = settings.retention_days;
                                        let stats = match crate::core::db::Database::open(
                                            &db_path.to_string_lossy(),
                                        ) {
                                            Ok(db) => {
                                                let scope =
                                                    crate::core::cache_cleanup::CleanupSyncScope {
                                                        include_images: settings.sync_include_images,
                                                        favorites_only: settings.sync_favorites_only,
                                                        device_name: crate::services::backends::local_folder::hostname(),
                                                    };
                                                crate::core::cache_cleanup::run_cleanup(
                                                    &db,
                                                    retention_days,
                                                    Some(&scope),
                                                )
                                            }
                                            Err(e) => {
                                                log::error!("cleanup: failed to open DB: {e}");
                                                return;
                                            }
                                        };
                                        // Update last cleanup marker using the active interval format.
                                        let interval = settings.cleanup_interval.as_str();
                                        let last_cleanup = match interval {
                                            "weekly" => {
                                                let wk = chrono::Local::now().iso_week();
                                                format!("{}-W{:02}", wk.year(), wk.week())
                                            }
                                            "daily" => {
                                                chrono::Local::now().format("%Y-%m-%d").to_string()
                                            }
                                            _ => {
                                                chrono::Local::now().format("%Y-%m-%d").to_string()
                                            }
                                        };
                                        state.update(cx, |s, _cx| {
                                            s.settings.cleanup_last_date = last_cleanup;
                                            if stats.sync_dirty {
                                                s.sync_dirty.store(true, std::sync::atomic::Ordering::SeqCst);
                                            }
                                            s.pending_hotkey_unregister
                                                .extend(stats.expired_hotkey_item_ids.iter().copied());
                                            s.settings.save();
                                            if stats.expired_items > 0 {
                                                s.reload_items();
                                                s.reload_tags();
                                            }
                                        });
                                        // Show toast
                                        if stats.is_empty() {
                                            this.update(cx, |_panel, cx| {
                                                cx.emit(SettingsEvent::DataToast(
                                                    I18nKey::ToastCleanupNone.text().to_string(),
                                                ));
                                            });
                                        } else {
                                            let total_records =
                                                stats.expired_tombstones + stats.expired_items;
                                            let msg = I18nKey::ToastCleanupDone.fmt(&[
                                                &stats.orphan_images.to_string(),
                                                &stats.unreferenced_icons.to_string(),
                                                &total_records.to_string(),
                                            ]);
                                            this.update(cx, |_panel, cx| {
                                                cx.emit(SettingsEvent::DataToast(msg));
                                            });
                                        }
                                    })
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0xffffff))
                                            .child(I18nKey::BtnCleanupNow.text()),
                                    )
                            }),
                    )
            })
    }

    /// Show the reset-data-directory dialog. Available regardless of portable mode.
    pub fn show_reset_data_dialog(&mut self, cx: &mut Context<Self>) {
        let portable_path = crate::core::paths::portable_db_path()
            .to_string_lossy()
            .to_string();
        let system_path = crate::core::paths::system_data_dir()
            .join("clippi.db")
            .to_string_lossy()
            .to_string();

        let currently_portable = crate::core::paths::is_portable_mode();

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

    /// Apply the selected reset target: migrate DB, save config to target
    /// location, and restart.
    ///
    /// When the target database already exists, smart-merges data instead of
    /// overwriting — this prevents data loss when both locations have
    /// accumulated clipboard history.
    ///
    /// Saves the config file explicitly to the chosen directory rather than
    /// delegating to `AppSettings::save()`, because `config_path()` depends on
    /// `is_portable_mode()` which may not reflect the user's new choice yet
    /// (e.g. switching TO portable when `is_portable_mode()` is still false).
    pub fn apply_reset_data_dir(&mut self, cx: &mut Context<Self>) {
        let dialog = match self.reset_data_dialog.take() {
            Some(d) => d,
            None => return,
        };

        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let sys_data = crate::core::paths::system_data_dir();

        let (target_db, target_config, new_db_path) = match dialog.selected {
            StorageMode::Portable => (
                exe_dir.join("clippi.db"),
                exe_dir.join("clippi.toml"),
                String::new(),
            ),
            StorageMode::System => {
                let db = sys_data.join("clippi.db");
                let cfg = sys_data.join("clippi.toml");
                (db.clone(), cfg, db.to_string_lossy().to_string())
            }
        };

        let old_path = self.state.read(cx).settings.resolve_db_path();
        if old_path == target_db {
            cx.notify();
            return;
        }

        // Checkpoint the current WAL so we have a consistent DB file to work with.
        {
            let s = self.state.read(cx);
            if let Err(e) = s.db.checkpoint() {
                log::error!("checkpoint failed before reset: {e}");
            }
        }

        // ── Determine merge vs. fresh-migrate ──
        if target_db.exists() {
            // ── Smart merge: target already has data ──
            self.apply_reset_with_merge(
                &old_path,
                &target_db,
                &target_config,
                &new_db_path,
                &exe_dir,
                dialog.selected,
                cx,
            );
        } else {
            // ── Fresh migration: target is empty ──
            match migrate_database(&old_path, &target_db) {
                Ok(()) => {
                    self.finalize_reset(
                        &target_config,
                        &new_db_path,
                        &exe_dir,
                        dialog.selected,
                        cx,
                    );
                }
                Err(e) => {
                    cx.emit(SettingsEvent::DataError(e));
                }
            }
        }
    }

    /// Merge the current database + config into an already-existing target.
    #[allow(clippy::too_many_arguments)]
    fn apply_reset_with_merge(
        &mut self,
        old_path: &std::path::Path,
        target_db: &std::path::Path,
        target_config: &std::path::Path,
        new_db_path: &str,
        exe_dir: &std::path::Path,
        selected: StorageMode,
        cx: &mut Context<Self>,
    ) {
        // ── 1. Open the target database and merge the current DB into it. ──
        match crate::core::db::Database::open(&target_db.to_string_lossy()) {
            Ok(target) => {
                match target.merge_from(old_path) {
                    Ok(stats) => {
                        log::info!(
                            "reset: merged DB — items +{}/~{} tags +{}/~{}",
                            stats.items_added,
                            stats.items_updated,
                            stats.tags_added,
                            stats.tags_updated,
                        );
                    }
                    Err(e) => {
                        log::error!("reset: DB merge failed, falling back to copy: {e}");
                        // Fall back to simple copy on merge failure.
                        if let Err(e2) = migrate_database(old_path, target_db) {
                            cx.emit(SettingsEvent::DataError(e2));
                            return;
                        }
                    }
                }
            }
            Err(e) => {
                log::error!("reset: failed to open target DB, falling back to copy: {e}");
                if let Err(e2) = migrate_database(old_path, target_db) {
                    cx.emit(SettingsEvent::DataError(e2));
                    return;
                }
            }
        }

        // ── 2. Merge config files if the target already has one. ──
        let source_settings = self.state.read(cx).settings.clone();
        let merged = if target_config.exists() {
            match std::fs::read_to_string(target_config) {
                Ok(content) => match toml::from_str::<crate::core::settings::AppSettings>(&content)
                {
                    Ok(target_settings) => crate::core::settings::merge_configs(
                        &source_settings,
                        &target_settings,
                        new_db_path,
                    ),
                    Err(e) => {
                        log::warn!("reset: failed to parse target config, using source: {e}");
                        let mut s = source_settings.clone();
                        s.db_path = new_db_path.to_string();
                        s
                    }
                },
                Err(e) => {
                    log::warn!("reset: failed to read target config, using source: {e}");
                    let mut s = source_settings.clone();
                    s.db_path = new_db_path.to_string();
                    s
                }
            }
        } else {
            let mut s = source_settings;
            s.db_path = new_db_path.to_string();
            s
        };
        self.state.update(cx, |s, _cx| {
            s.settings = merged;
        });

        // ── 3. Merge images directories. ──
        let copied = crate::core::paths::merge_images_dir(old_path, target_db);
        if copied > 0 {
            log::info!("reset: copied {copied} image files");
        }

        self.finalize_reset(target_config, new_db_path, exe_dir, selected, cx);
    }

    /// Save config to target location, clean up exe-dir config if switching to
    /// system mode, spawn new process, and quit.
    fn finalize_reset(
        &mut self,
        target_config: &std::path::Path,
        new_db_path: &str,
        exe_dir: &std::path::Path,
        selected: StorageMode,
        cx: &mut Context<Self>,
    ) {
        // Update in-memory db_path (may have been overridden by merge).
        self.state.update(cx, |s, _cx| {
            s.settings.db_path = new_db_path.to_string();
        });

        // Save config to the target location explicitly.
        let settings = self.state.read(cx).settings.clone();
        if let Ok(content) = toml::to_string_pretty(&settings) {
            if let Some(parent) = target_config.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(target_config, content);
        }

        // When switching to system mode, remove the old config from the
        // exe dir so that is_portable_mode() returns false on next startup.
        if selected == StorageMode::System {
            let _ = std::fs::remove_file(exe_dir.join("clippi.toml"));
        }

        crate::core::settings::spawn_new_process();
        cx.quit();
    }

    /// Render the reset-data-directory dialog overlay.
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
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(text_3)
                            .child(I18nKey::DescStorageChoose.text()),
                    )
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
