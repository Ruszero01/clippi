//! GPUI edit panel for clipboard text and rich-text items.

use base64::Engine;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::text::{TextView, TextViewStyle};
use percent_encoding::percent_decode_str;
use std::borrow::Cow;

use crate::core::types::{ClipboardItem, ContentType, RichData};
use crate::state::app::AppState;

use super::theme::ClippiTheme;

const TYPE_OPTIONS: [(&str, &str); 8] = [
    ("plain_text", "Text"),
    ("markdown", "Markdown"),
    ("html", "HTML"),
    ("link", "URL"),
    ("path", "Path"),
    ("color", "Color"),
    ("email", "Email"),
    ("phone", "Phone"),
];

pub struct EditPanel {
    state: Entity<AppState>,
    content_input: Entity<InputState>,
    selected_type: String,
    type_menu_open: bool,
    last_item_id: i64,
    preview_generation: u64,
    theme: ClippiTheme,
    _subscriptions: Vec<Subscription>,
}

pub enum EditPanelEvent {
    Back,
    Saved,
}

impl EventEmitter<EditPanelEvent> for EditPanel {}

impl EditPanel {
    pub fn new(
        state: Entity<AppState>,
        theme: ClippiTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let content_input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("Content...")
        });
        let content_for_sub = content_input.clone();
        let _subscriptions =
            vec![
                cx.subscribe(&content_input, move |this, _, ev: &InputEvent, cx| {
                    if matches!(ev, InputEvent::Change) {
                        this.preview_generation = this.preview_generation.wrapping_add(1);
                        let _ = content_for_sub.read(cx).value();
                        cx.notify();
                    }
                }),
            ];

        Self {
            state,
            content_input,
            selected_type: "plain_text".into(),
            type_menu_open: false,
            last_item_id: -1,
            preview_generation: 0,
            theme,
            _subscriptions,
        }
    }

    pub fn set_theme(&mut self, theme: ClippiTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    fn sync_from_item(
        &mut self,
        item: &ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.last_item_id = item.id;
        let item_type = editor_type_from_item(item);
        self.selected_type = item_type.to_string();
        self.type_menu_open = false;
        self.preview_generation = self.preview_generation.wrapping_add(1);

        let content = SharedString::from(item.full_text.clone());
        self.content_input.update(cx, |input, cx| {
            input.set_value(content.clone(), window, cx);
            input.focus_handle(cx).focus(window);
        });
    }

    fn apply_content_transform(
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut App,
        transform: impl FnOnce(&str) -> String,
    ) {
        input.update(cx, |input, cx| {
            let current = input.value().to_string();
            input.set_value(SharedString::from(transform(&current)), window, cx);
        });
    }

    fn save(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let item_id = self.last_item_id;
        if item_id < 0 {
            return;
        }
        let text = self.content_input.read(cx).value().to_string();
        let editor_type = self.selected_type.clone();
        let saved = self.state.update(cx, |state, _cx| {
            state.save_edited_item(item_id, &text, &editor_type)
        });
        if saved {
            cx.emit(EditPanelEvent::Saved);
        }
    }
}

impl Render for EditPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let item = self.state.read(cx).editing_item.clone();
        if let Some(ref item) = item {
            if item.id != self.last_item_id {
                self.sync_from_item(item, window, cx);
            }
        }

        let this = cx.entity();
        let theme = self.theme.clone();
        let surface = theme.surface;
        let bg = theme.bg;
        let divider = theme.divider;
        let text_1 = theme.text_1;
        let text_2 = theme.text_2;
        let accent = theme.accent;
        let hover_bg = if bg == rgb(0x191a1b) {
            rgba(0xffffff10)
        } else {
            rgba(0x0000000a)
        };
        let is_rich_editor = matches!(self.selected_type.as_str(), "markdown" | "html");
        let selected_label = type_label(&self.selected_type);
        let content_input = self.content_input.clone();
        let content_text = self.content_input.read(cx).value().to_string();
        let selected_type = self.selected_type.clone();
        let preview_generation = self.preview_generation;

        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(bg)
            .p(px(8.))
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .h(px(36.))
                    .child(icon_button("\u{e62b}", text_2, accent, hover_bg, {
                        let this = this.clone();
                        move |_window, cx| {
                            this.update(cx, |_panel, cx| {
                                cx.emit(EditPanelEvent::Back);
                            });
                        }
                    }))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child("Edit"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(24.))
                    .gap(px(4.))
                    .child(
                        div()
                            .h(px(22.))
                            .px(px(8.))
                            .rounded(px(4.))
                            .border(px(1.))
                            .border_color(if self.type_menu_open { accent } else { divider })
                            .bg(surface)
                            .flex()
                            .items_center()
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, {
                                let this = this.clone();
                                move |_ev, _window, cx| {
                                    this.update(cx, |panel, cx| {
                                        panel.type_menu_open = !panel.type_menu_open;
                                        cx.notify();
                                    });
                                }
                            })
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(accent)
                                    .child(selected_label),
                            ),
                    )
                    .child(div().flex_1())
                    .child(icon_button("\u{e6da}", text_2, accent, hover_bg, {
                        let input = content_input.clone();
                        move |window, cx| {
                            EditPanel::apply_content_transform(&input, window, cx, |text| {
                                percent_decode_str(text)
                                    .decode_utf8()
                                    .unwrap_or(Cow::Borrowed(text))
                                    .into_owned()
                            });
                        }
                    }))
                    .child(icon_button("\u{e66e}", text_2, accent, hover_bg, {
                        let input = content_input.clone();
                        move |window, cx| {
                            EditPanel::apply_content_transform(&input, window, cx, decode_base64);
                        }
                    }))
                    .child(icon_button("\u{e819}", text_2, accent, hover_bg, {
                        let input = content_input.clone();
                        move |window, cx| {
                            EditPanel::apply_content_transform(&input, window, cx, json_format);
                        }
                    }))
                    .child(icon_button("\u{e6db}", text_2, accent, hover_bg, {
                        let input = content_input.clone();
                        move |window, cx| {
                            EditPanel::apply_content_transform(&input, window, cx, trim_text);
                        }
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .when(!is_rich_editor, |area| {
                        area.child(editor_box(&content_input, surface, divider, px(0.), true))
                    })
                    .when(is_rich_editor, |area| {
                        area.child(editor_box(&content_input, surface, divider, px(0.), true))
                            .child(
                                div()
                                    .h(px(150.))
                                    .rounded(px(8.))
                                    .border(px(1.))
                                    .border_color(divider)
                                    .bg(surface)
                                    .overflow_hidden()
                                    .child(
                                        div().size_full().overflow_y_scrollbar().child(
                                            div()
                                                .pt(px(10.))
                                                .pb(px(10.))
                                                .pl(px(10.))
                                                .pr(px(18.))
                                                .child(render_rich_preview(
                                                    &selected_type,
                                                    &content_text,
                                                    self.last_item_id,
                                                    preview_generation,
                                                    window,
                                                    cx,
                                                )),
                                        ),
                                    ),
                            )
                    }),
            )
            .child(
                div()
                    .h(px(32.))
                    .flex()
                    .flex_row()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(text_button("Cancel", text_2, divider, rgba(0x00000000), {
                        let this = this.clone();
                        move |_window, cx| {
                            this.update(cx, |_panel, cx| {
                                cx.emit(EditPanelEvent::Back);
                            });
                        }
                    }))
                    .child(text_button("Save", rgb(0xffffff), accent, accent, {
                        let this = this.clone();
                        move |window, cx| {
                            this.update(cx, |panel, cx| panel.save(window, cx));
                        }
                    })),
            )
            .when(self.type_menu_open, |panel| {
                panel
                    .child(
                        div()
                            .absolute()
                            .size_full()
                            .on_mouse_down(MouseButton::Left, {
                                let this = this.clone();
                                move |_ev, _window, cx| {
                                    this.update(cx, |panel, cx| {
                                        panel.type_menu_open = false;
                                        cx.notify();
                                    });
                                }
                            }),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(8.))
                            .top(px(78.))
                            .w(px(112.))
                            .rounded(px(6.))
                            .border(px(1.))
                            .border_color(divider)
                            .bg(surface)
                            .shadow_lg()
                            .p(px(4.))
                            .occlude()
                            .children(TYPE_OPTIONS.into_iter().map(|(key, label)| {
                                let this = this.clone();
                                let key = key.to_string();
                                let active = self.selected_type == key;
                                div()
                                    .h(px(26.))
                                    .rounded(px(4.))
                                    .px(px(8.))
                                    .flex()
                                    .items_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(move |style| style.bg(hover_bg))
                                    .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                        let key = key.clone();
                                        this.update(cx, |panel, cx| {
                                            panel.selected_type = key;
                                            panel.type_menu_open = false;
                                            panel.preview_generation =
                                                panel.preview_generation.wrapping_add(1);
                                            panel.content_input.update(cx, |input, cx| {
                                                input.focus_handle(cx).focus(window);
                                            });
                                            cx.notify();
                                        });
                                    })
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(if active { accent } else { text_1 })
                                            .child(label),
                                    )
                            })),
                    )
            })
    }
}

fn editor_box(
    input: &Entity<InputState>,
    surface: Rgba,
    divider: Rgba,
    height: Pixels,
    fill: bool,
) -> Div {
    let box_el = div()
        .rounded(px(8.))
        .border(px(1.))
        .border_color(divider)
        .bg(surface)
        .pt(px(8.))
        .pb(px(8.))
        .pl(px(8.))
        .pr(px(0.))
        .child(
            Input::new(input)
                .appearance(false)
                .bordered(false)
                .focus_bordered(false)
                .w_full()
                .h_full()
                .text_size(px(12.)),
        );
    if fill {
        box_el.flex_1()
    } else {
        box_el.h(height).min_h(px(220.))
    }
}

fn render_rich_preview(
    selected_type: &str,
    text: &str,
    item_id: i64,
    generation: u64,
    window: &mut Window,
    cx: &mut Context<EditPanel>,
) -> AnyElement {
    let preview_key = (item_id.max(0) as u64)
        .wrapping_mul(1_000_003)
        .wrapping_add(generation);
    let style = TextViewStyle::default()
        .paragraph_gap(rems(0.25))
        .heading_font_size(|level, base| if level <= 2 { base * 1.08 } else { base });
    if selected_type == "html" {
        TextView::html(
            ("edit-html-preview", preview_key),
            text.to_string(),
            window,
            cx,
        )
        .style(style)
        .selectable(false)
        .into_any_element()
    } else {
        TextView::markdown(
            ("edit-markdown-preview", preview_key),
            text.to_string(),
            window,
            cx,
        )
        .style(style)
        .selectable(false)
        .into_any_element()
    }
}

fn icon_button(
    icon: &'static str,
    normal: Rgba,
    hover: Rgba,
    hover_bg: Rgba,
    handler: impl Fn(&mut Window, &mut App) + 'static,
) -> Div {
    div()
        .w(px(22.))
        .h(px(22.))
        .rounded(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::PointingHand)
        .hover(move |style| style.bg(hover_bg))
        .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            handler(window, cx)
        })
        .child(
            div()
                .font_family("iconfont")
                .text_size(px(14.))
                .text_color(normal)
                .hover(move |style| style.text_color(hover))
                .child(icon),
        )
}

fn text_button(
    label: &'static str,
    text_color: Rgba,
    border_color: Rgba,
    bg: Rgba,
    handler: impl Fn(&mut Window, &mut App) + 'static,
) -> Div {
    div()
        .w(px(60.))
        .h(px(28.))
        .rounded(px(6.))
        .border(px(1.))
        .border_color(border_color)
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::PointingHand)
        .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            handler(window, cx)
        })
        .child(
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_color)
                .child(label),
        )
}

fn editor_type_from_item(item: &ClipboardItem) -> &'static str {
    match item.meta_type.as_str() {
        "markdown" => "markdown",
        "html" => "html",
        "email" => "email",
        "phone" => "phone",
        _ => match item.content_type {
            ContentType::RichText => {
                let rich = RichData::from_json(&item.rich_data);
                if rich
                    .html
                    .as_deref()
                    .is_some_and(|html| !html.trim().is_empty())
                {
                    "html"
                } else {
                    "plain_text"
                }
            }
            ContentType::Link => "link",
            ContentType::Path => "path",
            ContentType::Color => "color",
            _ => "plain_text",
        },
    }
}

fn type_label(key: &str) -> &'static str {
    TYPE_OPTIONS
        .iter()
        .find_map(|(option_key, label)| (*option_key == key).then_some(*label))
        .unwrap_or("Text")
}

fn json_format(text: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_else(|_| text.to_string()),
        Err(_) => text.to_string(),
    }
}

fn trim_text(text: &str) -> String {
    let text = text
        .replace("\r\n", "\n")
        .replace(['\r', '\u{2028}', '\u{2029}'], "\n");
    let mut result = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut prev_ws = false;
        for ch in trimmed.chars() {
            if ch.is_whitespace() {
                if !prev_ws {
                    result.push(' ');
                    prev_ws = true;
                }
            } else {
                result.push(ch);
                prev_ws = false;
            }
        }
        result.push('\n');
    }
    if result.ends_with('\n') {
        result.pop();
    }
    result
}

fn decode_base64(text: &str) -> String {
    let encoded = if let Some(pos) = text.find(";base64,") {
        &text[pos + 8..]
    } else {
        text
    };
    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) {
        return String::from_utf8_lossy(&bytes).into_owned();
    }
    match base64::engine::general_purpose::URL_SAFE.decode(encoded) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => text.to_string(),
    }
}
