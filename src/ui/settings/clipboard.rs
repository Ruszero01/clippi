//! Clipboard settings tab — sort, card height, source app, scroll, copy, hover, OCR, QR.
//!
//! --- Matches the original Slint `SettingsTabClipboard.slint` layout. ---

use gpui::*;

use crate::core::i18n_keys::I18nKey;
use super::SettingsPanel;

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
            // --- Sort by created (dynamic desc) ---
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if sort_by_created {
                    I18nKey::DescSortFirst.text()
                } else {
                    I18nKey::DescSortLast.text()
                };
                self.setting_row_with_toggle(
                    I18nKey::SettingSortCreated.text(),
                    desc,
                    sort_by_created,
                    window,
                    cx,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.sort_by_created = !s.settings.sort_by_created;
                            s.settings.save();
                        });
                        this.update(_cx, |_panel, cx| {
                            cx.emit(super::SettingsEvent::ClipboardSettingsChanged {
                                reload_items: true,
                                scroll_to_top: false,
                            });
                            cx.notify();
                        });
                    },
                )
            })
            // --- Auto focus search (dynamic desc) ---
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if auto_focus_search {
                    I18nKey::DescAutoFocusSearchOn.text()
                } else {
                    I18nKey::DescAutoFocusSearchOff.text()
                };
                self.setting_row_with_toggle(
                    I18nKey::SettingAutoFocusSearch.text(),
                    desc,
                    auto_focus_search,
                    window,
                    cx,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.auto_focus_search = !s.settings.auto_focus_search;
                            s.settings.save();
                        });
                        this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // --- Show source app (dynamic desc) ---
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if show_source_app {
                    I18nKey::DescShowSourceOn.text()
                } else {
                    I18nKey::DescShowSourceOff.text()
                };
                self.setting_row_with_toggle(
                    I18nKey::SettingShowSource.text(),
                    desc,
                    show_source_app,
                    window,
                    cx,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.show_source_app = !s.settings.show_source_app;
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
            // --- Scroll to top (dynamic desc) ---
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if auto_scroll_to_top {
                    I18nKey::DescScrollTopOn.text()
                } else {
                    I18nKey::DescScrollTopOff.text()
                };
                self.setting_row_with_toggle(
                    I18nKey::SettingScrollTop.text(),
                    desc,
                    auto_scroll_to_top,
                    window,
                    cx,
                    move |_window, _cx| {
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
                            cx.notify();
                        });
                    },
                )
            })
            // --- Copy as plain text (dynamic desc) ---
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if copy_as_plain_text {
                    I18nKey::DescCopyPlainOn.text()
                } else {
                    I18nKey::DescCopyPlainOff.text()
                };
                self.setting_row_with_toggle(
                    I18nKey::SettingCopyPlain.text(),
                    desc,
                    copy_as_plain_text,
                    window,
                    cx,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.copy_as_plain_text = !s.settings.copy_as_plain_text;
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
            // --- Show original on hover (dynamic desc) ---
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if show_original_on_hover {
                    I18nKey::DescShowOriginalOn.text()
                } else {
                    I18nKey::DescShowOriginalOff.text()
                };
                self.setting_row_with_toggle(
                    I18nKey::SettingShowOriginal.text(),
                    desc,
                    show_original_on_hover,
                    window,
                    cx,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.show_original_on_hover = !s.settings.show_original_on_hover;
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
            // --- Auto Image OCR (dynamic desc) ---
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if ocr_enabled {
                    I18nKey::DescOcrOn.text()
                } else {
                    I18nKey::DescOcrOff.text()
                };
                self.setting_row_with_toggle(
                    I18nKey::SettingOcr.text(),
                    desc,
                    ocr_enabled,
                    window,
                    cx,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.ocr_enabled = !s.settings.ocr_enabled;
                            s.settings.save();
                        });
                        this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // --- Auto QR Detection (dynamic desc) ---
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if qr_enabled {
                    I18nKey::DescQrOn.text()
                } else {
                    I18nKey::DescQrOff.text()
                };
                self.setting_row_with_toggle(
                    I18nKey::SettingQr.text(),
                    desc,
                    qr_enabled,
                    window,
                    cx,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.qr_enabled = !s.settings.qr_enabled;
                            s.settings.save();
                        });
                        this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
    }
}
