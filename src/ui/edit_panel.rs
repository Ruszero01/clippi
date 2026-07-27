//! GPUI edit panel for clipboard text and rich-text items.

use base64::Engine;
use gpui::prelude::*;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::text::{TextView, TextViewStyle};
use gpui_component::tooltip::Tooltip;
use percent_encoding::percent_decode_str;
use std::borrow::Cow;

use crate::core::i18n_keys::I18nKey;
use crate::core::types::{ClipboardItem, RichData};
use crate::state::app::AppState;

use super::rich_preview;
use super::theme::ClippiTheme;

const TYPE_OPTIONS: [(&str, I18nKey); 9] = [
    ("plain_text", I18nKey::EditTypeText),
    ("markdown", I18nKey::EditTypeMarkdown),
    ("html", I18nKey::EditTypeHtml),
    ("link", I18nKey::EditTypeUrl),
    ("path", I18nKey::EditTypePath),
    ("color", I18nKey::EditTypeColor),
    ("email", I18nKey::EditTypeEmail),
    ("phone", I18nKey::EditTypePhone),
    ("secret", I18nKey::EditTypeSecret),
];

pub struct EditPanel {
    state: Entity<AppState>,
    content_input: Entity<InputState>,
    selected_type: String,
    type_menu_open: bool,
    last_item_id: i64,
    preview_generation: u64,
    theme: ClippiTheme,
    last_lang_version: u64,
    /// 编辑器区域占内容区的比例（0.0~1.0），默认 0.5 各占一半
    split_ratio: f32,
    /// 正在拖拽分隔手柄时的鼠标起始 Y 坐标（窗口坐标）
    split_dragging: Option<Pixels>,
    /// 拖拽开始时的 split_ratio
    split_drag_start_ratio: f32,
    /// 当从富文本类型切换到纯文本类型时，缓存原始富文本和提取的纯文本，
    /// 以便切回富文本时能将纯文本编辑同步回 HTML 标签中。
    rich_cache: Option<RichTextCache>,
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
                .placeholder(I18nKey::EditContentPlaceholder.text())
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
            last_lang_version: crate::core::i18n::lang_version(),
            split_ratio: 0.5,
            split_dragging: None,
            split_drag_start_ratio: 0.5,
            rich_cache: None,
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
        self.rich_cache = None;
        self.preview_generation = self.preview_generation.wrapping_add(1);

        // --- For HTML items, load the raw HTML from rich_data so the ---
        // --- preview can render colored <span> tags properly.         ---
        let content = if item_type == "html" {
            let rich = RichData::from_json(&item.rich_data);
            SharedString::from(rich.html.unwrap_or_else(|| item.full_text.clone()))
        } else {
            SharedString::from(item.full_text.clone())
        };
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
            self.rich_cache = None;
            cx.emit(EditPanelEvent::Saved);
        }
    }
}

impl Render for EditPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 语言切换时刷新 InputState placeholder
        let current = crate::core::i18n::lang_version();
        if self.last_lang_version != current {
            self.last_lang_version = current;
            self.content_input.update(cx, |state, cx| {
                state.set_placeholder(I18nKey::EditContentPlaceholder.text(), window, cx);
            });
        }

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
        let is_rich_editor = is_rich_editor_type(&self.selected_type);
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
            .rounded_b(px(12.))
            .overflow_hidden()
            .p(px(8.))
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .h(px(36.))
                    .child(icon_button(
                        "\u{e62b}",
                        text_2,
                        accent,
                        hover_bg,
                        Some(I18nKey::EditTooltipBack.text()),
                        {
                            let this = this.clone();
                            move |_window, cx| {
                                this.update(cx, |_panel, cx| {
                                    cx.emit(EditPanelEvent::Back);
                                });
                            }
                        },
                    ))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(I18nKey::EditPanelTitle.text()),
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
                    .child(icon_button(
                        "\u{e6da}",
                        text_2,
                        accent,
                        hover_bg,
                        Some(I18nKey::EditTooltipUrlDecode.text()),
                        {
                            let input = content_input.clone();
                            move |window, cx| {
                                EditPanel::apply_content_transform(&input, window, cx, |text| {
                                    percent_decode_str(text)
                                        .decode_utf8()
                                        .unwrap_or(Cow::Borrowed(text))
                                        .into_owned()
                                });
                            }
                        },
                    ))
                    .child(icon_button(
                        "\u{e66e}",
                        text_2,
                        accent,
                        hover_bg,
                        Some(I18nKey::EditTooltipBase64Decode.text()),
                        {
                            let input = content_input.clone();
                            move |window, cx| {
                                EditPanel::apply_content_transform(
                                    &input,
                                    window,
                                    cx,
                                    decode_base64,
                                );
                            }
                        },
                    ))
                    .child(icon_button(
                        "\u{e819}",
                        text_2,
                        accent,
                        hover_bg,
                        Some(I18nKey::EditTooltipJsonFormat.text()),
                        {
                            let input = content_input.clone();
                            move |window, cx| {
                                EditPanel::apply_content_transform(&input, window, cx, json_format);
                            }
                        },
                    ))
                    .child(icon_button(
                        "\u{e6db}",
                        text_2,
                        accent,
                        hover_bg,
                        Some(I18nKey::EditTooltipTrim.text()),
                        {
                            let input = content_input.clone();
                            move |window, cx| {
                                EditPanel::apply_content_transform(&input, window, cx, trim_text);
                            }
                        },
                    )),
            )
            .child({
                let split_ratio = self.split_ratio;
                let is_dragging = self.split_dragging.is_some();
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .overflow_hidden()
                    .when(is_dragging, |el| {
                        // 拖拽时在整个区域监听鼠标移动和释放，防止鼠标移出手柄后丢失跟踪
                        el.on_mouse_move({
                            let this = this.clone();
                            move |ev, window, cx| {
                                this.update(cx, |panel, cx| {
                                    if let Some(start_y) = panel.split_dragging {
                                        let delta = f32::from(ev.position.y) - f32::from(start_y);
                                        // 从窗口高度估算内容区高度（减去 header/toolbar/button/gap/padding ≈ 132px）
                                        let content_h = (f32::from(window.viewport_size().height)
                                            - 132.0)
                                            .max(200.0);
                                        let delta_ratio = delta / content_h;
                                        let new_ratio = (panel.split_drag_start_ratio
                                            + delta_ratio)
                                            .clamp(0.15, 0.85);
                                        panel.split_ratio = new_ratio;
                                        cx.notify();
                                    }
                                });
                            }
                        })
                        .on_mouse_up(MouseButton::Left, {
                            let this = this.clone();
                            move |_ev, _window, cx| {
                                this.update(cx, |panel, cx| {
                                    panel.split_dragging = None;
                                    cx.notify();
                                });
                            }
                        })
                    })
                    .when(!is_rich_editor, |area| {
                        area.child(editor_box(&content_input, surface, divider, px(0.), true))
                    })
                    .when(is_rich_editor, |area| {
                        area.child(
                            editor_box(&content_input, surface, divider, px(0.), false)
                                .h(relative(split_ratio)),
                        )
                        .child(
                            // 分隔拖拽手柄
                            div()
                                .h(px(4.))
                                .w_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor(CursorStyle::ResizeUpDown)
                                .on_mouse_down(MouseButton::Left, {
                                    let this = this.clone();
                                    move |ev, _window, cx| {
                                        this.update(cx, |panel, cx| {
                                            panel.split_dragging = Some(ev.position.y);
                                            panel.split_drag_start_ratio = panel.split_ratio;
                                            cx.notify();
                                        });
                                    }
                                })
                                .on_mouse_up(MouseButton::Left, {
                                    let this = this.clone();
                                    move |_ev, _window, cx| {
                                        this.update(cx, |panel, cx| {
                                            panel.split_dragging = None;
                                            cx.notify();
                                        });
                                    }
                                })
                                .child(
                                    // 手柄视觉元素 — hover 时高亮
                                    div()
                                        .w(px(32.))
                                        .h(px(3.))
                                        .rounded(px(2.))
                                        .bg(divider)
                                        .hover(|style| style.bg(accent)),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_h(px(60.))
                                .rounded(px(8.))
                                .border(px(1.))
                                .border_color(divider)
                                .bg(surface)
                                .overflow_y_scrollbar()
                                .child(
                                    div().pt(px(10.)).pb(px(10.)).pl(px(10.)).pr(px(18.)).child(
                                        render_rich_preview(
                                            &selected_type,
                                            &content_text,
                                            self.last_item_id,
                                            preview_generation,
                                            text_1,
                                            window,
                                            cx,
                                        ),
                                    ),
                                ),
                        )
                    })
            })
            .child(
                div()
                    .h(px(32.))
                    .flex()
                    .flex_row()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(text_button(
                        I18nKey::BtnCancel.text(),
                        text_2,
                        divider,
                        rgba(0x00000000),
                        {
                            let this = this.clone();
                            move |_window, cx| {
                                this.update(cx, |_panel, cx| {
                                    cx.emit(EditPanelEvent::Back);
                                });
                            }
                        },
                    ))
                    .child(text_button(
                        I18nKey::EditSave.text(),
                        rgb(0xffffff),
                        accent,
                        accent,
                        {
                            let this = this.clone();
                            move |window, cx| {
                                this.update(cx, |panel, cx| panel.save(window, cx));
                            }
                        },
                    )),
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
                            .children(TYPE_OPTIONS.into_iter().map(|(key, label_key)| {
                                let this = this.clone();
                                let key = key.to_string();
                                let label = label_key.text();
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

                                        // 提前取出 input handle，避免在 this.update 内部
                                        // 调用 input.set_value 造成 GPUI re-entrancy
                                        let input_handle = this.read(cx).content_input.clone();
                                        let mut pending_value: Option<String> = None;

                                        this.update(cx, |panel, cx| {
                                            let old_type = panel.selected_type.clone();
                                            let new_type = key.clone();

                                            // 富文本 → 纯文本：提取纯文本 + 记录文本段，缓存完整 HTML
                                            if is_rich_editor_type(&old_type)
                                                && is_plain_editor_type(&new_type)
                                            {
                                                let current = panel
                                                    .content_input
                                                    .read(cx)
                                                    .value()
                                                    .to_string();
                                                let (plain, segments) =
                                                    extract_text_and_segments(&current);
                                                panel.rich_cache = Some(RichTextCache {
                                                    html: current,
                                                    plain: plain.clone(),
                                                    segments,
                                                });
                                                pending_value = Some(plain);
                                            }
                                            // 纯文本 → 富文本：将编辑后的纯文本同步回 HTML
                                            else if is_plain_editor_type(&old_type)
                                                && is_rich_editor_type(&new_type)
                                            {
                                                if let Some(cache) = panel.rich_cache.take() {
                                                    let current_plain = panel
                                                        .content_input
                                                        .read(cx)
                                                        .value()
                                                        .to_string();
                                                    let restored = if current_plain == cache.plain {
                                                        // 未编辑 → 直接还原
                                                        cache.html
                                                    } else {
                                                        // 有编辑 → 尝试同步文本回 HTML
                                                        replace_text_in_html(
                                                            &cache.html,
                                                            &current_plain,
                                                            &cache.segments,
                                                        )
                                                    };
                                                    pending_value = Some(restored);
                                                }
                                            }

                                            panel.selected_type = new_type;
                                            panel.type_menu_open = false;
                                            panel.preview_generation =
                                                panel.preview_generation.wrapping_add(1);
                                            cx.notify();
                                        });

                                        // 在 this.update 外部应用编辑器内容，避免重入
                                        if let Some(val) = pending_value {
                                            input_handle.update(cx, |input, cx| {
                                                input.set_value(
                                                    SharedString::from(val),
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }
                                        input_handle.update(cx, |input, cx| {
                                            input.focus_handle(cx).focus(window);
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
        box_el.h(height).min_h(px(80.))
    }
}

fn render_rich_preview(
    selected_type: &str,
    text: &str,
    item_id: i64,
    generation: u64,
    fallback_color: Rgba,
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
        // --- Try to render colored spans first, fall back to plain HTML ---
        let normalized = rich_preview::normalize_clipboard_html_for_render(text);
        let stripped = rich_preview::strip_html_links(&normalized);
        if let Some(lines) = rich_preview::parse_styled_html_lines(&stripped) {
            return div()
                .child(rich_preview::render_styled_html_lines(
                    lines,
                    fallback_color,
                ))
                .into_any_element();
        }
        TextView::html(("edit-html-preview", preview_key), stripped, window, cx)
            .style(style)
            .selectable(false)
            .into_any_element()
    } else {
        TextView::markdown(
            ("edit-markdown-preview", preview_key),
            rich_preview::strip_markdown_links(text),
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
    tooltip: Option<&'static str>,
    handler: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(icon)
        .w(px(22.))
        .h(px(22.))
        .rounded(px(4.))
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
    use crate::core::types::DisplayKind;

    match item.display_kind() {
        DisplayKind::Html => "html",
        DisplayKind::Markdown => "markdown",
        DisplayKind::Rtf => "plain_text",
        DisplayKind::Email => "email",
        DisplayKind::Phone => "phone",
        DisplayKind::Link => "link",
        DisplayKind::Path => "path",
        DisplayKind::Color => "color",
        DisplayKind::Secret => "secret",
        _ => "plain_text",
    }
}

fn type_label(key: &str) -> &'static str {
    TYPE_OPTIONS
        .iter()
        .find_map(|(option_key, label_key)| (*option_key == key).then_some(label_key.text()))
        .unwrap_or(I18nKey::EditTypeText.text())
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

/// 是否为富文本编辑器类型（带有样式标签，如 HTML、Markdown）
fn is_rich_editor_type(t: &str) -> bool {
    matches!(t, "markdown" | "html")
}

/// 是否为纯文本编辑器类型（无样式标签）
fn is_plain_editor_type(t: &str) -> bool {
    matches!(
        t,
        "plain_text" | "link" | "path" | "color" | "email" | "phone" | "secret"
    )
}

/// 缓存富文本内容，支持纯文本编辑后同步回 HTML。
struct RichTextCache {
    /// 原始 HTML/富文本内容（含完整标签）
    html: String,
    /// 从 HTML 中提取的纯文本（规范化后，无多余空行）
    plain: String,
    /// 提取时记录的各文本段（顺序与 HTML 中 `>text<` 一致）
    segments: Vec<String>,
}

/// 从 HTML 提取纯文本的同时记录各文本段（用于反向同步）。
///
/// 只跟踪标签名（第一个空格或 `/` 之前的部分），跳过标签属性值，
/// 避免对 base64 等大数据调用 to_lowercase() 导致 UI 卡死。
fn extract_text_and_segments(html: &str) -> (String, Vec<String>) {
    let mut text = String::with_capacity(html.len());
    let mut segments: Vec<String> = Vec::new();
    let mut current_text = String::new(); // 标签外的文本
    let mut current_tag = String::new(); // 仅标签名（最多几十字节）
    let mut in_tag = false;
    let mut in_tag_name = true; // 仍在收集标签名（遇到空格或自闭合 / 后停止）
    let mut last_was_newline = false;

    for ch in html.chars() {
        if ch == '<' {
            // 结束当前文本段
            if !current_text.is_empty() {
                if !current_text.chars().all(|c| c.is_whitespace()) {
                    text.push_str(&current_text);
                    segments.push(current_text.clone());
                    last_was_newline = false;
                }
                current_text.clear();
            }
            in_tag = true;
            in_tag_name = true;
            current_tag.clear();
        } else if ch == '>' {
            in_tag = false;
            in_tag_name = false;
            let tag_lower = current_tag.to_lowercase(); // 标签名很短，安全
            if is_block_tag(&tag_lower) && !last_was_newline {
                text.push('\n');
                last_was_newline = true;
            }
            current_tag.clear();
        } else if in_tag {
            if in_tag_name {
                if current_tag.is_empty() && ch == '/' {
                    // 闭合标签：</p> → 保留 / 前缀用于 is_block_tag 匹配
                    current_tag.push(ch);
                } else if ch == '/' || ch.is_whitespace() {
                    // 自闭合 <br/> <br /> 或标签名结束 → 停止收集
                    in_tag_name = false;
                } else {
                    current_tag.push(ch);
                }
            }
            // 跳过标签属性值（不累积，避免大内存分配）
        } else {
            // 文本内容
            if last_was_newline && ch.is_whitespace() && ch != '\n' {
                // 跳过块级标签后的前导空白
                continue;
            }
            if ch == '\n' {
                if !last_was_newline {
                    current_text.push('\n');
                }
            } else {
                current_text.push(ch);
                last_was_newline = false;
            }
        }
    }

    // 收尾：最后一个文本段
    if !current_text.is_empty() {
        let trimmed: String = current_text
            .chars()
            .filter(|&c| c != '\n' || !last_was_newline)
            .collect();
        if !trimmed.chars().all(|c| c.is_whitespace()) {
            text.push_str(&trimmed);
            segments.push(trimmed);
        }
    }

    // 规范化：合并连续空行，去除首尾空行
    let mut result = String::with_capacity(text.len());
    let mut prev_newline = false;
    for ch in text.chars() {
        if ch == '\n' {
            if !prev_newline {
                result.push('\n');
                prev_newline = true;
            }
        } else {
            result.push(ch);
            prev_newline = false;
        }
    }
    let result = result.trim().to_string();

    // 解码常见 HTML 实体
    let result = result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");

    (result, segments)
}

/// 是否为块级 HTML 标签（闭合后应换行）
fn is_block_tag(tag: &str) -> bool {
    // 匹配闭合标签如 /p, /div 或自闭合/空标签如 br, hr
    tag == "/p"
        || tag == "/div"
        || tag == "/li"
        || tag == "/tr"
        || tag == "/h1"
        || tag == "/h2"
        || tag == "/h3"
        || tag == "/h4"
        || tag == "/h5"
        || tag == "/h6"
        || tag == "/table"
        || tag == "/ul"
        || tag == "/ol"
        || tag == "/blockquote"
        || tag == "/section"
        || tag == "/article"
        || tag == "/header"
        || tag == "/footer"
        || tag == "/nav"
        || tag == "/main"
        || tag == "/pre"
        || tag == "/figure"
        || tag == "/figcaption"
        || tag == "/dl"
        || tag == "/dt"
        || tag == "/dd"
        || tag == "/td"
        || tag == "/th"
        || tag == "/hr"
        || tag.starts_with("br")
}

/// 将编辑后的纯文本同步回 HTML，替换文本节点。
///
/// 策略：
/// 1. 从原 HTML 中重新提取文本段（`>text<` 模式）
/// 2. 如果新旧文本段数量一致 → 逐个替换
/// 3. 否则 → 返回原始 HTML（无法可靠映射）
fn replace_text_in_html(html: &str, new_plain: &str, old_segments: &[String]) -> String {
    // 解析新纯文本的段落（按换行拆分）
    let new_segments: Vec<&str> = new_plain.lines().collect();
    if new_segments.len() != old_segments.len() {
        // 行数不匹配，无法可靠替换，返回原始 HTML
        return html.to_string();
    }

    let mut result = html.to_string();
    for (old, new) in old_segments.iter().zip(new_segments.iter()) {
        if old == new {
            continue;
        }
        // 查找并替换第一个出现在 `>...<` 之间的匹配项
        let pattern = format!(">{}<", old);
        if let Some(pos) = result.find(&pattern) {
            let replacement = format!(">{}<", new);
            result.replace_range(pos..pos + pattern.len(), &replacement);
        }
    }
    result
}
