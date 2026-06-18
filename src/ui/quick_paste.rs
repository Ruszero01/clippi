//! Quick paste popup — compact, non-focus clipboard candidate list.

use gpui::*;

use crate::core::i18n_keys::I18nKey;
use crate::core::types::{ClipboardItem, ContentType, DisplayKind, FileData};
use crate::state::app::AppState;

const VISIBLE_ROWS: usize = 5;
const ROW_HEIGHT: f32 = 44.0;
pub const QUICK_WINDOW_WIDTH: f32 = 430.0;
pub const QUICK_WINDOW_HEIGHT: f32 = 252.0;

pub enum QuickPasteEvent {
    Paste(i64),
}

pub struct QuickPasteView {
    state: Entity<AppState>,
    items: Vec<ClipboardItem>,
    selected_index: usize,
    first_visible: usize,
}

impl EventEmitter<QuickPasteEvent> for QuickPasteView {}

impl QuickPasteView {
    pub fn new(state: Entity<AppState>) -> Self {
        Self {
            state,
            items: Vec::new(),
            selected_index: 0,
            first_visible: 0,
        }
    }

    pub fn set_items(&mut self, items: Vec<ClipboardItem>, cx: &mut Context<Self>) {
        self.items = items;
        self.selected_index = self.selected_index.min(self.items.len().saturating_sub(1));
        self.first_visible = self.first_visible.min(self.items.len().saturating_sub(1));
        self.ensure_selected_visible();
        cx.notify();
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        self.selected_index = (self.selected_index + 1).min(self.items.len() - 1);
        self.ensure_selected_visible();
        cx.notify();
    }

    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        self.selected_index = self.selected_index.saturating_sub(1);
        self.ensure_selected_visible();
        cx.notify();
    }

    pub fn select_visible_slot(&mut self, slot: usize, cx: &mut Context<Self>) -> Option<i64> {
        let index = self.first_visible + slot;
        if index >= self.items.len() {
            return None;
        }
        self.selected_index = index;
        self.ensure_selected_visible();
        cx.notify();
        self.selected_item_id()
    }

    pub fn selected_item_id(&self) -> Option<i64> {
        self.items.get(self.selected_index).map(|item| item.id)
    }

    fn ensure_selected_visible(&mut self) {
        if self.selected_index < self.first_visible {
            self.first_visible = self.selected_index;
        }
        if self.selected_index >= self.first_visible + VISIBLE_ROWS {
            self.first_visible = self.selected_index + 1 - VISIBLE_ROWS;
        }
    }

    fn visible_items(&self) -> impl Iterator<Item = (usize, &ClipboardItem)> {
        self.items
            .iter()
            .enumerate()
            .skip(self.first_visible)
            .take(VISIBLE_ROWS)
    }
}

impl Render for QuickPasteView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_dark = self.state.read(cx).settings.theme != "light";
        let bg = if is_dark {
            rgb(0x17191c)
        } else {
            rgb(0xf7f8fa)
        };
        let border = if is_dark {
            rgb(0x30343a)
        } else {
            rgb(0xd7dbe2)
        };
        let text = if is_dark {
            rgb(0xf2f4f8)
        } else {
            rgb(0x1d232c)
        };
        let muted = if is_dark {
            rgb(0xa3acb9)
        } else {
            rgb(0x667085)
        };
        let selected_bg = if is_dark {
            rgb(0x26313f)
        } else {
            rgb(0xe7f0ff)
        };
        let accent = rgb(0x3b82f6);

        let total = self.items.len();
        let position = if total == 0 {
            "0 / 0".to_string()
        } else {
            format!("{} / {}", self.selected_index + 1, total)
        };

        div().size_full().p(px(8.0)).bg(rgba(0x00000000)).child(
            div()
                .size_full()
                .rounded(px(8.0))
                .border_1()
                .border_color(border)
                .bg(bg)
                .shadow_lg()
                .overflow_hidden()
                .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _window, cx| {
                    let delta = ev.delta.pixel_delta(px(16.0)).y;
                    if delta < px(0.0) {
                        this.select_next(cx);
                    } else if delta > px(0.0) {
                        this.select_previous(cx);
                    }
                }))
                .child(
                    div()
                        .h(px(30.0))
                        .px(px(12.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .border_b_1()
                        .border_color(border)
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(muted)
                                .child("Quick Paste"),
                        )
                        .child(div().text_size(px(11.0)).text_color(muted).child(position)),
                )
                .children(if self.items.is_empty() {
                    vec![div()
                        .h(px(ROW_HEIGHT * VISIBLE_ROWS as f32))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(13.0))
                        .text_color(muted)
                        .child("No clipboard items")
                        .into_any_element()]
                } else {
                    self.visible_items()
                        .enumerate()
                        .map(|(slot, (index, item))| {
                            let selected = index == self.selected_index;
                            let item_id = item.id;
                            let label = type_label(item);
                            let preview = preview_text(item);
                            div()
                                .h(px(ROW_HEIGHT))
                                .px(px(10.0))
                                .flex()
                                .items_center()
                                .gap(px(9.0))
                                .bg(if selected { selected_bg } else { bg })
                                .border_b_1()
                                .border_color(border)
                                .cursor(CursorStyle::PointingHand)
                                .on_mouse_down(MouseButton::Left, {
                                    let view = cx.entity();
                                    move |ev, _window, cx| {
                                        view.update(cx, |this, cx| {
                                            this.selected_index = index;
                                            this.ensure_selected_visible();
                                            if ev.click_count == 2 {
                                                cx.emit(QuickPasteEvent::Paste(item_id));
                                            }
                                            cx.notify();
                                        });
                                    }
                                })
                                .child(
                                    div()
                                        .w(px(22.0))
                                        .h(px(22.0))
                                        .rounded(px(5.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .bg(if selected { accent } else { border })
                                        .text_color(if selected { rgb(0xffffff) } else { muted })
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::BOLD)
                                        .child((slot + 1).to_string()),
                                )
                                .child(
                                    div()
                                        .w(px(58.0))
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(if selected { accent } else { muted })
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .overflow_hidden()
                                        .text_size(px(13.0))
                                        .text_color(text)
                                        .child(preview),
                                )
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                }),
        )
    }
}

fn type_label(item: &ClipboardItem) -> String {
    match item.display_kind() {
        DisplayKind::Html => I18nKey::CardTypeHtml.text().into(),
        DisplayKind::Markdown => I18nKey::CardTypeMd.text().into(),
        DisplayKind::Rtf => I18nKey::CardTypeRtf.text().into(),
        DisplayKind::Email => I18nKey::CardTypeEmail.text().into(),
        DisplayKind::Phone => I18nKey::CardTypePhone.text().into(),
        DisplayKind::Link => I18nKey::CardTypeUrl.text().into(),
        DisplayKind::Path => I18nKey::CardTypePath.text().into(),
        DisplayKind::Color => I18nKey::CardTypeColor.text().into(),
        DisplayKind::File => I18nKey::CardTypeFile.text().into(),
        DisplayKind::Image => I18nKey::CardTypeImage.text().into(),
        DisplayKind::PlainText => I18nKey::CardTypeText.text().into(),
    }
}

fn preview_text(item: &ClipboardItem) -> String {
    let raw = match item.content_type {
        ContentType::Image => {
            if item.image_width > 0 && item.image_height > 0 {
                format!("Image {} x {}", item.image_width, item.image_height)
            } else {
                "Image".to_string()
            }
        }
        ContentType::File => {
            let data = FileData::from_json(&item.file_data);
            if data.files.is_empty() {
                item.full_text.clone()
            } else if data.files.len() == 1 {
                data.files[0].name.clone()
            } else {
                format!("{} files", data.files.len())
            }
        }
        _ => item.full_text.clone(),
    };
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}
