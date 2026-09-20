//! Clipboard settings tab — sort, card height, source app, scroll, copy, hover, OCR, QR.

use crate::ui::font::fs;
use gpui::*;

use super::SettingsPanel;
use crate::core::i18n_keys::I18nKey;
use crate::services::copy_sound::{preview_sound, SOUND_LIST};
use crate::ui::components::slider::{SliderDetent, SteppedSlider};

/// Height of the collapsed copy-sound card (header only).
const SOUND_CARD_COLLAPSED: f32 = 52.0;
/// Height of the sound slider footer (track band + label band + padding).
const SOUND_FOOTER_HEIGHT: f32 = 58.0;
/// Height of the expanded copy-sound card: header + divider + slider footer.
const SOUND_CARD_EXPANDED: f32 = SOUND_CARD_COLLAPSED + 1.0 + SOUND_FOOTER_HEIGHT;

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
        let search_favorites_first = app.settings.search_favorites_first;
        let card_height_mode = app.settings.card_height_mode.clone();
        let paste_click_mode = app.settings.paste_click_mode_normalized();
        let show_source_app = app.settings.show_source_app;
        let auto_scroll_to_top = app.settings.auto_scroll_to_top;
        let copy_as_plain_text = app.settings.copy_as_plain_text;
        let show_original_on_hover = app.settings.show_original_on_hover;
        let ocr_enabled = app.settings.ocr_enabled;
        let qr_enabled = app.settings.qr_enabled;
        let auto_focus_search = app.settings.auto_focus_search;
        let clear_search_on_show = app.settings.clear_search_on_show;
        let auto_fetch_url_title = app.settings.auto_fetch_url_title;
        let filter_foreign_paths = app.settings.filter_foreign_paths;
        let copy_sound_enabled = app.settings.copy_sound_enabled;
        let copy_sound_file = app.settings.copy_sound_file.clone();
        // --- borrow released here — `app` is a &AppState reference ---

        let theme = self.theme.clone();
        let divider = theme.divider;
        let accent = theme.accent;
        let text_1 = theme.text_1;
        let text_3 = theme.text_3;

        let mut container = div().flex().flex_col().gap(px(14.)).pt(px(8.));

        // --- List & display ---
        let list_rows: Vec<AnyElement> = vec![
            self.render_toggle_row(
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
            )
            .into_any_element(),
            self.render_toggle_row(
                I18nKey::SettingSearchFavoritesFirst,
                I18nKey::DescSearchFavoritesFirstOn,
                I18nKey::DescSearchFavoritesFirstOff,
                search_favorites_first,
                window,
                cx,
                |state, this, _window, _cx| {
                    state.update(_cx, |s, _cx| {
                        s.settings.search_favorites_first = !s.settings.search_favorites_first;
                        s.settings.save();
                    });
                    this.update(_cx, |_panel, cx| {
                        cx.emit(super::SettingsEvent::ClipboardSettingsChanged {
                            reload_items: true,
                            scroll_to_top: false,
                        });
                    });
                },
            )
            .into_any_element(),
            self.render_toggle_row(
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
            )
            .into_any_element(),
            self.render_toggle_row(
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
            )
            .into_any_element(),
            self.render_toggle_row(
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
            )
            .into_any_element(),
            self.render_toggle_row(
                I18nKey::SettingClearSearchOnShow,
                I18nKey::DescClearSearchOnShowOn,
                I18nKey::DescClearSearchOnShowOff,
                clear_search_on_show,
                window,
                cx,
                |state, _this, _window, _cx| {
                    state.update(_cx, |s, _cx| {
                        s.settings.clear_search_on_show = !s.settings.clear_search_on_show;
                        s.settings.save();
                    });
                },
            )
            .into_any_element(),
        ];
        container =
            container.child(self.settings_group(I18nKey::GroupListDisplay.text(), list_rows));

        // --- Card layout ---
        let card_rows: Vec<AnyElement> = vec![{
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
        }
        .into_any_element()];
        container =
            container.child(self.settings_group(I18nKey::GroupCardLayout.text(), card_rows));

        // --- Copy & paste ---
        let paste_rows: Vec<AnyElement> = vec![
            {
                let state = state.clone();
                let this = this.clone();
                self.setting_row_with_options(
                    I18nKey::SettingPasteClickMode.text(),
                    I18nKey::DescPasteClickMode.text(),
                    &[
                        ("double_click", I18nKey::PasteClickModeDouble.text()),
                        ("single_click", I18nKey::PasteClickModeSingle.text()),
                    ],
                    &paste_click_mode,
                    move |key, _window, _cx| {
                        state.update(_cx, |s, _cx| {
                            s.settings.paste_click_mode = key.to_string();
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
            }
            .into_any_element(),
            self.render_toggle_row(
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
            )
            .into_any_element(),
            self.render_toggle_row(
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
            )
            .into_any_element(),
        ];
        container =
            container.child(self.settings_group(I18nKey::GroupCopyPaste.text(), paste_rows));

        // --- Content detection ---
        let detect_rows: Vec<AnyElement> = vec![
            self.render_toggle_row(
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
            )
            .into_any_element(),
            self.render_toggle_row(
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
            )
            .into_any_element(),
            self.render_toggle_row(
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
            )
            .into_any_element(),
        ];
        container =
            container.child(self.settings_group(I18nKey::GroupContentDetect.text(), detect_rows));

        // --- Sound ---
        let sound_rows: Vec<AnyElement> = vec![{
            let state = state.clone();
            let this = this.clone();
            let this_preview = this.clone();
            let enabled = copy_sound_enabled;
            let current = copy_sound_file.clone();
            // Plain digits: the label already sits inside a filled badge, so
            // the circled "①" forms would read as a circle inside a circle.
            let nums = ["1", "2", "3", "4", "5", "6"];
            let active_sound = SOUND_LIST
                .iter()
                .position(|(filename, _)| *filename == current)
                .unwrap_or(0);
            let gen = self.copy_sound_anim_gen;
            let key = gen.wrapping_add(if enabled { 1 } else { 0 } << 32);
            let card_h = Self::transition_f32(
                window,
                cx,
                ("copy-sound-h", key),
                if enabled {
                    SOUND_CARD_COLLAPSED
                } else {
                    SOUND_CARD_EXPANDED
                },
                if enabled {
                    SOUND_CARD_EXPANDED
                } else {
                    SOUND_CARD_COLLAPSED
                },
            );
            let footer_opacity = Self::transition_f32(
                window,
                cx,
                ("copy-sound-fo", key),
                if enabled { 0.0 } else { 1.0 },
                if enabled { 1.0 } else { 0.0 },
            );
            let drag = self.slider_drag("copy-sound-slider", active_sound);
            let colors = self.slider_colors();
            let detents: Vec<SliderDetent> = SOUND_LIST
                .iter()
                .enumerate()
                .map(|(i, _)| SliderDetent::major(nums.get(i).copied().unwrap_or("●")))
                .collect();

            div()
                .h(px(card_h))
                .overflow_hidden()
                .flex()
                .flex_col()
                // Header: label + toggle
                .child(
                    div()
                        .h(px(52.))
                        .flex_shrink_0()
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .child(
                                    div()
                                        .max_w_full()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_size(fs(12.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(text_1)
                                        .child(I18nKey::SettingCopySound.text()),
                                )
                                .child(
                                    div()
                                        .max_w_full()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_size(fs(10.))
                                        .text_color(text_3)
                                        .child(if enabled {
                                            I18nKey::DescCopySoundOn.text()
                                        } else {
                                            I18nKey::DescCopySoundOff.text()
                                        }),
                                ),
                        )
                        .child(div().flex_shrink_0().child(
                            crate::ui::components::toggle::render_toggle(
                                enabled,
                                "copy-sound",
                                crate::ui::components::toggle::ToggleColors {
                                    accent,
                                    track_off: divider,
                                },
                                &mut self.toggle_states,
                                window,
                                cx,
                                {
                                    let state = state.clone();
                                    let this = this.clone();
                                    move |_window, cx| {
                                        state.update(cx, |s, _cx| {
                                            s.settings.copy_sound_enabled =
                                                !s.settings.copy_sound_enabled;
                                            s.settings.save();
                                        });
                                        this.update(cx, |panel, cx| {
                                            panel.copy_sound_anim_gen =
                                                panel.copy_sound_anim_gen.wrapping_add(1);
                                            cx.emit(
                                                super::SettingsEvent::ClipboardSettingsChanged {
                                                    reload_items: false,
                                                    scroll_to_top: false,
                                                },
                                            );
                                        });
                                    }
                                },
                            ),
                        )),
                )
                // Footer: bare detent slider — no fill, the knob rides the
                // six numbered stops and each stop previews its sound.
                .child(
                    div()
                        .h(px(SOUND_FOOTER_HEIGHT))
                        .flex_shrink_0()
                        .opacity(footer_opacity)
                        .border_t(px(1.))
                        .border_color(divider)
                        .px(px(14.))
                        .flex()
                        .items_center()
                        .child(
                            SteppedSlider::new("copy-sound-slider", detents)
                                .value(active_sound)
                                .show_fill(false)
                                .badge_labels(true)
                                .label_inset(14.)
                                .drag_state(drag)
                                .colors(colors)
                                .on_change(move |index, _window, cx| {
                                    let Some(&(filename, _)) = SOUND_LIST.get(index) else {
                                        return;
                                    };
                                    preview_sound(filename);
                                    state.update(cx, |s, _cx| {
                                        s.settings.copy_sound_file = filename.to_string();
                                        s.settings.save();
                                    });
                                    this.update(cx, |_panel, cx| cx.notify());
                                })
                                .on_preview(move |_window, cx| {
                                    this_preview.update(cx, |_panel, cx| cx.notify());
                                }),
                        ),
                )
        }
        .into_any_element()];
        container = container.child(self.settings_group(I18nKey::GroupSound.text(), sound_rows));

        // --- Filtering ---
        let filter_rows: Vec<AnyElement> = vec![
            self.render_toggle_row(
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
            )
            .into_any_element(),
            {
                let this_click = this.clone();
                let count = self.state.read(cx).settings.clipboard_app_blacklist.len();
                let desc = I18nKey::ClipboardAppBlacklistCount.fmt(&[&count.to_string()]);
                self.setting_row_opener(
                    I18nKey::ClipboardAppBlacklistTitle.text(),
                    &desc,
                    move |_window, cx| {
                        this_click.update(cx, |panel, cx| {
                            panel.toggle_app_blacklist_popup();
                            cx.notify();
                        });
                    },
                )
            }
            .into_any_element(),
        ];
        container =
            container.child(self.settings_group(I18nKey::GroupFiltering.text(), filter_rows));

        container
    }
}
