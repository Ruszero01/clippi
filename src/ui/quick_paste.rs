//! Quick paste popup — compact, non-focus clipboard candidate list.

use std::time::Duration;

use gpui::prelude::*;
use gpui::*;
use gpui::{InteractiveElement, StatefulInteractiveElement};

use super::rich_preview;
use crate::core::color::detect_color;
use crate::core::secret::sensitive_preview_to_text;
use crate::core::types::{
    format_relative_time, path_is_native, url_domain, url_path, url_site_name, ClipboardItem,
    ContentType, DisplayKind, FileData, RichData,
};
use crate::services::favicon::favicon_cache_path;
use crate::state::app::AppState;
use crate::ui::filter_bar::filter_type_display;
use crate::ui::theme::ClippiTheme;
use gpui_component::tooltip::Tooltip;
use gpui_transitions::WindowUseTransition;

const VISIBLE_ROWS: usize = 5;
const ROW_HEIGHT: f32 = 44.0;
pub const QUICK_WINDOW_WIDTH: f32 = 430.0;
// Dynamic height is computed by calc_quick_window_height().

const TYPE_BAR_HEIGHT: f32 = 30.0;
const TAG_ROW_HEIGHT: f32 = 26.0;
const TAG_INDICATOR_ANIM_DURATION: Duration = Duration::from_millis(180);
/// Width of the favorites filter button (icon is ~14px, 3px padding each side).
const FAV_BUTTON_SIZE: f32 = 20.0;
/// Gap between the type filter area and the favorites button.
const FAV_BUTTON_GAP: f32 = 6.0;

pub const QUICK_WINDOW_CORNER_RADIUS: f32 = 8.0;
const HORIZONTAL_PADDING: f32 = 10.0;
const LIST_INSET: f32 = 4.0;
const HINT_BAR_HEIGHT: f32 = 24.0;
const TOOLTIP_ITEM_HEIGHT: f32 = 28.0;
const TOOLTIP_PADDING: f32 = 6.0;

fn advanced_modifier_pressed(modifiers: Modifiers) -> bool {
    #[cfg(target_os = "macos")]
    {
        modifiers.control || modifiers.secondary()
    }
    #[cfg(not(target_os = "macos"))]
    {
        modifiers.control
    }
}

fn advanced_modifier_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "\u{2318}"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "Ctrl"
    }
}

fn quick_tag_indicator_ease(delta: f32) -> f32 {
    1.0 - (1.0 - delta).powi(3)
}

/// Calculate the quick window height based on visible bars.
/// Used by window_manager for positioning and main.rs for initial window size.
pub fn calc_quick_window_height(has_tag_row: bool, has_type_bar: bool) -> f32 {
    let mut h = VISIBLE_ROWS as f32 * ROW_HEIGHT + LIST_INSET * 2.0;
    // Top bar always exists (contains at minimum the favorites filter button).
    h += TYPE_BAR_HEIGHT;
    if has_type_bar {
        h += 1.0; // divider below type bar area
    }
    if has_tag_row {
        h += TAG_ROW_HEIGHT + 1.0; // bar + divider
    }
    h + HINT_BAR_HEIGHT // always: bottom hint bar
}

/// (slot, id, icon, color_swatch, preview_text, preview_subtitle, note, relative_time, image_path, favicon_path, file_icon_path, path_color, styled_first_line)
type RowData = (
    usize,
    i64,
    &'static str,
    Option<Rgba>,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<Rgba>,
    Option<Vec<rich_preview::StyledHtmlSpan>>,
);

pub enum QuickPasteEvent {
    Paste(i64),
    PastePlain(i64),       // Shift+click → force plain text
    PasteAlt(i64, String), // advanced modifier click / tooltip click → advanced mode
}

pub struct QuickPasteView {
    state: Entity<AppState>,
    selected_index: usize,
    first_visible: usize,
    /// Hover tracking — when set, drives selection in single-item mode.
    hovered_index: Option<usize>,
    _appearance_subscription: Subscription,
    // Modifier key state (updated by WindowManager poll)
    pub(crate) shift_held: bool,
    pub(crate) ctrl_held: bool,
    /// Which advanced paste mode is currently selected for the current item type.
    current_alt_index: usize,
    pub(crate) current_alt_mode: String,
    pub(crate) available_alt_modes: Vec<String>,
    /// Accumulated scroll delta for page-flip threshold (prevents touchpad over-scroll).
    scroll_accumulator: f32,
}

impl EventEmitter<QuickPasteEvent> for QuickPasteView {}

impl QuickPasteView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance_subscription =
            cx.observe_window_appearance(window, |_this, _window, cx| cx.notify());
        let image_alt_mode = state.read(cx).settings.image_alt_mode.clone();
        Self {
            state,
            selected_index: 0,
            first_visible: 0,
            hovered_index: None,
            _appearance_subscription: appearance_subscription,
            shift_held: false,
            ctrl_held: false,
            current_alt_index: 0,
            current_alt_mode: image_alt_mode,
            available_alt_modes: Vec::new(),
            scroll_accumulator: 0.0,
        }
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        let len = self.state.read(cx).items.len();
        if len == 0 {
            return;
        }
        self.selected_index = (self.selected_index + 1).min(len - 1);
        self.ensure_selected_visible();
        if self.ctrl_held {
            self.update_alt_modes_for_selection(cx);
        }
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
        let current_page = self.selected_index / VISIBLE_ROWS;
        let offset = self.selected_index % VISIBLE_ROWS;
        let last_page = (len - 1) / VISIBLE_ROWS;
        if current_page >= last_page {
            return;
        }
        let next_page = current_page + 1;
        self.first_visible = next_page * VISIBLE_ROWS;
        self.selected_index = (self.first_visible + offset).min(len - 1);
        cx.notify();
    }

    pub fn select_previous_page(&mut self, cx: &mut Context<Self>) {
        let len = self.state.read(cx).items.len();
        if len == 0 {
            return;
        }
        let current_page = self.selected_index / VISIBLE_ROWS;
        if current_page == 0 {
            return;
        }
        let offset = self.selected_index % VISIBLE_ROWS;
        let previous_page = current_page - 1;
        self.first_visible = previous_page * VISIBLE_ROWS;
        self.selected_index = (self.first_visible + offset).min(len - 1);
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
        if self.ctrl_held {
            self.update_alt_modes_for_selection(cx);
        }
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
        let settings = &self.state.read(cx).settings;
        let auto_fetch_title = settings.auto_fetch_url_title;

        self.state
            .read(cx)
            .items
            .iter()
            .skip(self.first_visible)
            .take(VISIBLE_ROWS)
            .enumerate()
            .filter(|(_, item)| {
                // Exclude items whose source file is gone — useless for quick paste.
                let is_remote = RichData::from_json(&item.rich_data).remote_host.is_some();
                if !is_remote
                    && item.content_type == ContentType::Image
                    && !item.image_path.is_empty()
                {
                    return std::path::Path::new(&item.image_path).exists();
                }
                if !is_remote && item.content_type == ContentType::File {
                    let fd = FileData::from_json(&item.file_data);
                    // Only filter single-file items; multi-file stays.
                    if fd.files.len() == 1 {
                        if let Some(fi) = fd.files.first() {
                            return std::path::Path::new(&fi.path).exists();
                        }
                    }
                }
                true
            })
            .map(|(slot, item)| {
                let remote_host = RichData::from_json(&item.rich_data).remote_host;
                let is_image =
                    item.content_type == ContentType::Image && !item.image_path.is_empty();
                let color_swatch = if item.display_kind() == DisplayKind::Color {
                    detect_color(&item.full_text)
                        .map(|c| rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32))
                } else {
                    None
                };
                // Favicon cache path for link-type items
                let favicon_path = if item.meta_type == "link" {
                    let domain = url_domain(&item.full_text);
                    if !domain.is_empty() {
                        favicon_cache_path(&domain)
                    } else {
                        None
                    }
                } else {
                    None
                };
                let (preview_label, preview_subtitle) = preview_parts(item, auto_fetch_title);
                // File icon cache path for file-type items
                let file_icon_path = if item.content_type == ContentType::File
                    && remote_host.is_none()
                {
                    let data = FileData::from_json(&item.file_data);
                    data.files
                        .first()
                        .and_then(|fi| {
                            crate::ui::clipboard_card::cached_file_icon_path(&fi.path, fi.is_dir)
                        })
                        .map(|p| p.to_string_lossy().to_string())
                } else {
                    None
                };
                // Path color: invalid → danger, foreign → warn
                let path_color = if item.meta_type == "path" {
                    let foreign = !path_is_native(&item.full_text);
                    let invalid = !foreign && !crate::core::types::path_exists(&item.full_text);
                    if invalid {
                        Some(rgb(0xef4444)) // danger
                    } else if foreign {
                        Some(rgb(0xeab308)) // path_warn
                    } else {
                        None
                    }
                } else {
                    None
                };
                let styled_first_line = styled_preview(item);
                (
                    slot,
                    item.id,
                    type_icon(item),
                    color_swatch,
                    preview_label,
                    preview_subtitle,
                    item.note.clone(),
                    format_relative_time(&item.updated_at),
                    (is_image && remote_host.is_none()).then(|| item.image_path.clone()),
                    favicon_path,
                    file_icon_path,
                    path_color,
                    styled_first_line,
                )
            })
            .collect()
    }

    // ── Modifier key & advanced paste ──

    /// Update modifier key state from external poll.
    pub fn set_modifiers(&mut self, shift: bool, ctrl: bool, cx: &mut Context<Self>) {
        let changed = self.shift_held != shift || self.ctrl_held != ctrl;
        self.shift_held = shift;
        self.ctrl_held = ctrl;
        if changed {
            if ctrl {
                self.update_alt_modes_for_selection(cx);
            }
            cx.notify();
        }
    }

    /// Refresh available_alt_modes and current_alt_index based on the currently
    /// selected item's type. Only images and colors have advanced modes.
    fn update_alt_modes_for_selection(&mut self, cx: &Context<Self>) {
        self.available_alt_modes.clear();
        self.current_alt_index = 0;

        let state = self.state.read(cx);
        let Some(item) = state.items.get(self.selected_index) else {
            return;
        };

        let is_image = item.content_type == ContentType::Image;
        let is_file = item.content_type == ContentType::File && !item.file_data.is_empty();
        let is_color = item.meta_type == "color";

        if is_image {
            self.available_alt_modes.push("bitmap".to_string());
            self.available_alt_modes.push("path".to_string());
            self.available_alt_modes.push("ocr".to_string());
            let default_mode = &state.settings.image_alt_mode;
            if let Some(pos) = self
                .available_alt_modes
                .iter()
                .position(|m| m == default_mode)
            {
                self.current_alt_index = pos;
                self.current_alt_mode = default_mode.clone();
            }
        } else if is_file {
            self.available_alt_modes.push("file_path".to_string());
            self.current_alt_mode = "file_path".to_string();
        } else if is_color {
            let full_text = &item.full_text;
            let current_is_hex = detect_color(full_text)
                .map(|_c| {
                    full_text.starts_with('#')
                        || (full_text.len() == 6
                            && full_text.chars().all(|ch| ch.is_ascii_hexdigit()))
                })
                .unwrap_or(false);
            if current_is_hex {
                self.available_alt_modes.push("rgb".to_string());
                self.current_alt_mode = "rgb".to_string();
            } else {
                self.available_alt_modes.push("hex".to_string());
                self.current_alt_mode = "hex".to_string();
            }
        }
    }

    pub(crate) fn ensure_alt_modes_for_selection(&mut self, cx: &Context<Self>) -> bool {
        self.update_alt_modes_for_selection(cx);
        !self.available_alt_modes.is_empty()
    }

    /// Whether the currently selected item has any advanced paste modes.
    pub(crate) fn has_alt_modes(&self) -> bool {
        !self.available_alt_modes.is_empty()
    }

    /// Cycle the current alt mode and persist the new default.
    pub fn cycle_alt_mode(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.available_alt_modes.is_empty() {
            self.update_alt_modes_for_selection(cx);
        }
        if self.available_alt_modes.is_empty() {
            return;
        }
        let len = self.available_alt_modes.len() as i32;
        let new_index = (self.current_alt_index as i32 + delta).rem_euclid(len) as usize;
        self.current_alt_index = new_index;
        self.current_alt_mode = self.available_alt_modes[new_index].clone();

        // Persist image-specific alt mode to settings (skip "plain").
        let mode = self.current_alt_mode.clone();
        if matches!(mode.as_str(), "bitmap" | "path" | "ocr") {
            self.state.update(cx, |state, _cx| {
                state.settings.image_alt_mode = mode;
                state.settings.save();
            });
        }
        cx.notify();
    }

    /// Items for the floating tooltip menu: (label, mode, is_selected).
    fn tooltip_items(&self) -> Vec<(&'static str, String, bool)> {
        if self.ctrl_held && !self.available_alt_modes.is_empty() {
            return self
                .available_alt_modes
                .iter()
                .enumerate()
                .map(|(i, mode)| {
                    let label = match mode.as_str() {
                        "bitmap" => "粘贴为位图",
                        "path" => "粘贴图片路径",
                        "ocr" => "粘贴OCR文本",
                        "rgb" => "粘贴为RGB",
                        "hex" => "粘贴为HEX",
                        "file_path" => "粘贴文件路径",
                        _ => "高级粘贴",
                    };
                    (label, mode.clone(), i == self.current_alt_index)
                })
                .collect();
        }
        Vec::new()
    }

    /// Computes the tooltip Y position (right edge is fixed at 8px inset).
    fn tooltip_position(
        &self,
        has_tag_row: bool,
        has_type_bar: bool,
        cx: &mut Context<Self>,
    ) -> Option<Pixels> {
        let item_count = self.state.read(cx).items.len();
        if item_count == 0 {
            return None;
        }

        let visible_idx = self.selected_index.saturating_sub(self.first_visible);
        let bars_offset = TYPE_BAR_HEIGHT // always present (fav button row)
            + if has_type_bar { 1.0 } else { 0.0 } // divider only with type bar
            + if has_tag_row {
                TAG_ROW_HEIGHT + 1.0
            } else {
                0.0
            };

        let row_top = visible_idx as f32 * ROW_HEIGHT + LIST_INSET + bars_offset;
        let items = self.tooltip_items();
        if items.is_empty() {
            return None;
        }
        let tooltip_h = items.len() as f32 * TOOLTIP_ITEM_HEIGHT + TOOLTIP_PADDING * 2.0;
        // Hint bar is always present now
        let content_h = VISIBLE_ROWS as f32 * ROW_HEIGHT + LIST_INSET * 2.0 + bars_offset;

        let space_below = content_h - row_top - ROW_HEIGHT;
        let space_above = row_top;

        let tooltip_y = if space_below >= tooltip_h {
            row_top + ROW_HEIGHT
        } else if space_above >= tooltip_h {
            row_top - tooltip_h
        } else {
            (content_h - tooltip_h).max(0.0)
        };

        Some(px(tooltip_y))
    }

    fn theme(&self, appearance: WindowAppearance, cx: &Context<Self>) -> ClippiTheme {
        ClippiTheme::from_setting(&self.state.read(cx).settings.theme, Some(appearance))
    }
}

impl Render for QuickPasteView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme(window.appearance(), cx);
        let view_entity = cx.entity();

        let (type_config, filters, items_count, show_original_on_hover, pinned_tags) = {
            let state = self.state.read(cx);
            // Only clone tag data for pinned tags (avoid cloning all tags every frame)
            let pinned_tags: Vec<(i64, String, String)> = state
                .settings
                .pinned_tag_ids
                .iter()
                .filter_map(|&id| {
                    state
                        .tags
                        .iter()
                        .find(|t| t.id == id)
                        .map(|t| (t.id, t.name.clone(), t.color.clone()))
                })
                .collect();
            (
                state.settings.type_filter_config.clone(),
                state.filters.clone(),
                state.items.len(),
                state.settings.show_original_on_hover,
                pinned_tags,
            )
        };

        let has_type_bar = !type_config.is_empty();
        let has_tag_row = !pinned_tags.is_empty();
        let fav_active = filters.is_favorites_active();

        // ── Tag compact mode detection ──
        // Estimate total tag width; if it overflows the row, switch to flex_1 equal division.
        let tag_avail = QUICK_WINDOW_WIDTH - HORIZONTAL_PADDING * 2.0;
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
        let fav_space = if has_type_bar {
            FAV_BUTTON_SIZE + FAV_BUTTON_GAP
        } else {
            0.0
        };
        let type_bar_avail = QUICK_WINDOW_WIDTH - HORIZONTAL_PADDING * 2.0 - fav_space;
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
            .rounded(px(QUICK_WINDOW_CORNER_RADIUS))
            .overflow_hidden()
            .bg(theme.bg)
            .flex()
            .flex_col()
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _window, cx| {
                let pixel = ev.delta.pixel_delta(px(16.0));
                if advanced_modifier_pressed(ev.modifiers) {
                    // Advanced modifier + scroll → cycle advanced mode
                    let delta = if pixel.y < px(0.0) || pixel.x < px(0.0) {
                        1
                    } else {
                        -1
                    };
                    this.cycle_alt_mode(delta, cx);
                } else {
                    // Extract f32 from Pixels via Div (Pixels / Pixels = f32).
                    let dy = pixel.y / px(1.0);
                    // Reset accumulator on direction change to avoid momentum carry-over.
                    if dy.signum() != this.scroll_accumulator.signum()
                        && this.scroll_accumulator != 0.0
                    {
                        this.scroll_accumulator = 0.0;
                    }
                    this.scroll_accumulator += dy;
                    // Page-flip threshold: ROW_HEIGHT pixels triggers one page scroll.
                    let threshold = ROW_HEIGHT;
                    while this.scroll_accumulator <= -threshold {
                        this.scroll_accumulator += threshold;
                        this.select_next_page(cx);
                    }
                    while this.scroll_accumulator >= threshold {
                        this.scroll_accumulator -= threshold;
                        this.select_previous_page(cx);
                    }
                }
            }))
            // ── Top bar (always visible: type filters + favorites button) ──
            .child(
                div()
                    .h(px(TYPE_BAR_HEIGHT))
                    .px(px(HORIZONTAL_PADDING))
                    .flex()
                    .items_center()
                    .gap(px(filter_gap))
                    // Type filter buttons area (only when has_type_bar, takes remaining space)
                    .when(has_type_bar, |row| {
                        row.child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .gap(px(filter_gap))
                                .children(visible_type_entries.iter().map(|entry| {
                                    let active = filters.is_type_active(&entry.key);
                                    let (icon, label) = filter_type_display(&entry.key)
                                        .unwrap_or(("\u{e606}", "".into()));
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
                    // Spacer to push fav button to the right when no type filters are shown.
                    .when(!has_type_bar, |row| row.child(div().flex_1()))
                    // Favorites filter button (always visible, fixed width, right-aligned)
                    .child(
                        div()
                            .w(px(FAV_BUTTON_SIZE))
                            .h(px(FAV_BUTTON_SIZE))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, {
                                let s = self.state.clone();
                                let v = view_entity.clone();
                                move |_, _window, cx| {
                                    s.update(cx, |s, _cx| {
                                        s.toggle_favorites_filter();
                                    });
                                    v.update(cx, |view, cx| view.reset_scroll(cx));
                                }
                            })
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .font_family("iconfont")
                                    .text_color(if fav_active {
                                        theme.fav_color
                                    } else {
                                        theme.text_2
                                    })
                                    .child("\u{e630}"),
                            ),
                    ),
            )
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
                            let indicator_target: f32 = if active { 1.0 } else { 0.0 };
                            let indicator_transition = window
                                .use_keyed_transition(
                                    ("quick-tag-indicator", tag_id as u64),
                                    cx,
                                    TAG_INDICATOR_ANIM_DURATION,
                                    move |_, _| indicator_target,
                                )
                                .with_easing(quick_tag_indicator_ease);
                            indicator_transition.update(cx, |value, cx| {
                                *value = indicator_target;
                                cx.notify();
                            });
                            let indicator_progress = (*indicator_transition.evaluate(window, cx))
                                .clamp(0.0_f32, 1.0_f32);
                            let indicator_offset = (1.0_f32 - indicator_progress) / 2.0_f32;
                            let app_state = self.state.clone();
                            let tag_name = name.clone();
                            let tag_name_for_tip = name.clone();
                            div()
                                .id(("quick-tag", tag_id as u64))
                                .h(px(20.0))
                                .px(px(6.0))
                                .rounded(px(4.0))
                                .relative()
                                .flex()
                                .items_center()
                                .when(tag_compact, |d| d.flex_1().min_w(px(0.0)))
                                .when(!tag_compact, |d| d.max_w(px(120.0)))
                                .overflow_hidden()
                                .text_size(px(10.0))
                                .font_weight(FontWeight::MEDIUM)
                                .bg(if active {
                                    theme.accent_overlay()
                                } else {
                                    tag_color
                                })
                                .text_color(if active { tag_color } else { rgb(0xffffff) })
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
                                .when(indicator_progress > 0.01, |d| {
                                    d.child(
                                        div()
                                            .absolute()
                                            .left(relative(indicator_offset))
                                            .bottom(px(0.0))
                                            .w(relative(indicator_progress))
                                            .h(px(4.0))
                                            .rounded_b(px(4.0))
                                            .bg(theme.accent),
                                    )
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
            // ── List viewport ──
            // Outer container has overflow_hidden + rounded(10px) so all
            // corners are clipped uniformly — no separate inset radius needed.
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .mx(px(LIST_INSET))
                    .pt(px(LIST_INSET))
                    .flex()
                    .flex_col()
                    .pb(px(LIST_INSET))
                    .on_mouse_move({
                        let ve_clear = view_entity.clone();
                        move |_ev, _window, cx| {
                            ve_clear.update(cx, |view, cx| {
                                if view.hovered_index.is_some() {
                                    view.hovered_index = None;
                                    cx.notify();
                                }
                            });
                        }
                    })
                    .when(items_count == 0, |list| {
                        list.child(
                            div()
                                .flex_1()
                                .min_h(px(0.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(13.0))
                                .text_color(theme.text_2)
                                .child("No clipboard items"),
                        )
                    })
                    .children({
                        let t = theme.clone();
                        let selected_index = self.selected_index;
                        let first_visible = self.first_visible;
                        let view_entity = cx.entity();
                        self.row_data(cx)
                            .into_iter()
                            .map(
                                move |(
                                    slot,
                                    item_id,
                                    icon,
                                    color_swatch,
                                    preview,
                                    preview_subtitle,
                                    note,
                                    time,
                                    img_path,
                                    favicon_path,
                                    file_icon_path,
                                    path_color,
                                    styled_first_line,
                                )| {
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
                                    } else if let Some(subtitle) = preview_subtitle {
                                        // Rich label + dimmed subtitle (URL / Path / File)
                                        let label_color = path_color.unwrap_or(t.text_1);
                                        div()
                                            .flex_1()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .overflow_hidden()
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .text_color(label_color)
                                                    .whitespace_nowrap()
                                                    .child(preview),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .text_color(t.text_3)
                                                    .whitespace_nowrap()
                                                    .child(" - "),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .text_color(t.text_3)
                                                    .whitespace_nowrap()
                                                    .text_ellipsis()
                                                    .child(subtitle),
                                            )
                                            .into_any_element()
                                    } else if let Some(spans) = styled_first_line {
                                        // Rich text with inline colours — render styled spans inline
                                        div()
                                            .flex_1()
                                            .overflow_hidden()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .whitespace_nowrap()
                                            .children(spans.into_iter().map(|span| {
                                                let mut d = div()
                                                    .text_size(px(13.0))
                                                    .text_color(span.color.unwrap_or(t.text_1))
                                                    .font_weight(
                                                        span.font_weight.unwrap_or_default(),
                                                    );
                                                if span.font_style == Some(FontStyle::Italic) {
                                                    d = d.italic();
                                                }
                                                if let Some(bg) = span.background_color {
                                                    d = d.text_bg(bg);
                                                }
                                                d.child(span.text)
                                            }))
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
                                        .flex_shrink_0()
                                        .px(px(HORIZONTAL_PADDING - LIST_INSET))
                                        .rounded(px(6.0))
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .bg(if selected {
                                            t.accent_overlay()
                                        } else {
                                            rgba(0x00000000)
                                        })
                                        .on_mouse_move({
                                            let ve_hover = ve.clone();
                                            move |_ev, _window, cx| {
                                                cx.stop_propagation();
                                                ve_hover.update(cx, |view, cx| {
                                                    if view.hovered_index != Some(index) {
                                                        view.hovered_index = Some(index);
                                                        view.selected_index = index;
                                                        view.ensure_selected_visible();
                                                        cx.notify();
                                                    }
                                                });
                                            }
                                        })
                                        .cursor(CursorStyle::PointingHand)
                                        .on_mouse_down(MouseButton::Left, {
                                            let ve2 = ve.clone();
                                            let ve3 = ve.clone();
                                            move |ev, _window, cx| {
                                                // Single-click: select + paste
                                                if ev.click_count == 1 {
                                                    ve2.update(cx, |view, cx| {
                                                        view.select_index(index, cx);
                                                    });
                                                    if ev.modifiers.shift {
                                                        ve3.update(cx, |_, cx| {
                                                            cx.emit(QuickPasteEvent::PastePlain(
                                                                item_id,
                                                            ));
                                                        });
                                                    } else if advanced_modifier_pressed(
                                                        ev.modifiers,
                                                    ) {
                                                        let mode = ve2.update(cx, |view, cx| {
                                                            view.ensure_alt_modes_for_selection(cx)
                                                                .then(|| {
                                                                    view.current_alt_mode.clone()
                                                                })
                                                        });
                                                        if let Some(mode) = mode {
                                                            ve3.update(cx, |_, cx| {
                                                                cx.emit(QuickPasteEvent::PasteAlt(
                                                                    item_id, mode,
                                                                ));
                                                            });
                                                        } else {
                                                            ve3.update(cx, |_, cx| {
                                                                cx.emit(QuickPasteEvent::Paste(
                                                                    item_id,
                                                                ));
                                                            });
                                                        }
                                                    } else {
                                                        ve3.update(cx, |_, cx| {
                                                            cx.emit(QuickPasteEvent::Paste(
                                                                item_id,
                                                            ));
                                                        });
                                                    }
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
                                        // Type icon / color swatch
                                        .child(if let Some(swatch) = color_swatch {
                                            let swatch_border = color_border_for_swatch(swatch);
                                            div()
                                                .w(px(16.0))
                                                .h(px(16.0))
                                                .rounded(px(3.0))
                                                .bg(swatch)
                                                .border(px(1.0))
                                                .border_color(swatch_border)
                                                .into_any_element()
                                        } else if let Some(ref fav_path) = favicon_path {
                                            // Favicon image for link-type items
                                            gpui::img(std::path::Path::new(fav_path))
                                                .w(px(16.0))
                                                .h(px(16.0))
                                                .rounded(px(3.0))
                                                .into_any_element()
                                        } else if let Some(ref ficon_path) = file_icon_path {
                                            // File extension icon
                                            gpui::img(std::path::Path::new(ficon_path))
                                                .w(px(16.0))
                                                .h(px(16.0))
                                                .rounded(px(3.0))
                                                .into_any_element()
                                        } else {
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
                                                .child(icon)
                                                .into_any_element()
                                        })
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
            // ── Floating tooltip (advanced modifier held only) ──
            .when(self.ctrl_held, |parent| {
                let items = self.tooltip_items();
                if items.is_empty() {
                    return parent;
                }
                let tooltip_y = self.tooltip_position(has_tag_row, has_type_bar, cx);
                let Some(tip_y) = tooltip_y else {
                    return parent;
                };
                let t = theme.clone();
                let selected_item_id = {
                    self.state
                        .read(cx)
                        .items
                        .get(self.selected_index)
                        .map(|item| item.id)
                };
                let view_entity = cx.entity();
                parent.child(
                    div()
                        .absolute()
                        .top(tip_y)
                        .right(px(8.0))
                        .bg(t.surface)
                        .border(px(1.0))
                        .border_color(t.divider)
                        .rounded(px(6.0))
                        .px(px(TOOLTIP_PADDING))
                        .py(px(TOOLTIP_PADDING))
                        .flex()
                        .flex_col()
                        .children(items.iter().map(move |(label, mode, selected)| {
                            let mode = mode.clone();
                            let selected = *selected;
                            let item_id = selected_item_id;
                            let ve = view_entity.clone();
                            div()
                                .h(px(TOOLTIP_ITEM_HEIGHT))
                                .px(px(8.0))
                                .rounded(px(4.0))
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .bg(if selected {
                                    t.accent_overlay()
                                } else {
                                    rgba(0x00000000)
                                })
                                .text_color(if selected { t.accent } else { t.text_1 })
                                .text_size(px(12.0))
                                .cursor(CursorStyle::PointingHand)
                                .on_mouse_down(MouseButton::Left, {
                                    let m = mode.clone();
                                    let id = item_id;
                                    move |_ev, _window, cx| {
                                        let id = id.unwrap_or(0);
                                        ve.update(cx, |_, cx| {
                                            cx.emit(QuickPasteEvent::PasteAlt(id, m.clone()));
                                        });
                                    }
                                })
                                .child(
                                    div()
                                        .w(px(14.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(if selected { "●" } else { "" }),
                                )
                                .child(*label)
                        })),
                )
            })
            // ── Bottom hint bar ──
            .child(
                div()
                    .h(px(HINT_BAR_HEIGHT))
                    .w_full()
                    .px(px(HORIZONTAL_PADDING))
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .text_size(px(10.0))
                    .text_color(theme.text_3)
                    .border_t(px(1.0))
                    .border_color(theme.divider)
                    .child(
                        div()
                            .font_family("iconfont")
                            .text_size(px(12.0))
                            .child("\u{e66b}"),
                    )
                    .child(div().child("Enter 粘贴"))
                    .child(
                        div()
                            .when(self.shift_held, |s| {
                                s.text_size(px(10.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.accent)
                            })
                            .child(if self.shift_held {
                                "Shift 纯文本粘贴"
                            } else {
                                "Shift 纯文本"
                            }),
                    )
                    .child(
                        div()
                            .when(self.ctrl_held, |s| {
                                s.text_size(px(10.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.accent)
                            })
                            .child(if self.ctrl_held {
                                format!("{} 高级粘贴", advanced_modifier_label())
                            } else {
                                format!("{} 高级", advanced_modifier_label())
                            }),
                    ),
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
        DisplayKind::PlainText => "\u{e606}",
        DisplayKind::Html | DisplayKind::Markdown | DisplayKind::Rtf => "\u{e853}",
        DisplayKind::Image => "\u{e626}",
        DisplayKind::File => "\u{e68a}",
        DisplayKind::Link => "\u{e6d7}",
        DisplayKind::Path => "\u{e60f}",
        DisplayKind::Color => "\u{e608}",
        DisplayKind::Email => "\u{e604}",
        DisplayKind::Phone => "\u{e966}",
        DisplayKind::Secret => "\u{e612}",
    }
}

/// Split preview into (label, optional_subtitle) for rich display.
/// URL: (site/domain, page title or path)  Path: (leaf, full path)
/// Extract the first line of styled HTML for quick-window preview.
/// Returns `None` when the item has no colour-inline HTML → falls back to plain text.
fn styled_preview(item: &ClipboardItem) -> Option<Vec<rich_preview::StyledHtmlSpan>> {
    let rich = RichData::from_json(&item.rich_data);
    let html = rich.html.as_deref()?;
    if html.trim().is_empty() {
        return None;
    }
    let html = rich_preview::normalize_clipboard_html_for_render(html);
    let lines = rich_preview::parse_styled_html_lines(&html)?;
    lines.into_iter().next()
}

fn preview_parts(item: &ClipboardItem, auto_fetch_title: bool) -> (String, Option<String>) {
    let remote_host = RichData::from_json(&item.rich_data).remote_host;
    let raw = match item.content_type {
        ContentType::Image => {
            if remote_host.is_some() {
                let path = item.image_path.clone();
                let name = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("Image")
                    .to_string();
                return (name, Some(path));
            }
            let label = if item.image_width > 0 && item.image_height > 0 {
                format!("Image {}×{}", item.image_width, item.image_height)
            } else {
                "Image".to_string()
            };
            return (label, None);
        }
        ContentType::File => {
            let data = FileData::from_json(&item.file_data);
            if data.files.is_empty() {
                return (item.full_text.clone(), None);
            }
            let first_name = data.files[0].name.clone();
            if data.files.len() == 1 {
                let subtitle = remote_host.map(|_| data.files[0].path.clone());
                return (first_name, subtitle);
            }
            // Multiple files: first name as label, "等N个文件" as faded subtitle
            let count_label = crate::core::i18n_keys::I18nKey::QuickFileCount
                .fmt(&[&data.files.len().to_string()]);
            return (first_name, Some(count_label));
        }
        _ => {
            // ── URL: site name + page title (or domain + path fallback) ──
            if item.meta_type == "link" {
                let masked_url = sensitive_preview_to_text(&item.full_text, "link");
                let domain = url_domain(&item.full_text);
                let path = url_path(&masked_url);
                if auto_fetch_title {
                    let rd = RichData::from_json(&item.rich_data);
                    if let Some(title) = rd.page_title {
                        let site = url_site_name(&item.full_text);
                        return (site, Some(title));
                    }
                }
                // Fallback: domain (label) + path (subtitle)
                if !domain.is_empty() && !path.is_empty() && path != "/" {
                    return (domain, Some(path));
                }
                return (masked_url, None);
            }
            // ── Path: leaf name (label) + full path (subtitle) ──
            if item.meta_type == "path" {
                let path_text = item.full_text.trim_end_matches(['\\', '/']);
                if let Some(pos) = path_text.rfind(['\\', '/']) {
                    if pos + 1 < path_text.len() {
                        return (
                            path_text[pos + 1..].to_string(),
                            Some(item.full_text.clone()),
                        );
                    }
                }
                return (item.full_text.clone(), None);
            }
            item.full_text.clone()
        }
    };
    // Normalize whitespace in a single pass (avoid intermediate Vec allocation)
    let raw: String = {
        let mut s = String::with_capacity(raw.len());
        let mut first = true;
        for token in raw.split_whitespace() {
            if !first {
                s.push(' ');
            }
            s.push_str(token);
            first = false;
        }
        s
    };
    // Mask sensitive content (email / phone / secret) in preview
    let masked = sensitive_preview_to_text(&raw, &item.meta_type);
    (masked, None)
}

fn parse_hex_for_tag(hex: &str) -> Rgba {
    use crate::core::types::parse_hex_color;
    parse_hex_color(hex)
        .map(|(r, g, b)| rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32))
        .unwrap_or(rgb(0x3b82f6))
}

/// Pick a border color that contrasts with the swatch's perceived brightness.
/// Light colors get a dark border, dark colors get a light border — so the
/// swatch is always visible regardless of theme or color value.
fn color_border_for_swatch(swatch: Rgba) -> Rgba {
    // ITU-R BT.601 perceived brightness (gpui Rgba uses 0.0–1.0 floats)
    let lum = 0.299 * swatch.r + 0.587 * swatch.g + 0.114 * swatch.b;
    if lum > 0.55 {
        // Light swatch → dark semi-transparent border
        rgba(0x00000030)
    } else {
        // Dark swatch → light semi-transparent border
        rgba(0xffffff30)
    }
}
