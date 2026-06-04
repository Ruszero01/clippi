//! Clipboard settings tab — sort, card height, source app, scroll, copy, hover, OCR, QR.
//!
//! Matches the original Slint `SettingsTabClipboard.slint` layout.

use gpui::*;

use super::SettingsPanel;

impl SettingsPanel {
    pub fn render_clipboard_tab(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let this = cx.entity().clone();

        // Snapshot current values from AppState
        let app = self.state.read(cx);
        let sort_by_created = app.settings.sort_by_created;
        let card_height_mode = app.settings.card_height_mode.clone();
        let show_source_app = app.settings.show_source_app;
        let auto_scroll_to_top = app.settings.auto_scroll_to_top;
        let copy_as_plain_text = app.settings.copy_as_plain_text;
        let show_original_on_hover = app.settings.show_original_on_hover;
        let ocr_enabled = app.settings.ocr_enabled;
        let qr_enabled = app.settings.qr_enabled;
        drop(app);

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(8.))
            // ── Sort by created (dynamic desc) ──
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if sort_by_created { "First created" } else { "Last modified" };
                self.setting_row_with_toggle(
                    "Sort by created",
                    desc,
                    sort_by_created,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.sort_by_created = !s.settings.sort_by_created;
                            s.settings.save();
                        });
                        let _ = this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // ── Card height (4-option group) ──
            .child({
                let state = state.clone();
                self.setting_row_with_options(
                    "Card height",
                    "Adjust card height",
                    &[("high", "Tall"), ("medium", "Med"), ("low", "Short"), ("auto", "Auto")],
                    &card_height_mode,
                    move |key, _window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.card_height_mode = key.to_string();
                            s.settings.save();
                        });
                    },
                )
            })
            // ── Show source app (dynamic desc) ──
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if show_source_app { "Show source app icon" } else { "Show content type only" };
                self.setting_row_with_toggle(
                    "Show source app",
                    desc,
                    show_source_app,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.show_source_app = !s.settings.show_source_app;
                            s.settings.save();
                        });
                        let _ = this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // ── Scroll to top (dynamic desc) ──
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if auto_scroll_to_top { "Scroll to top on open" } else { "Keep last scroll position" };
                self.setting_row_with_toggle(
                    "Scroll to top",
                    desc,
                    auto_scroll_to_top,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.auto_scroll_to_top = !s.settings.auto_scroll_to_top;
                            s.settings.save();
                        });
                        let _ = this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // ── Copy as plain text (dynamic desc) ──
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if copy_as_plain_text { "Save as plain text only" } else { "Keep rich formatting" };
                self.setting_row_with_toggle(
                    "Copy as plain text",
                    desc,
                    copy_as_plain_text,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.copy_as_plain_text = !s.settings.copy_as_plain_text;
                            s.settings.save();
                        });
                        let _ = this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // ── Show original on hover (dynamic desc) ──
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if show_original_on_hover { "Show original on hover" } else { "Cards with notes show note" };
                self.setting_row_with_toggle(
                    "Show original on hover",
                    desc,
                    show_original_on_hover,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.show_original_on_hover = !s.settings.show_original_on_hover;
                            s.settings.save();
                        });
                        let _ = this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // ── Auto Image OCR (dynamic desc) ──
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if ocr_enabled { "Auto OCR for images" } else { "OCR disabled" };
                self.setting_row_with_toggle(
                    "Auto Image OCR",
                    desc,
                    ocr_enabled,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.ocr_enabled = !s.settings.ocr_enabled;
                            s.settings.save();
                        });
                        let _ = this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
            // ── Auto QR Detection (dynamic desc) ──
            .child({
                let state = state.clone();
                let this = this.clone();
                let desc = if qr_enabled { "Auto detect QR in images" } else { "QR detection disabled" };
                self.setting_row_with_toggle(
                    "Auto QR Detection",
                    desc,
                    qr_enabled,
                    move |_window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.qr_enabled = !s.settings.qr_enabled;
                            s.settings.save();
                        });
                        let _ = this.update(_cx, |_panel, cx| cx.notify());
                    },
                )
            })
    }
}
