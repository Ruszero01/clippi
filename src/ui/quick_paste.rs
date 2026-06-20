//! Quick paste popup — compact, non-focus clipboard candidate list.

use gpui::prelude::*;
use gpui::*;
use gpui::{InteractiveElement, StatefulInteractiveElement};

use crate::core::types::{
    format_relative_time, mask_sensitive_preview, ClipboardItem, ContentType, DisplayKind, FileData,
};
use crate::state::app::AppState;
use crate::ui::search_bar::filter_type_display;
use crate::ui::theme::ClippiTheme;
use gpui_component::tooltip::Tooltip;

const VISIBLE_ROWS: usize = 5;
const ROW_HEIGHT: f32 = 44.0;
pub const QUICK_WINDOW_WIDTH: f32 = 430.0;
// Height: 5 rows (220) + type bar (30) + tag bar (26) + dividers (2) + outer pad (4) = 282
// Kept as reference; actual height is computed by calc_quick_window_height().
#[allow(dead_code)]
pub const QUICK_WINDOW_HEIGHT: f32 = 282.0;

const TYPE_BAR_HEIGHT: f32 = 30.0;
const TAG_ROW_HEIGHT: f32 = 26.0;
#[cfg(target_os = "macos")]
const OUTER_PADDING: f32 = 0.0;
#[cfg(not(target_os = "macos"))]
const OUTER_PADDING: f32 = 2.0;

pub const QUICK_WINDOW_CORNER_RADIUS: f32 = 10.0;
const HORIZONTAL_PADDING: f32 = 10.0;

/// Calculate the quick window height based on visible bars.
/// Used by window_manager for positioning and main.rs for initial window size.
pub fn calc_quick_window_height(has_tag_row: bool, has_type_bar: bool) -> f32 {
    let mut h = VISIBLE_ROWS as f32 * ROW_HEIGHT + OUTER_PADDING * 2.0;
    if has_type_bar {
        h += TYPE_BAR_HEIGHT + 1.0; // bar + divider
    }
    if has_tag_row {
        h += TAG_ROW_HEIGHT + 1.0; // bar + divider
    }
    h
}

/// (slot, id, icon, preview_text, note, relative_time, image_path)
type RowData = (
    usize,
    i64,
    &'static str,
    String,
    String,
    String,
    Option<String>,
);

pub enum QuickPasteEvent {
    Paste(i64),
}

pub struct QuickPasteView {
    state: Entity<AppState>,
    selected_index: usize,
    first_visible: usize,
}

impl EventEmitter<QuickPasteEvent> for QuickPasteView {}

impl QuickPasteView {
    pub fn new(state: Entity<AppState>) -> Self {
        Self {
            state,
            selected_index: 0,
            first_visible: 0,
        }
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        let len = self.state.read(cx).items.len();
        if len == 0 {
            return;
        }
        self.selected_index = (self.selected_index + 1).min(len - 1);
        self.ensure_selected_visible();
        cx.notify();
    }

    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        let len = self.state.read(cx).items.len();
        if len == 0 {
            return;
        }
        self.selected_index = self.selected_index.saturating_sub(1);
        self.ensure_selected_visible();
        cx.notify();
    }

    pub fn select_next_page(&mut self, cx: &mut Context<Self>) {
        let len = self.state.read(cx).items.len();
        if len == 0 {
            return;
        }
        let last_page = (len - 1) / VISIBLE_ROWS;
        let current_page = self.selected_index / VISIBLE_ROWS;
        if current_page >= last_page {
            return;
        }
        let next_page = current_page + 1;
        self.selected_index = next_page * VISIBLE_ROWS;
        self.first_visible = self.selected_index;
        cx.notify();
    }

    pub fn select_previous_page(&mut self, cx: &mut Context<Self>) {
        if self.state.read(cx).items.is_empty() {
            return;
        }
        let current_page = self.selected_index / VISIBLE_ROWS;
        if current_page == 0 {
            return;
        }
        let previous_page = current_page - 1;
        self.selected_index = previous_page * VISIBLE_ROWS;
        self.first_visible = self.selected_index;
        cx.notify();
    }

    pub fn select_visible_slot(&mut self, slot: usize, cx: &mut Context<Self>) -> Option<i64> {
        let index = self.first_visible + slot;
        if index >= self.state.read(cx).items.len() {
            return None;
        }
        self.selected_index = index;
        self.ensure_selected_visible();
        cx.notify();
        self.selected_item_id(cx)
    }

    /// Select a specific index (for mouse click).
    pub fn select_index(&mut self, index: usize, cx: &mut Context<Self>) {
        let len = self.state.read(cx).items.len();
        if index >= len {
            return;
        }
        self.selected_index = index;
        self.ensure_selected_visible();
        cx.notify();
    }

    pub fn selected_item_id(&self, cx: &Context<Self>) -> Option<i64> {
        self.state
            .read(cx)
            .items
            .get(self.selected_index)
            .map(|item| item.id)
    }

    fn ensure_selected_visible(&mut self) {
        if self.selected_index < self.first_visible {
            self.first_visible = self.selected_index;
        }
        if self.selected_index >= self.first_visible + VISIBLE_ROWS {
            self.first_visible = self.selected_index + 1 - VISIBLE_ROWS;
        }
    }

    /// Reset scroll to top (called on filter change / window open).
    pub fn reset_scroll(&mut self, cx: &mut Context<Self>) {
        self.selected_index = 0;
        self.first_visible = 0;
        cx.notify();
    }

    fn row_data(&self, cx: &Context<Self>) -> Vec<RowData> {
        self.state
            .read(cx)
            .items
            .iter()
            .skip(self.first_visible)
            .take(VISIBLE_ROWS)
            .enumerate()
            .map(|(slot, item)| {
                let is_image =
                    item.content_type == ContentType::Image && !item.image_path.is_empty();
                (
                    slot,
                    item.id,
                    type_icon(item),
                    preview_text(item),
                    item.note.clone(),
                    format_relative_time(&item.updated_at),
                    is_image.then(|| item.image_path.clone()),
                )
            })
            .collect()
    }

    fn theme(&self, appearance: WindowAppearance, cx: &Context<Self>) -> ClippiTheme {
        ClippiTheme::from_setting(&self.state.read(cx).settings.theme, Some(appearance))
    }
}

impl Render for QuickPasteView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme(window.appearance(), cx);
        let view_entity = cx.entity();

        let (
            type_config,
            pinned_tag_ids,
            filters,
            tags_snapshot,
            items_count,
            show_original_on_hover,
        ) = {
            let state = self.state.read(cx);
            (
                state.settings.type_filter_config.clone(),
                state.settings.pinned_tag_ids.clone(),
                state.filters.clone(),
                state.tags.clone(),
                state.items.len(),
                state.settings.show_original_on_hover,
            )
        };

        // Resolve pinned tags (only show if they exist in DB)
        let pinned_tags: Vec<(i64, String, String)> = pinned_tag_ids
            .iter()
            .filter_map(|&id| {
                tags_snapshot
                    .iter()
                    .find(|t| t.id == id)
                    .map(|t| (t.id, t.name.clone(), t.color.clone()))
            })
            .collect();

        let has_type_bar = !type_config.is_empty();
        let has_tag_row = !pinned_tags.is_empty();

        // ── Tag compact mode detection ──
        // Estimate total tag width; if it overflows the row, switch to flex_1 equal division.
        let tag_avail = QUICK_WINDOW_WIDTH - OUTER_PADDING * 2.0 - HORIZONTAL_PADDING * 2.0;
        let tag_gap = 4.0;
        let char_est: f32 = 6.5; // rough char width at 10px font
        let tag_pad: f32 = 12.0; // px(6.0) * 2
        let total_est: f32 = pinned_tags
            .iter()
            .map(|(_, name, _)| name.chars().count() as f32 * char_est + tag_pad)
            .sum();
        let gaps = (pinned_tags.len().max(1) - 1) as f32 * tag_gap;
        let tag_compact = (total_est + gaps) > tag_avail * 0.88;

        // ── Dynamic type filter sizing ──
        let visible_type_entries: Vec<&crate::core::settings::TypeFilterEntry> =
            type_config.iter().filter(|e| e.visible).collect();
        let visible_count = visible_type_entries.len().max(1) as f32;
        // Use design width constant so sizing is deterministic — viewport
        // may vary on first paint before SetWindowPos enforces correct size.
        let type_bar_avail = QUICK_WINDOW_WIDTH - OUTER_PADDING * 2.0 - HORIZONTAL_PADDING * 2.0;
        let text_gap_total = 4.0 * (visible_count - 1.0).max(0.0); // TYPE_FILTER_TEXT_GAP
        let type_slot_width = (type_bar_avail - text_gap_total).max(0.0) / visible_count;
        let icon_only = type_slot_width < 50.0; // TYPE_FILTER_TEXT_MIN_SLOT_WIDTH
        let filter_gap = if icon_only { 3.0 } else { 4.0 };
        let inactive_bg = if theme.bg == rgb(0x191a1b) {
            rgba(0xffffff0a)
        } else {
            rgba(0x00000008)
        };

        let window_h = calc_quick_window_height(has_tag_row, has_type_bar);

        div()
            .w(px(QUICK_WINDOW_WIDTH))
            .h(px(window_h))
            .p(px(OUTER_PADDING))
            .child(
                div()
                    .size_full()
                    .rounded(px(QUICK_WINDOW_CORNER_RADIUS))
                    .border_1()
                    .border_color(theme.divider)
                    .bg(theme.bg)
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _window, cx| {
                        let delta = ev.delta.pixel_delta(px(16.0)).y;
                        if delta < px(0.0) {
                            this.select_next(cx);
                        } else if delta > px(0.0) {
                            this.select_previous(cx);
                        }
                    }))
                    // ── Type filter bar ──
                    .when(has_type_bar, |parent| {
                        parent.child(
                            div()
                                .h(px(TYPE_BAR_HEIGHT))
                                .px(px(HORIZONTAL_PADDING))
                                .flex()
                                .items_center()
                                .gap(px(filter_gap))
                                .children(visible_type_entries.iter().map(|entry| {
                                    let active = filters.is_type_active(&entry.key);
                                    let (icon, label) = filter_type_display(&entry.key)
                                        .unwrap_or(("\u{e60e}", "".into()));
                                    let key = entry.key.clone();
                                    let t = theme.clone();
                                    let app_state = self.state.clone();
                                    let filter_text = if active { rgb(0xffffff) } else { t.text_2 };
                                    div()
                                        .h(px(22.0))
                                        .when(icon_only, |b| {
                                            b.flex_1().min_w(px(0.0)).justify_center()
                                        })
                                        .when(!icon_only, |b| {
                                            b.flex_1()
                                                .min_w(px(0.0))
                                                .justify_center()
                                                .px(px(5.0))
                                                .gap(px(2.0))
                                        })
                                        .rounded(px(5.0))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .bg(if active { t.accent } else { inactive_bg })
                                        .cursor(CursorStyle::PointingHand)
                                        .on_mouse_down(MouseButton::Left, {
                                            let s = app_state.clone();
                                            let k = key.clone();
                                            let v = view_entity.clone();
                                            move |_, _window, cx| {
                                                s.update(cx, |s, _cx| {
                                                    s.toggle_type_filter(&k);
                                                });
                                                v.update(cx, |view, cx| view.reset_scroll(cx));
                                            }
                                        })
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_family("iconfont")
                                                .text_color(filter_text)
                                                .child(icon),
                                        )
                                        .when(!icon_only, |b| {
                                            b.child(
                                                div()
                                                    .text_size(px(11.0))
                                                    .text_color(filter_text)
                                                    .child(label),
                                            )
                                        })
                                        .into_any_element()
                                })),
                        )
                    })
                    .when(has_type_bar, |parent| {
                        parent.child(div().h(px(1.0)).w_full().bg(theme.divider))
                    })
                    // ── Pinned tag row ──
                    .when(has_tag_row, |parent| {
                        parent.child(
                            div()
                                .h(px(TAG_ROW_HEIGHT))
                                .px(px(HORIZONTAL_PADDING))
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .children(pinned_tags.iter().map(|(id, name, color_hex)| {
                                    let active = filters.tag_ids.contains(id);
                                    let tag_color = parse_hex_for_tag(color_hex);
                                    let tag_id = *id;
                                    let app_state = self.state.clone();
                                    let tag_name = name.clone();
                                    let tag_name_for_tip = name.clone();
                                    div()
                                        .id(("quick-tag", tag_id as u64))
                                        .h(px(20.0))
                                        .px(px(6.0))
                                        .rounded(px(4.0))
                                        .flex()
                                        .items_center()
                                        .when(tag_compact, |d| d.flex_1().min_w(px(0.0)))
                                        .when(!tag_compact, |d| d.max_w(px(120.0)))
                                        .overflow_hidden()
                                        .text_size(px(10.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .bg(if active { theme.accent } else { tag_color })
                                        .text_color(rgb(0xffffff))
                                        .cursor(CursorStyle::PointingHand)
                                        .when(tag_compact, move |d| {
                                            let tip = tag_name_for_tip;
                                            d.tooltip(move |window, cx| {
                                                let tip = tip.clone();
                                                Tooltip::element(move |_window, _cx| {
                                                    div().text_size(px(10.)).child(tip.clone())
                                                })
                                                .build(window, cx)
                                            })
                                        })
                                        .on_mouse_down(MouseButton::Left, {
                                            let s = app_state.clone();
                                            let v = view_entity.clone();
                                            move |_, _window, cx| {
                                                s.update(cx, |s, _cx| {
                                                    s.toggle_tag_filter(tag_id);
                                                });
                                                v.update(cx, |view, cx| view.reset_scroll(cx));
                                            }
                                        })
                                        .child(
                                            div()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .child(tag_name),
                                        )
                                        .into_any_element()
                                })),
                        )
                    })
                    .when(has_tag_row, |parent| {
                        parent.child(div().h(px(1.0)).w_full().bg(theme.divider))
                    })
                    // ── Empty state ──
                    .when(items_count == 0, |parent| {
                        parent.child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(13.0))
                                .text_color(theme.text_2)
                                .child("No clipboard items"),
                        )
                    })
                    // ── List rows ──
                    .children({
                        let t = theme.clone();
                        let selected_index = self.selected_index;
                        let first_visible = self.first_visible;
                        let view_entity = cx.entity();
                        self.row_data(cx)
                            .into_iter()
                            .map(
                                move |(slot, item_id, icon, preview, note, time, img_path)| {
                                    let index = first_visible + slot;
                                    let selected = index == selected_index;
                                    let t = t.clone();
                                    let ve = view_entity.clone();

                                    let show_note =
                                        !(note.is_empty() || show_original_on_hover && selected);
                                    let content_cell = if show_note {
                                        // Note takes precedence for every content type, including images.
                                        div()
                                            .flex_1()
                                            .overflow_hidden()
                                            .text_size(px(12.0))
                                            .text_color(t.text_2)
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(note)
                                            .into_any_element()
                                    } else if let Some(ref path) = img_path {
                                        let thumb_h = ROW_HEIGHT - 6.0;
                                        div()
                                            .flex_1()
                                            .h(px(thumb_h))
                                            .rounded(px(4.0))
                                            .overflow_hidden()
                                            .flex()
                                            .items_center()
                                            .child(
                                                gpui::img(std::path::Path::new(path))
                                                    .h(px(thumb_h))
                                                    .object_fit(ObjectFit::Contain),
                                            )
                                            .into_any_element()
                                    } else {
                                        // No note, or selected with show_original_on_hover → show original (masked)
                                        div()
                                            .flex_1()
                                            .overflow_hidden()
                                            .text_size(px(13.0))
                                            .text_color(t.text_1)
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(preview)
                                            .into_any_element()
                                    };

                                    div()
                                        .h(px(ROW_HEIGHT))
                                        .px(px(HORIZONTAL_PADDING))
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .bg(if selected {
                                            t.accent_overlay()
                                        } else {
                                            rgba(0x00000000)
                                        })
                                        .cursor(CursorStyle::PointingHand)
                                        .on_mouse_down(MouseButton::Left, {
                                            let ve2 = ve.clone();
                                            let ve3 = ve.clone();
                                            move |ev, _window, cx| {
                                                // Single-click: select
                                                ve2.update(cx, |view, cx| {
                                                    view.select_index(index, cx);
                                                });
                                                // Double-click: paste
                                                if ev.click_count == 2 {
                                                    ve3.update(cx, |_, cx| {
                                                        cx.emit(QuickPasteEvent::Paste(item_id));
                                                    });
                                                }
                                            }
                                        })
                                        // Slot number badge
                                        .child(
                                            div()
                                                .w(px(22.0))
                                                .h(px(22.0))
                                                .rounded(px(5.0))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .bg(if selected {
                                                    t.accent
                                                } else {
                                                    rgba(0x00000000)
                                                })
                                                .text_color(if selected {
                                                    rgb(0xffffff)
                                                } else {
                                                    t.text_2
                                                })
                                                .text_size(px(11.0))
                                                .font_weight(FontWeight::BOLD)
                                                .child((slot + 1).to_string()),
                                        )
                                        // Type icon
                                        .child(
                                            div()
                                                .w(px(16.0))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_size(px(12.0))
                                                .font_family("iconfont")
                                                .text_color(if selected {
                                                    t.accent
                                                } else {
                                                    t.text_2
                                                })
                                                .child(icon),
                                        )
                                        .child(content_cell)
                                        // Relative time
                                        .child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(t.text_3)
                                                .whitespace_nowrap()
                                                .child(time),
                                        )
                                        .into_any_element()
                                },
                            )
                            .collect::<Vec<_>>()
                    }),
            )
    }
}

// ── Helpers (adapted from clipboard_card.rs) ──

fn type_icon(item: &ClipboardItem) -> &'static str {
    if item.meta_type == "email" {
        return "\u{e604}";
    }
    if item.meta_type == "phone" {
        return "\u{e966}";
    }
    if item.content_type == ContentType::Image
        && crate::core::types::RichData::from_json(&item.rich_data)
            .qr_text
            .is_some()
    {
        return "\u{e605}";
    }
    match item.display_kind() {
        DisplayKind::PlainText => "\u{e60e}",
        DisplayKind::Html | DisplayKind::Markdown | DisplayKind::Rtf => "\u{e6ae}",
        DisplayKind::Image => "\u{e626}",
        DisplayKind::File => "\u{e68a}",
        DisplayKind::Link => "\u{e6d7}",
        DisplayKind::Path => "\u{e60f}",
        DisplayKind::Color => "\u{e610}",
        DisplayKind::Email => "\u{e604}",
        DisplayKind::Phone => "\u{e966}",
    }
}

fn preview_text(item: &ClipboardItem) -> String {
    let raw = match item.content_type {
        ContentType::Image => {
            if item.image_width > 0 && item.image_height > 0 {
                format!("Image {}×{}", item.image_width, item.image_height)
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
    let raw = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    // Mask sensitive content (email / phone) in preview
    mask_sensitive_preview(&raw, &item.meta_type)
}

fn parse_hex_for_tag(hex: &str) -> Rgba {
    use crate::core::types::parse_hex_color;
    parse_hex_color(hex)
        .map(|(r, g, b)| rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32))
        .unwrap_or(rgb(0x3b82f6))
}
