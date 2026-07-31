//! Tag filter panel — floating panel for filtering clipboard items by tag.
//!
//! --- Matches the original Slint TagFilterPanel.slint design: ---
//! --- - Title row with AND/OR mode toggle, clear, close buttons ---
//! --- - Tag creation input with "+" button (real gpui_component Input) ---
//! --- - 2-column tag grid (140px each, 30px height) ---
//! --- - Edit/delete buttons per tag row ---
//! - TagEditPanel overlay for editing name/color

use std::rc::Rc;

use gpui::prelude::*;
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::tooltip::Tooltip;

use crate::core::i18n_keys::I18nKey;
use crate::core::types::{tag_preset_colors, TagInfo};
use crate::state::app::{pinyin_match, AppState};

use super::clipboard_list::ClipboardListView;
use super::search_bar::SearchBar;
use super::theme::ClippiTheme;

pub struct TagFilterPanel {
    state: Entity<AppState>,
    list_view: Entity<ClipboardListView>,
    search_bar: Entity<SearchBar>,
    create_input: Entity<InputState>,
    edit_name_input: Entity<InputState>,
    last_edit_tag_id: i64,
    last_lang_version: u64,
}

impl TagFilterPanel {
    pub fn new(
        state: Entity<AppState>,
        list_view: Entity<ClipboardListView>,
        search_bar: Entity<SearchBar>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let create_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(I18nKey::TagCreatePlaceholder.text())
        });
        let edit_name_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(I18nKey::TagCreatePlaceholder.text())
        });

        Self {
            state,
            list_view,
            search_bar,
            create_input,
            edit_name_input,
            last_edit_tag_id: -1,
            last_lang_version: crate::core::i18n::lang_version(),
        }
    }

    pub(crate) fn refresh_list(&self, cx: &mut App) {
        let items = self.state.read(cx).visible_items();
        self.list_view.update(cx, |l, cx| l.set_items(items, cx));
        self.search_bar.update(cx, |_b, cx| cx.notify());
    }

    pub(crate) fn cancel_edit_tag(&self, cx: &mut App) {
        self.state.update(cx, |s, _cx| s.cancel_edit_tag());
    }

    pub(crate) fn set_edit_tag_color(&self, color: &str, cx: &mut App) {
        self.state.update(cx, |s, _cx| s.set_edit_tag_color(color));
    }

    pub(crate) fn update_tag(&self, tag_id: i64, name: &str, color: &str, cx: &mut App) {
        self.state
            .update(cx, |s, _cx| s.update_tag(tag_id, name, color));
        self.refresh_list(cx);
    }

    fn toggle_filter(&self, tag_id: i64, cx: &mut App) {
        self.state.update(cx, |s, _cx| {
            s.toggle_tag_filter(tag_id);
        });
        self.refresh_list(cx);
    }

    fn clear_filters(&self, cx: &mut App) {
        self.state.update(cx, |s, _cx| {
            s.clear_tag_filters();
        });
        self.refresh_list(cx);
    }

    fn toggle_mode(&self, cx: &mut App) {
        self.state.update(cx, |s, _cx| {
            s.toggle_tag_match_mode();
        });
        self.refresh_list(cx);
    }

    fn close(&self, cx: &mut App) {
        self.search_bar
            .update(cx, |bar, cx| bar.close_tag_panel(cx));
    }

    fn create_tag(&self, name: &str, cx: &mut App) {
        self.state.update(cx, |s, _cx| s.create_tag(name));
        self.refresh_list(cx);
    }

    /// Read the create input value.
    /// If the name exactly matches an existing tag → toggle its filter;
    /// otherwise create a new tag. Clear the input afterwards.
    fn create_tag_from_input(&self, window: &mut Window, cx: &mut App) {
        let name = self.create_input.read(cx).value().to_string();
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            let tag_id = {
                let app_state = self.state.read(cx);
                app_state
                    .tags
                    .iter()
                    .find(|t| t.name.eq_ignore_ascii_case(trimmed))
                    .map(|t| t.id)
            };
            if let Some(tag_id) = tag_id {
                self.toggle_filter(tag_id, cx);
            } else {
                self.create_tag(trimmed, cx);
            }
        }
        self.create_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
    }

    pub fn edit_name_input(&self) -> &Entity<InputState> {
        &self.edit_name_input
    }

    fn delete_tag(&self, tag_id: i64, cx: &mut App) {
        self.state.update(cx, |s, _cx| s.delete_tag(tag_id));
        self.refresh_list(cx);
    }
}

impl Render for TagFilterPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 语言切换时刷新 InputState placeholder
        let current = crate::core::i18n::lang_version();
        if self.last_lang_version != current {
            self.last_lang_version = current;
            self.create_input.update(cx, |state, cx| {
                state.set_placeholder(I18nKey::TagCreatePlaceholder.text(), window, cx);
            });
            self.edit_name_input.update(cx, |state, cx| {
                state.set_placeholder(I18nKey::TagCreatePlaceholder.text(), window, cx);
            });
        }

        let app_state = self.state.read(cx);
        // --- Filter tags by input text for live search (pinyin-aware) ---
        let filter_text = self.create_input.read(cx).value();
        let pinned_tag_ids = app_state.settings.pinned_tag_ids.clone();
        let tags: Vec<(TagInfo, bool, bool)> = app_state
            .tags
            .iter()
            .filter(|t| filter_text.is_empty() || pinyin_match(&t.name, &filter_text))
            .map(|t| {
                (
                    t.clone(),
                    app_state.filters.tag_ids.contains(&t.id),
                    pinned_tag_ids.contains(&t.id),
                )
            })
            .collect();
        let tag_match_all = app_state.filters.is_tag_match_all();
        let has_tag_filter = !app_state.filters.tag_ids.is_empty();
        let theme = ClippiTheme::from_setting(&app_state.settings.theme, Some(window.appearance()));
        let _ = app_state;

        let is_dark = theme.bg == rgb(0x191a1b);
        let accent = theme.accent;
        let text_1 = theme.text_1;
        let text_2 = theme.text_2;
        let text_3 = theme.text_3;
        let surface = theme.panel_surface;
        let input_bg = theme.panel_input_bg;
        let sep_line = theme.panel_sep_line;
        let btn_hover = theme.btn_hover;
        let accent_hover_bg = if is_dark {
            rgba(0x7ecba335)
        } else {
            rgba(0x6ab89035)
        };
        let panel_border = if is_dark {
            rgba(0xffffff14)
        } else {
            rgba(0x00000012)
        };
        let active_bg = theme.accent_overlay();
        let danger_hover_bg = if is_dark {
            rgba(0xff5f5720)
        } else {
            rgba(0xff5f5718)
        };

        let rows: Vec<Vec<(TagInfo, bool, bool)>> = tags.chunks(2).map(|r| r.to_vec()).collect();
        let rows_is_empty = rows.is_empty();
        let this_entity = cx.entity().clone();

        div()
            .flex()
            .flex_col()
            .w(px(304.))
            .max_h(px(360.))
            .bg(surface)
            .border_color(panel_border)
            .border(px(1.))
            .rounded(px(8.))
            .shadow_lg()
            .pt(px(8.))
            .pb(px(8.))
            .pl(px(8.))
            .gap(px(4.))
            // --- Title row ---
            .child({
                let this = this_entity.clone();
                div()
                    .pr(px(8.))
                    .h(px(24.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text_1)
                            .child(I18nKey::TagFilterTitle.text()),
                    )
                    .child(div().flex_1())
                    .child(icon_btn(
                        "\u{e61e}",
                        if tag_match_all { accent } else { text_2 },
                        btn_hover,
                        Some(I18nKey::TagTooltipToggleMode.text()),
                        {
                            let this = this.clone();
                            move |_, cx| this.update(cx, |panel, cx| panel.toggle_mode(cx))
                        },
                    ))
                    .when(has_tag_filter, |el| {
                        el.child(icon_btn(
                            "\u{e62e}",
                            text_2,
                            btn_hover,
                            Some(I18nKey::TagTooltipClear.text()),
                            {
                                let this = this.clone();
                                move |_, cx| this.update(cx, |panel, cx| panel.clear_filters(cx))
                            },
                        ))
                    })
                    .child(icon_btn("\u{e7b7}", text_2, btn_hover, None, {
                        let this = this.clone();
                        move |_, cx| this.update(cx, |panel, cx| panel.close(cx))
                    }))
            })
            .child(div().h(px(1.)).w_full().bg(sep_line))
            // --- Create tag row (real Input) ---
            .child({
                let this = this_entity.clone();
                div()
                    .pr(px(8.))
                    .h(px(26.))
                    .flex()
                    .flex_row()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex_1()
                            .h(px(26.))
                            .rounded(px(5.))
                            .bg(input_bg)
                            .px(px(7.))
                            .flex()
                            .items_center()
                            .child(
                                Input::new(&self.create_input)
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .w_full()
                                    .h(px(20.))
                                    .text_size(px(11.))
                                    .text_color(text_1),
                            )
                            // --- Handle Enter key on the parent div — uses the raw ---
                            // --- KeyDownEvent (like the clipboard note editor does), ---
                            // --- bypassing InputEvent::PressEnter subscriptions ---
                            // which cause stack corruption during action dispatch.
                            .on_key_down({
                                let this = this.clone();
                                move |ev: &KeyDownEvent, window, cx| {
                                    if ev.keystroke.key.as_str() == "enter" {
                                        cx.stop_propagation();
                                        this.update(cx, |panel, cx| {
                                            panel.create_tag_from_input(window, cx);
                                            cx.notify();
                                        });
                                    }
                                }
                            }),
                    )
                    .child({
                        let this = this.clone();
                        div()
                            .w(px(26.))
                            .h(px(26.))
                            .rounded(px(5.))
                            .bg(btn_hover)
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(move |style| style.bg(accent_hover_bg))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(accent)
                                    .child(I18nKey::TagFilterAdd.text()),
                            )
                            .on_mouse_down(MouseButton::Left, {
                                let this = this.clone();
                                move |_ev, window, cx| {
                                    this.update(cx, |panel, cx| {
                                        panel.create_tag_from_input(window, cx);
                                        cx.notify();
                                    });
                                }
                            })
                    })
            })
            .child(div().h(px(1.)).w_full().bg(sep_line))
            // --- Tag list ---
            .child(
                div()
                    .flex()
                    .flex_col()
                    .max_h(px(200.))
                    .overflow_y_scrollbar()
                    .child(div().pr(px(8.)).flex().flex_col().gap(px(4.)).children(
                        rows.into_iter().map(|row| {
                            let this = this_entity.clone();
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(4.))
                                .children(row.into_iter().map(|(tag, checked, pinned)| {
                                    let tag_id = tag.id;
                                    let tag_color = parse_hex_to_rgba(&tag.color);
                                    let tag_name = tag.name.clone();
                                    let tag_name_edit = tag.name.clone();
                                    let tag_color_hex = tag.color.clone();
                                    let this = this.clone();
                                    div()
                                        .w(px(140.))
                                        .h(px(30.))
                                        .rounded(px(5.))
                                        .bg(if checked { active_bg } else { rgba(0x00000000) })
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(5.))
                                        .px(px(6.))
                                        .cursor(CursorStyle::PointingHand)
                                        .hover(move |style| {
                                            if checked {
                                                style.bg(active_bg)
                                            } else {
                                                style.bg(btn_hover)
                                            }
                                        })
                                        .on_mouse_down(MouseButton::Left, {
                                            let this = this.clone();
                                            move |_ev, _window, cx| {
                                                this.update(cx, |panel, cx| {
                                                    panel.toggle_filter(tag_id, cx)
                                                });
                                            }
                                        })
                                        .child(if pinned {
                                            // Pin icon replacing color dot for pinned tags
                                            div()
                                                .w(px(10.))
                                                .h(px(10.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_size(px(12.))
                                                .font_family("iconfont")
                                                .text_color(tag_color)
                                                .child("\u{e633}")
                                        } else {
                                            div()
                                                .w(px(10.))
                                                .h(px(10.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(
                                                    div()
                                                        .w(px(8.))
                                                        .h(px(8.))
                                                        .rounded_full()
                                                        .bg(tag_color),
                                                )
                                        })
                                        .child(
                                            div()
                                                .flex_1()
                                                .text_size(px(11.))
                                                .font_weight(if checked {
                                                    FontWeight::SEMIBOLD
                                                } else {
                                                    FontWeight::default()
                                                })
                                                .text_color(if checked { accent } else { text_1 })
                                                .truncate()
                                                .child(tag_name),
                                        )
                                        .child(small_btn(
                                            "\u{e679}",
                                            text_3,
                                            false,
                                            btn_hover,
                                            danger_hover_bg,
                                            {
                                                let this = this.clone();
                                                move |w, cx| {
                                                    // --- Set edit name input value before updating state ---
                                                    let input =
                                                        this.read(cx).edit_name_input.clone();
                                                    input.update(cx, |input, cx| {
                                                        input.set_value(&tag_name_edit, w, cx);
                                                    });
                                                    this.update(cx, |panel, cx| {
                                                        panel.state.update(cx, |s, _cx| {
                                                            s.start_edit_tag(
                                                                tag_id,
                                                                &tag_name_edit,
                                                                &tag_color_hex,
                                                            );
                                                        });
                                                        panel.last_edit_tag_id = tag_id;
                                                        cx.notify();
                                                    });
                                                }
                                            },
                                        ))
                                        .child(small_btn(
                                            "\u{e696}",
                                            text_3,
                                            true,
                                            btn_hover,
                                            danger_hover_bg,
                                            {
                                                let this = this.clone();
                                                move |_w, cx| {
                                                    this.update(cx, |panel, cx| {
                                                        panel.delete_tag(tag_id, cx);
                                                        cx.notify();
                                                    })
                                                }
                                            },
                                        ))
                                }))
                        }),
                    )),
            )
            .when(rows_is_empty, |el| {
                el.child(
                    div()
                        .py(px(8.))
                        .px(px(6.))
                        .text_size(px(11.))
                        .text_color(text_3)
                        .child(I18nKey::TagFilterNoTags.text()),
                )
            })
    }
}

// --- TagEditPanel render helper ---

pub fn render_edit_panel(
    name_input: &Entity<InputState>,
    color: &str,
    theme: ClippiTheme,
    scale: f32,
    on_cancel: impl Fn(&mut Window, &mut App) + 'static,
    on_color: impl Fn(&mut Window, &mut App, String) + 'static,
    on_save: impl Fn(&mut Window, &mut App, String, String) + 'static,
) -> impl IntoElement {
    let is_dark = theme.bg == rgb(0x191a1b);
    let accent = theme.accent;
    let text_1 = theme.text_1;
    let text_2 = theme.text_2;
    let surface = theme.panel_surface;
    let input_bg = theme.panel_input_bg;
    let sep_line = theme.panel_sep_line;
    let btn_hover = theme.btn_hover;
    let panel_border = if is_dark {
        rgba(0xffffff20)
    } else {
        rgba(0x00000012)
    };
    let accent_hover = if is_dark {
        rgba(0x7ecba3cc)
    } else {
        rgba(0x6ab890cc)
    };
    let current_color = parse_hex_to_rgba(color);
    let presets: Vec<Rgba> = tag_preset_colors()
        .iter()
        .map(|(_, hex)| parse_hex_to_rgba(hex))
        .collect();
    let color_rows: Vec<Vec<Rgba>> = presets.chunks(6).map(|r| r.to_vec()).collect();
    let on_cancel = Rc::new(on_cancel);
    let on_color = Rc::new(on_color);
    let on_save = Rc::new(on_save);

    div()
        .w(px(260. * scale))
        .bg(surface)
        .border_color(panel_border)
        .border(px(1.))
        .rounded(px(8.))
        .shadow_lg()
        .occlude()
        .p(px(10. * scale))
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_1)
                .child(I18nKey::TagEditTitle.text()),
        )
        .child(div().h(px(1.)).w_full().bg(sep_line))
        .child(
            div()
                .text_size(px(11.))
                .text_color(text_2)
                .child(I18nKey::TagNameLabel.text()),
        )
        .child(
            div()
                .h(px(26.))
                .rounded(px(5.))
                .bg(input_bg)
                .px(px(7.))
                .flex()
                .items_center()
                .child(
                    Input::new(name_input)
                        .appearance(false)
                        .bordered(false)
                        .focus_bordered(false)
                        .w_full()
                        .h(px(20.))
                        .px(px(0.))
                        .text_size(px(11.))
                        .text_color(text_1),
                ),
        )
        .child(div().h(px(1.)).w_full().bg(sep_line))
        .child(
            div()
                .text_size(px(11.))
                .text_color(text_2)
                .child(I18nKey::TagColor.text()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .children(color_rows.into_iter().map(|row| {
                    let on_color = on_color.clone();
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(4.))
                        .children(row.into_iter().map(move |c| {
                            let on_color = on_color.clone();
                            let r = (c.r * 255.0) as u8;
                            let g = (c.g * 255.0) as u8;
                            let b = (c.b * 255.0) as u8;
                            let hex = format!("#{:02X}{:02X}{:02X}", r, g, b);
                            div()
                                .w(px(36.))
                                .h(px(24.))
                                .rounded(px(4.))
                                .bg(c)
                                .border(if c == current_color { px(2.) } else { px(0.) })
                                .border_color(text_1)
                                .cursor(CursorStyle::PointingHand)
                                .occlude()
                                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                    on_color(window, cx, hex.clone());
                                })
                        }))
                })),
        )
        .child(div().h(px(1.)).w_full().bg(sep_line))
        .child({
            let r = (current_color.r * 255.0) as u8;
            let g = (current_color.g * 255.0) as u8;
            let b = (current_color.b * 255.0) as u8;
            let hex_save = format!("#{:02X}{:02X}{:02X}", r, g, b);
            div()
                .h(px(26.))
                .flex()
                .flex_row()
                .justify_end()
                .gap(px(6.))
                .child(
                    div()
                        .w(px(52.))
                        .h(px(26.))
                        .rounded(px(5.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor(CursorStyle::PointingHand)
                        .hover(|style| style.bg(btn_hover))
                        .text_size(px(11.))
                        .text_color(text_2)
                        .child(I18nKey::BtnCancel.text())
                        .occlude()
                        .on_mouse_down(MouseButton::Left, {
                            let on_cancel = on_cancel.clone();
                            move |_ev, w, cx| on_cancel(w, cx)
                        }),
                )
                .child(
                    div()
                        .w(px(52.))
                        .h(px(26.))
                        .rounded(px(5.))
                        .bg(accent)
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor(CursorStyle::PointingHand)
                        .hover(move |style| style.bg(accent_hover))
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0xffffff))
                        .child(I18nKey::BackendSave.text())
                        .occlude()
                        .on_mouse_down(MouseButton::Left, {
                            let on_save = on_save.clone();
                            let name_input = name_input.clone();
                            let hex = hex_save.clone();
                            move |_ev, w, cx| {
                                let name = name_input.read(cx).value().to_string();
                                on_save(w, cx, name, hex.clone())
                            }
                        }),
                )
        })
}

// --- Helpers ---

fn icon_btn(
    icon: &'static str,
    color: Rgba,
    hover_bg: Rgba,
    tooltip: Option<&'static str>,
    handler: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(icon)
        .w(px(22.))
        .h(px(22.))
        .rounded(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::PointingHand)
        .hover(move |style| style.bg(hover_bg))
        .when_some(tooltip, |button, tip| {
            button.tooltip(move |window, cx| {
                Tooltip::element(move |_window, _cx| div().text_size(px(10.)).child(tip))
                    .build(window, cx)
            })
        })
        .child(
            div()
                .font_family("iconfont")
                .text_size(px(12.))
                .text_color(color)
                .child(icon),
        )
        .on_mouse_down(MouseButton::Left, move |_e, w, cx| handler(w, cx))
}

fn small_btn(
    icon: &'static str,
    color: Rgba,
    danger: bool,
    hover_bg: Rgba,
    danger_hover_bg: Rgba,
    handler: impl Fn(&mut Window, &mut App) + 'static,
) -> Div {
    div()
        .w(px(18.))
        .h(px(18.))
        .rounded(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::PointingHand)
        .hover(move |style| {
            if danger {
                style.bg(danger_hover_bg)
            } else {
                style.bg(hover_bg)
            }
        })
        .child(
            div()
                .font_family("iconfont")
                .text_size(px(12.))
                .text_color(color)
                .child(icon),
        )
        .occlude()
        .on_mouse_down(MouseButton::Left, move |_e, w, cx| handler(w, cx))
}

fn parse_hex_to_rgba(hex: &str) -> Rgba {
    let s = hex.strip_prefix('#').unwrap_or(hex);
    if s.len() == 6 {
        if let Ok(val) = u32::from_str_radix(s, 16) {
            return rgb(val);
        }
    }
    rgb(0x888888)
}
