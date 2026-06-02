//! Tag filter panel — floating panel for filtering clipboard items by tag.
//!
//! Matches the original Slint TagFilterPanel.slint design:
//! - Title row with AND/OR mode toggle, clear, close buttons
//! - Tag creation input with "+" button (real gpui_component Input)
//! - 2-column tag grid (140px each, 30px height)
//! - Edit/delete buttons per tag row
//! - TagEditPanel overlay for editing name/color

use std::rc::Rc;

use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::input::{Input, InputEvent, InputState};

use crate::core::types::{TagInfo, tag_preset_colors};
use crate::state::app::AppState;

use super::clipboard_list::ClipboardListView;
use super::search_bar::SearchBar;

pub struct TagFilterPanel {
    state: Entity<AppState>,
    list_view: Entity<ClipboardListView>,
    search_bar: Entity<SearchBar>,
    create_input: Entity<InputState>,
    edit_name_input: Entity<InputState>,
    last_edit_tag_id: i64,
    _subscriptions: Vec<Subscription>,
}

impl TagFilterPanel {
    pub fn new(
        state: Entity<AppState>,
        list_view: Entity<ClipboardListView>,
        search_bar: Entity<SearchBar>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let create_input = cx.new(|cx| InputState::new(window, cx).placeholder("Tag name..."));
        let edit_name_input = cx.new(|cx| InputState::new(window, cx));

        let state_enter = state.clone();
        let list_enter = list_view.clone();
        let search_enter = search_bar.clone();
        let input_enter = create_input.clone();
        let this_enter = cx.entity().clone();

        let _subscriptions = vec![
            cx.subscribe(&create_input, move |_this, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&create_input, move |_this, _, ev: &InputEvent, cx| {
                if !matches!(ev, InputEvent::PressEnter { .. }) {
                    return;
                }
                let name = input_enter.read(cx).value().to_string();
                if name.trim().is_empty() {
                    return;
                }
                state_enter.update(cx, |s, _cx| s.create_tag(name.trim()));
                let items = state_enter.read(cx).items.clone();
                list_enter.update(cx, |l, cx| l.set_items(items, cx));
                search_enter.update(cx, |_b, cx| cx.notify());
                this_enter.update(cx, |_this, cx| cx.notify());
            }),
        ];

        Self {
            state, list_view, search_bar,
            create_input, edit_name_input,
            last_edit_tag_id: -1,
            _subscriptions,
        }
    }

    fn refresh_list(&self, cx: &mut App) {
        let items = self.state.read(cx).items.clone();
        self.list_view.update(cx, |l, cx| l.set_items(items, cx));
        self.search_bar.update(cx, |_b, cx| cx.notify());
    }

    fn toggle_filter(&self, tag_id: i64, cx: &mut App) {
        self.state.update(cx, |s, _cx| { s.toggle_tag_filter(tag_id); });
        self.refresh_list(cx);
    }

    fn clear_filters(&self, cx: &mut App) {
        self.state.update(cx, |s, _cx| { s.clear_tag_filters(); });
        self.refresh_list(cx);
    }

    fn toggle_mode(&self, cx: &mut App) {
        self.state.update(cx, |s, _cx| { s.toggle_tag_match_mode(); });
        self.refresh_list(cx);
    }

    fn close(&self, cx: &mut App) {
        self.search_bar.update(cx, |bar, cx| bar.close_tag_panel(cx));
    }

    fn create_tag(&self, name: &str, cx: &mut App) {
        self.state.update(cx, |s, _cx| s.create_tag(name));
        self.refresh_list(cx);
    }

    fn delete_tag(&self, tag_id: i64, cx: &mut App) {
        self.state.update(cx, |s, _cx| s.delete_tag(tag_id));
        self.refresh_list(cx);
    }
}

impl Render for TagFilterPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app_state = self.state.read(cx);
        let tags: Vec<(TagInfo, bool)> = app_state.tags.iter().map(|t| {
            (t.clone(), app_state.filters.tag_ids.contains(&t.id))
        }).collect();
        let tag_match_all = app_state.filters.is_tag_match_all();
        let has_tag_filter = !app_state.filters.tag_ids.is_empty();
        let editing_tag_id = app_state.editing_tag_id;
        let _editing_tag_name = app_state.editing_tag_name.clone();
        let editing_tag_color = app_state.editing_tag_color.clone();
        let _ = app_state;

        // Track editing tag id changes
        if editing_tag_id != self.last_edit_tag_id {
            self.last_edit_tag_id = editing_tag_id;
        }

        let accent = rgb(0x7ecba3);
        let text_1 = rgb(0xeaebec);
        let text_2 = rgb(0x919496);
        let text_3 = rgb(0x5f6264);
        let surface = rgb(0x25262a);
        let input_bg = rgb(0x1f2023);
        let sep_line = rgba(0xffffff14);
        let btn_hover = rgb(0x3e3f42);

        let rows: Vec<Vec<(TagInfo, bool)>> = tags.chunks(2).map(|r| r.to_vec()).collect();
        let rows_is_empty = rows.is_empty();
        let this_entity = cx.entity().clone();

        div()
            .flex().flex_col()
            .w(px(304.)).max_h(px(360.))
            .bg(surface)
            .border_color(rgba(0xffffff14)).border(px(1.))
            .rounded(px(8.)).shadow_lg()
            .p(px(8.)).gap(px(4.))
            // ── Title row ──
            .child({
                let this = this_entity.clone();
                div().h(px(24.)).flex().flex_row().items_center()
                    .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(text_1).child("Tag filter"))
                    .child(div().flex_1())
                    .child(icon_btn("\u{e61e}", if tag_match_all { accent } else { text_2 }, {
                        let this = this.clone();
                        move |_, cx| this.update(cx, |panel, cx| panel.toggle_mode(cx))
                    }))
                    .when(has_tag_filter, |el| el.child(icon_btn("\u{e607}", text_2, {
                        let this = this.clone();
                        move |_, cx| this.update(cx, |panel, cx| panel.clear_filters(cx))
                    })))
                    .child(icon_btn("\u{e7b7}", text_2, {
                        let this = this.clone();
                        move |_, cx| this.update(cx, |panel, cx| panel.close(cx))
                    }))
            })
            .child(div().h(px(1.)).w_full().bg(sep_line))
            // ── Create tag row (real Input) ──
            .child({
                let this = this_entity.clone();
                div().h(px(26.)).flex().flex_row().gap(px(4.))
                    .child(
                        div().flex_1().h(px(26.)).rounded(px(5.)).bg(input_bg)
                            .px(px(7.)).flex().items_center()
                            .child(
                                Input::new(&self.create_input)
                                    .appearance(false).bordered(false).focus_bordered(false)
                                    .w_full().h(px(20.))
                                    .text_size(px(11.)).text_color(text_1),
                            ),
                    )
                    .child({
                        let this = this.clone();
                        div().w(px(26.)).h(px(26.)).rounded(px(5.)).bg(btn_hover)
                            .flex().items_center().justify_center()
                            .cursor(CursorStyle::PointingHand).hover(|style| style.bg(accent))
                            .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(accent)
                                .hover(|style| style.text_color(rgb(0xffffff))).child("+"))
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                this.update(cx, |panel, cx| {
                                    let name = panel.create_input.read(cx).value().to_string();
                                    if !name.trim().is_empty() {
                                        panel.create_tag(name.trim(), cx);
                                        cx.notify();
                                    }
                                });
                            })
                    })
            })
            .child(div().h(px(1.)).w_full().bg(sep_line))
            // ── Tag list ──
            .child(
                div().flex().flex_col().gap(px(4.)).max_h(px(200.)).overflow_hidden()
                    .children(rows.into_iter().map(|row| {
                        let this = this_entity.clone();
                        div().flex().flex_row().gap(px(4.))
                            .children(row.into_iter().map(|(tag, checked)| {
                                let tag_id = tag.id;
                                let tag_color = parse_hex_to_rgba(&tag.color);
                                let tag_name = tag.name.clone();
                                let tag_name_edit = tag.name.clone();
                                let tag_color_hex = tag.color.clone();
                                let this = this.clone();
                                div().w(px(140.)).h(px(30.)).rounded(px(5.))
                                    .bg(if checked { rgba(0x7ecba318) } else { rgba(0x00000000) })
                                    .flex().flex_row().items_center().gap(px(5.)).px(px(6.))
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(move |style| { if checked { style.bg(rgba(0x7ecba318)) } else { style.bg(btn_hover) } })
                                    .on_mouse_down(MouseButton::Left, {
                                        let this = this.clone();
                                        move |_ev, _window, cx| {
                                            this.update(cx, |panel, cx| panel.toggle_filter(tag_id, cx));
                                        }
                                    })
                                    .child(div().w(px(8.)).h(px(8.)).rounded(px(4.)).bg(tag_color))
                                    .child(div().flex_1().text_size(px(11.))
                                        .font_weight(if checked { FontWeight::SEMIBOLD } else { FontWeight::default() })
                                        .text_color(if checked { accent } else { text_1 })
                                        .truncate().child(tag_name))
                                    .child(small_btn("\u{e648}", text_3, false, {
                                        let this = this.clone();
                                        move |w, cx| {
                                            // Set edit name input value before updating state
                                            let input = this.read(cx).edit_name_input.clone();
                                            input.update(cx, |input, cx| {
                                                input.set_value(&tag_name_edit, w, cx);
                                            });
                                            this.update(cx, |panel, cx| {
                                                panel.state.update(cx, |s, _cx| {
                                                    s.start_edit_tag(tag_id, &tag_name_edit, &tag_color_hex);
                                                });
                                                panel.last_edit_tag_id = tag_id;
                                                cx.notify();
                                            });
                                        }
                                    }))
                                    .child(small_btn("\u{e8b6}", text_3, true, {
                                        let this = this.clone();
                                        move |_w, cx| this.update(cx, |panel, cx| {
                                            panel.delete_tag(tag_id, cx);
                                            cx.notify();
                                        })
                                    }))
                            }))
                    }))
            )
            .when(rows_is_empty, |el| {
                el.child(div().py(px(8.)).px(px(6.)).text_size(px(11.)).text_color(text_3).child("No tags. Create above"))
            })
            // ── TagEditPanel overlay ──
            .when(editing_tag_id >= 0, move |el| {
                let this = this_entity.clone();
                let this_for_backdrop = this_entity.clone();
                el.child(
                    div().absolute().size_full().rounded(px(8.))
                        .child(
                            // Backdrop — click outside edit panel to cancel
                            div().absolute().size_full().bg(rgba(0x00000060))
                                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                    this_for_backdrop.update(cx, |panel, cx| {
                                        panel.state.update(cx, |s, _cx| s.cancel_edit_tag());
                                        cx.notify();
                                    });
                                }),
                        )
                        .child(
                            // Edit panel wrapper — on top of backdrop
                            div().absolute().size_full().flex().items_center().justify_center()
                                .child(render_edit_panel(
                            &self.edit_name_input, &editing_tag_color,
                            {
                                let this = this.clone();
                                move |_w, cx| this.update(cx, |panel, cx| {
                                    panel.state.update(cx, |s, _cx| s.cancel_edit_tag());
                                    cx.notify();
                                })
                            },
                            {
                                let this = this.clone();
                                move |_w, cx, color| this.update(cx, |panel, cx| {
                                    panel.state.update(cx, |s, _cx| s.set_edit_tag_color(&color));
                                    cx.notify();
                                })
                            },
                            {
                                let this = this.clone();
                                move |_w, cx, name, color| this.update(cx, |panel, cx| {
                                    panel.state.update(cx, |s, _cx| s.update_tag(editing_tag_id, &name, &color));
                                    panel.refresh_list(cx);
                                    cx.notify();
                                })
                            },
                        )),
                    )
                )
            })
    }
}

// ── TagEditPanel render helper ──

fn render_edit_panel(
    name_input: &Entity<InputState>,
    color: &str,
    on_cancel: impl Fn(&mut Window, &mut App) + 'static,
    on_color: impl Fn(&mut Window, &mut App, String) + 'static,
    on_save: impl Fn(&mut Window, &mut App, String, String) + 'static,
) -> impl IntoElement {
    let accent = rgb(0x7ecba3);
    let text_1 = rgb(0xeaebec);
    let text_2 = rgb(0x919496);
    let surface = rgb(0x2c2d2e);
    let input_bg = rgb(0x1f2023);
    let sep_line = rgba(0xffffff14);
    let btn_hover = rgb(0x3e3f42);
    let current_color = parse_hex_to_rgba(color);
    let presets: Vec<Rgba> = tag_preset_colors().iter().map(|(_, hex)| parse_hex_to_rgba(hex)).collect();
    let color_rows: Vec<Vec<Rgba>> = presets.chunks(6).map(|r| r.to_vec()).collect();
    let on_cancel = Rc::new(on_cancel);
    let on_color = Rc::new(on_color);
    let on_save = Rc::new(on_save);

    div().w(px(260.))
        .bg(surface).border_color(rgba(0xffffff20)).border(px(1.))
        .rounded(px(8.)).shadow_lg()
        .occlude()
        .p(px(10.)).flex().flex_col().gap(px(6.))
        .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(text_1).child("Edit tag"))
        .child(div().h(px(1.)).w_full().bg(sep_line))
        .child(div().text_size(px(11.)).text_color(text_2).child("Name"))
        .child(div().h(px(26.)).rounded(px(5.)).bg(input_bg).px(px(7.))
            .flex().items_center()
            .child(
                Input::new(name_input)
                    .appearance(false).bordered(false).focus_bordered(false)
                    .w_full().h(px(20.)).px(px(0.))
                    .text_size(px(11.)).text_color(text_1),
            ))
        .child(div().h(px(1.)).w_full().bg(sep_line))
        .child(div().text_size(px(11.)).text_color(text_2).child("Color"))
        .child(div().flex().flex_col().gap(px(4.))
            .children(color_rows.into_iter().map(|row| {
                let on_color = on_color.clone();
                div().flex().flex_row().gap(px(4.))
                    .children(row.into_iter().map(move |c| {
                        let on_color = on_color.clone();
                        let r = (c.r * 255.0) as u8;
                        let g = (c.g * 255.0) as u8;
                        let b = (c.b * 255.0) as u8;
                        let hex = format!("#{:02X}{:02X}{:02X}", r, g, b);
                        div().w(px(36.)).h(px(24.)).rounded(px(4.)).bg(c)
                            .border(if c == current_color { px(2.) } else { px(0.) }).border_color(text_1)
                            .cursor(CursorStyle::PointingHand)
                            .occlude()
                            .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                on_color(window, cx, hex.clone());
                            })
                    }))
            })))
        .child(div().h(px(1.)).w_full().bg(sep_line))
        .child({
            let r = (current_color.r * 255.0) as u8;
            let g = (current_color.g * 255.0) as u8;
            let b = (current_color.b * 255.0) as u8;
            let hex_save = format!("#{:02X}{:02X}{:02X}", r, g, b);
            div().h(px(26.)).flex().flex_row().justify_end().gap(px(6.))
                .child(div().w(px(52.)).h(px(26.)).rounded(px(5.))
                    .flex().items_center().justify_center()
                    .cursor(CursorStyle::PointingHand).hover(|style| style.bg(btn_hover))
                    .text_size(px(11.)).text_color(text_2).child("Cancel")
                    .occlude()
                    .on_mouse_down(MouseButton::Left, {
                        let on_cancel = on_cancel.clone();
                        move |_ev, w, cx| on_cancel(w, cx)
                    }))
                .child(div().w(px(52.)).h(px(26.)).rounded(px(5.)).bg(accent)
                    .flex().items_center().justify_center()
                    .cursor(CursorStyle::PointingHand).hover(|style| style.bg(rgba(0x7ecba3cc)))
                    .text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(0xffffff)).child("Save")
                    .occlude()
                    .on_mouse_down(MouseButton::Left, {
                        let on_save = on_save.clone();
                        let name_input = name_input.clone();
                        let hex = hex_save.clone();
                        move |_ev, w, cx| {
                            let name = name_input.read(cx).value().to_string();
                            on_save(w, cx, name, hex.clone())
                        }
                    }))
        })
}

// ── Helpers ──

fn icon_btn(icon: &'static str, color: Rgba, handler: impl Fn(&mut Window, &mut App) + 'static) -> Div {
    div().w(px(22.)).h(px(22.)).rounded(px(5.))
        .flex().items_center().justify_center()
        .cursor(CursorStyle::PointingHand).hover(|style| style.bg(rgb(0x3e3f42)))
        .child(div().font_family("iconfont").text_size(px(12.)).text_color(color).child(icon))
        .on_mouse_down(MouseButton::Left, move |_e, w, cx| handler(w, cx))
}

fn small_btn(icon: &'static str, color: Rgba, danger: bool, handler: impl Fn(&mut Window, &mut App) + 'static) -> Div {
    div().w(px(18.)).h(px(18.)).rounded(px(4.))
        .flex().items_center().justify_center().cursor(CursorStyle::PointingHand)
        .hover(move |style| if danger { style.bg(rgba(0xff5f5720)) } else { style.bg(rgb(0x3e3f42)) })
        .child(div().font_family("iconfont").text_size(px(12.)).text_color(color).child(icon))
        .occlude()
        .on_mouse_down(MouseButton::Left, move |_e, w, cx| handler(w, cx))
}

fn parse_hex_to_rgba(hex: &str) -> Rgba {
    let s = hex.strip_prefix('#').unwrap_or(hex);
    if s.len() == 6 {
        if let Ok(val) = u32::from_str_radix(s, 16) { return rgb(val); }
    }
    rgb(0x888888)
}
