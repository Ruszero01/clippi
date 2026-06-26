//! Clipboard settings tab — sort, card height, source app, scroll, copy, hover, OCR, QR.
//!
//! --- Matches the original Slint `SettingsTabClipboard.slint` layout. ---

use gpui::*;

use super::SettingsPanel;
use crate::core::i18n_keys::I18nKey;

impl SettingsPanel {
    pub fn render_clipboard_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let this = cx.entity().clone();

        // --- Snapshot current values from AppState ---
        let app = self.state.read(cx);
        let sort_by_created = app.settings.sort_by_created;
        let card_height_mode = app.settings.card_height_mode.clone();
        let show_source_app = app.settings.show_source_app;
        let auto_scroll_to_top = app.settings.auto_scroll_to_top;
        let copy_as_plain_text = app.settings.copy_as_plain_text;
        let show_original_on_hover = app.settings.show_original_on_hover;
        let ocr_enabled = app.settings.ocr_enabled;
        let qr_enabled = app.settings.qr_enabled;
        let auto_focus_search = app.settings.auto_focus_search;
        let auto_fetch_url_title = app.settings.auto_fetch_url_title;
        let filter_foreign_paths = app.settings.filter_foreign_paths;
        // --- borrow released here — `app` is a &AppState reference ---

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(8.))
            // --- Card height (4-option group) ---
            .child({
                let state = state.clone();
                let this = this.clone();
                self.setting_row_with_options(
                    I18nKey::SettingCardHeight.text(),
                    I18nKey::DescCardHeight.text(),
                    &[
                        ("high", I18nKey::CardHeightTall.text()),
                        ("medium", I18nKey::CardHeightMed.text()),
                        ("low", I18nKey::CardHeightShort.text()),
                        ("auto", I18nKey::CardHeightAuto.text()),
                    ],
                    &card_height_mode,
                    move |key, _window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.card_height_mode = key.to_string();
                            s.settings.save();
                        });
                        this.update(_cx, |_panel, cx| {
                            cx.emit(super::SettingsEvent::ClipboardSettingsChanged {
                                reload_items: false,
                                scroll_to_top: false,
                            });
                            cx.notify();
                        });
                    },
                )
            })
            // --- Sort by created ---
            .child(self.render_toggle_row(
                I18nKey::SettingSortCreated,
                I18nKey::DescSortFirst,
                I18nKey::DescSortLast,
                sort_by_created,
                window,
                cx,
                |state, this, _window, _cx| {
                    state.update(_cx, |s, _cx| {
                        s.settings.sort_by_created = !s.settings.sort_by_created;
                        s.settings.save();
                    });
                    this.update(_cx, |_panel, cx| {
                        cx.emit(super::SettingsEvent::ClipboardSettingsChanged {
                            reload_items: true,
                            scroll_to_top: false,
                        });
                    });
                },
            ))
            // --- Auto focus search ---
            .child(self.render_toggle_row(
                I18nKey::SettingAutoFocusSearch,
                I18nKey::DescAutoFocusSearchOn,
                I18nKey::DescAutoFocusSearchOff,
                auto_focus_search,
                window,
                cx,
                |state, _this, _window, _cx| {
                    state.update(_cx, |s, _cx| {
                        s.settings.auto_focus_search = !s.settings.auto_focus_search;
                        s.settings.save();
                    });
                },
            ))
            // --- Scroll to top ---
            .child(self.render_toggle_row(
                I18nKey::SettingScrollTop,
                I18nKey::DescScrollTopOn,
                I18nKey::DescScrollTopOff,
                auto_scroll_to_top,
                window,
                cx,
                |state, this, _window, _cx| {
                    let scroll_to_top = state.update(_cx, |s, _cx| {
                        s.settings.auto_scroll_to_top = !s.settings.auto_scroll_to_top;
                        s.settings.save();
                        s.settings.auto_scroll_to_top
                    });
                    this.update(_cx, |_panel, cx| {
                        cx.emit(super::SettingsEvent::ClipboardSettingsChanged {
                            reload_items: false,
                            scroll_to_top,
                        });
                    });
                },
            ))
            // --- Show source app ---
            .child(self.render_toggle_row(
                I18nKey::SettingShowSource,
                I18nKey::DescShowSourceOn,
                I18nKey::DescShowSourceOff,
                show_source_app,
                window,
                cx,
                |state, this, _window, _cx| {
                    state.update(_cx, |s, _cx| {
                        s.settings.show_source_app = !s.settings.show_source_app;
                        s.settings.save();
                    });
                    this.update(_cx, |_panel, cx| {
                        cx.emit(super::SettingsEvent::ClipboardSettingsChanged {
                            reload_items: false,
                            scroll_to_top: false,
                        });
                    });
                },
            ))
            // --- Filter foreign paths ---
            .child(self.render_toggle_row(
                I18nKey::SettingFilterForeignPaths,
                I18nKey::DescFilterForeignPathsOn,
                I18nKey::DescFilterForeignPathsOff,
                filter_foreign_paths,
                window,
                cx,
                |state, this, _window, _cx| {
                    state.update(_cx, |s, _cx| {
                        s.settings.filter_foreign_paths = !s.settings.filter_foreign_paths;
                        s.settings.save();
                    });
                    this.update(_cx, |_panel, cx| {
                        cx.emit(super::SettingsEvent::ClipboardSettingsChanged {
                            reload_items: true,
                            scroll_to_top: false,
                        });
                    });
                },
            ))
            // --- Copy as plain text ---
            .child(self.render_toggle_row(
                I18nKey::SettingCopyPlain,
                I18nKey::DescCopyPlainOn,
                I18nKey::DescCopyPlainOff,
                copy_as_plain_text,
                window,
                cx,
                |state, this, _window, _cx| {
                    state.update(_cx, |s, _cx| {
                        s.settings.copy_as_plain_text = !s.settings.copy_as_plain_text;
                        s.settings.save();
                    });
                    this.update(_cx, |_panel, cx| {
                        cx.emit(super::SettingsEvent::ClipboardSettingsChanged {
                            reload_items: false,
                            scroll_to_top: false,
                        });
                    });
                },
            ))
            // --- Show original on hover ---
            .child(self.render_toggle_row(
                I18nKey::SettingShowOriginal,
                I18nKey::DescShowOriginalOn,
                I18nKey::DescShowOriginalOff,
                show_original_on_hover,
                window,
                cx,
                |state, this, _window, _cx| {
                    state.update(_cx, |s, _cx| {
                        s.settings.show_original_on_hover = !s.settings.show_original_on_hover;
                        s.settings.save();
                    });
                    this.update(_cx, |_panel, cx| {
                        cx.emit(super::SettingsEvent::ClipboardSettingsChanged {
                            reload_items: false,
                            scroll_to_top: false,
                        });
                    });
                },
            ))
            // --- Auto fetch URL title ---
            .child(self.render_toggle_row(
                I18nKey::SettingAutoFetchUrlTitle,
                I18nKey::DescFetchUrlTitleOn,
                I18nKey::DescFetchUrlTitleOff,
                auto_fetch_url_title,
                window,
                cx,
                |state, this, _window, _cx| {
                    state.update(_cx, |s, _cx| {
                        s.settings.auto_fetch_url_title = !s.settings.auto_fetch_url_title;
                        s.settings.save();
                    });
                    this.update(_cx, |_panel, cx| {
                        cx.emit(super::SettingsEvent::ClipboardSettingsChanged {
                            reload_items: false,
                            scroll_to_top: false,
                        });
                    });
                },
            ))
            // --- Auto Image OCR ---
            .child(self.render_toggle_row(
                I18nKey::SettingOcr,
                I18nKey::DescOcrOn,
                I18nKey::DescOcrOff,
                ocr_enabled,
                window,
                cx,
                |state, _this, _window, _cx| {
                    state.update(_cx, |s, _cx| {
                        s.settings.ocr_enabled = !s.settings.ocr_enabled;
                        s.settings.save();
                    });
                },
            ))
            // --- Auto QR Detection ---
            .child(self.render_toggle_row(
                I18nKey::SettingQr,
                I18nKey::DescQrOn,
                I18nKey::DescQrOff,
                qr_enabled,
                window,
                cx,
                |state, _this, _window, _cx| {
                    state.update(_cx, |s, _cx| {
                        s.settings.qr_enabled = !s.settings.qr_enabled;
                        s.settings.save();
                    });
                },
            ))
    }
}
