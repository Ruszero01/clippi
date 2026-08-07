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
use crate::ui::clipboard_card::image_preview_path;
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

/// 按快速窗口规则判定单个条目是否可用。
///
/// 判定规则与快速窗口现有语义一致：
/// - 非图片/文件条目直接保留，不解析 `RichData`；
/// - 带 `remote_host` 的远程图片/文件不做本地路径存在性检查；
/// - 本地图片在 `image_path` 非空时检查路径存在性；
/// - 本地单文件条目检查源路径存在性，多文件条目保留；
/// - 空 `image_path`、空/无法解析的 `file_data` 保留。
///
/// `path_exists` 为依赖注入：生产代码传 `std::path::Path::exists`，
/// 单元测试传可控闭包，使核心逻辑不依赖真实文件系统。
fn is_quick_item_available<F>(item: &ClipboardItem, path_exists: F) -> bool
where
    F: Fn(&std::path::Path) -> bool,
{
    let is_image = item.content_type == ContentType::Image;
    let is_file = item.content_type == ContentType::File;
    if !is_image && !is_file {
        return true;
    }
    // 仅图片/文件条目解析一次 RichData 判断远程来源。
    let is_remote = RichData::from_json(&item.rich_data).remote_host.is_some();
    if is_remote {
        return true;
    }
    if is_image {
        if item.image_path.is_empty() {
            return true;
        }
        return path_exists(std::path::Path::new(&item.image_path));
    }
    let fd = FileData::from_json(&item.file_data);
    // 只过滤单文件条目；多文件条目保持现状。
    if fd.files.len() == 1 {
        if let Some(fi) = fd.files.first() {
            return path_exists(std::path::Path::new(&fi.path));
        }
    }
    true
}

/// 按快速窗口规则返回可用条目在 `items` 中的源下标（保持原始顺序）。
fn collect_quick_item_indices<F>(items: &[ClipboardItem], path_exists: F) -> Vec<usize>
where
    F: Fn(&std::path::Path) -> bool,
{
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| is_quick_item_available(item, &path_exists))
        .map(|(index, _)| index)
        .collect()
}

/// 复验目标条目 ID 在 `items` 中仍存在且按快速窗口规则可用。
/// 粘贴入口在发送事件前调用：目标失效时由调用方刷新画面而非粘贴。
fn quick_item_available_by_id<F>(items: &[ClipboardItem], item_id: i64, path_exists: F) -> bool
where
    F: Fn(&std::path::Path) -> bool,
{
    items
        .iter()
        .find(|item| item.id == item_id)
        .map(|item| is_quick_item_available(item, path_exists))
        .unwrap_or(false)
}

/// 将选中/视口位置归一化到当前可用序列内，返回 `(selected_index, first_visible)`。
///
/// - 可用数量为 0 时返回 `(0, 0)`；
/// - 选中下标越界时收缩到最后一个有效条目；
/// - 视口起点越界时收缩到最后一个有效页面起点；
/// - 最后调整视口起点，保证选中项处于 `[first_visible, first_visible + VISIBLE_ROWS)`。
fn normalize_quick_selection(
    selected_index: usize,
    first_visible: usize,
    available_count: usize,
) -> (usize, usize) {
    if available_count == 0 {
        return (0, 0);
    }
    let selected = selected_index.min(available_count - 1);
    let last_page_start = ((available_count - 1) / VISIBLE_ROWS) * VISIBLE_ROWS;
    let mut first = first_visible.min(last_page_start);
    if selected < first {
        first = selected;
    } else if selected >= first + VISIBLE_ROWS {
        first = selected + 1 - VISIBLE_ROWS;
    }
    (selected, first)
}

/// 从画面快照中解析选中条目 ID。快照元素为 `(源下标, 稳定 ID)`，交互一律用 ID：
/// `state.items` 在渲染后重载/重排时，源下标会指向其他条目，而 ID 始终指向同一条目。
fn snapshot_selected_id(snapshot: &[(usize, i64)], selected_index: usize) -> Option<i64> {
    snapshot.get(selected_index).map(|&(_, id)| id)
}

/// 计算条目支持的高级粘贴模式，返回 `(模式列表, 当前模式下标, 当前模式)`。
/// 无高级模式的条目（文本、链接等）返回 `None`。
/// 提取为纯函数使模式计算可单测；默认图片模式由设置注入。
fn compute_alt_modes(
    item: &ClipboardItem,
    image_alt_mode: &str,
) -> Option<(Vec<String>, usize, String)> {
    let is_image = item.content_type == ContentType::Image;
    let is_file = item.content_type == ContentType::File && !item.file_data.is_empty();
    let is_color = item.meta_type == "color";
    if is_image {
        let modes = vec!["bitmap".to_string(), "path".to_string(), "ocr".to_string()];
        let index = modes.iter().position(|m| m == image_alt_mode).unwrap_or(0);
        let mode = modes[index].clone();
        Some((modes, index, mode))
    } else if is_file {
        Some((vec!["file_path".to_string()], 0, "file_path".to_string()))
    } else if is_color {
        let current_is_hex = detect_color(&item.full_text)
            .map(|_c| {
                item.full_text.starts_with('#')
                    || (item.full_text.len() == 6
                        && item.full_text.chars().all(|ch| ch.is_ascii_hexdigit()))
            })
            .unwrap_or(false);
        if current_is_hex {
            Some((vec!["rgb".to_string()], 0, "rgb".to_string()))
        } else {
            Some((vec!["hex".to_string()], 0, "hex".to_string()))
        }
    } else {
        None
    }
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

/// (slot, id, icon, color_swatch, preview_text, preview_subtitle, note, relative_time, preview_img_path, favicon_path, file_icon_path, path_color, styled_first_line)
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
    /// 当前画面快照：最近一次 render 构建的过滤后 `(源下标, 稳定 ID)` 序列。
    /// 渲染用源下标取数据，交互用稳定 ID 解析条目身份——`state.items` 重载/重排后
    /// 源下标会指向其他条目，而 ID 始终指向同一条目。每次 render 重建，不长期缓存。
    display_indices: Vec<(usize, i64)>,
    /// 最近一次计算高级粘贴模式时对应的条目 ID。
    /// render 归一化改变选中身份后据此决定是否刷新浮层模式。
    alt_mode_item_id: Option<i64>,
    /// Per-view image cache so thumbnails/favicons/file icons are released
    /// when the popup hides instead of living forever in the global asset cache.
    image_cache: Entity<RetainAllImageCache>,
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
            display_indices: Vec::new(),
            alt_mode_item_id: None,
            image_cache: RetainAllImageCache::new(cx),
            _appearance_subscription: appearance_subscription,
            shift_held: false,
            ctrl_held: false,
            current_alt_index: 0,
            current_alt_mode: image_alt_mode,
            available_alt_modes: Vec::new(),
            scroll_accumulator: 0.0,
        }
    }

    /// 基于当前画面快照归一化选中/视口位置，返回快照长度。
    /// 快照为空时清理 hover/高级模式并重绘，让画面立即进入空状态。
    fn normalize_with_display_snapshot(&mut self, cx: &mut Context<Self>) -> usize {
        let len = self.display_indices.len();
        let (selected, first) =
            normalize_quick_selection(self.selected_index, self.first_visible, len);
        self.selected_index = selected;
        self.first_visible = first;
        if len == 0 {
            self.hovered_index = None;
            self.available_alt_modes.clear();
            self.current_alt_index = 0;
            cx.notify();
        }
        len
    }

    /// 重建画面快照并归一化选中位置，触发重绘。
    /// 用于目标条目失效后的兜底刷新（本次不执行粘贴）。
    fn refresh_display_snapshot(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        self.display_indices = collect_quick_item_indices(&state.items, std::path::Path::exists)
            .into_iter()
            .map(|source_index| (source_index, state.items[source_index].id))
            .collect();
        self.normalize_with_display_snapshot(cx);
        cx.notify();
    }

    /// 复验目标条目 ID 在当前状态下仍可用（判定规则与画面快照一致）。
    fn verify_paste_ready(&self, item_id: i64, cx: &Context<Self>) -> bool {
        quick_item_available_by_id(&self.state.read(cx).items, item_id, std::path::Path::exists)
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        let len = self.normalize_with_display_snapshot(cx);
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
        let len = self.normalize_with_display_snapshot(cx);
        if len == 0 {
            return;
        }
        self.selected_index = self.selected_index.saturating_sub(1);
        self.ensure_selected_visible();
        cx.notify();
    }

    pub fn select_next_page(&mut self, cx: &mut Context<Self>) {
        let len = self.normalize_with_display_snapshot(cx);
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
        let len = self.normalize_with_display_snapshot(cx);
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
        let len = self.normalize_with_display_snapshot(cx);
        if len == 0 {
            return None;
        }
        let index = self.first_visible + slot;
        if index >= len {
            return None;
        }
        self.selected_index = index;
        self.ensure_selected_visible();
        cx.notify();
        self.selected_item_id(cx)
    }

    /// Select a specific filtered logical index (for mouse click).
    pub fn select_index(&mut self, index: usize, cx: &mut Context<Self>) {
        let len = self.normalize_with_display_snapshot(cx);
        if len == 0 {
            return;
        }
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

    /// 基于当前画面快照解析选中条目 ID 并复验可用性。
    /// 快照元素为 `(源下标, 稳定 ID)`，交互一律用 ID 解析条目身份，
    /// `state.items` 重载/重排后仍指向同一条目；目标条目已失效时重建快照
    /// 并重绘，返回 `None`，调用方不粘贴失效项。
    pub fn selected_item_id(&mut self, cx: &mut Context<Self>) -> Option<i64> {
        let id = snapshot_selected_id(&self.display_indices, self.selected_index)?;
        if self.verify_paste_ready(id, cx) {
            Some(id)
        } else {
            self.refresh_display_snapshot(cx);
            None
        }
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
        // 清空画面快照：新 render 之前拒绝使用上一次窗口会话的快照
        //（窗口打开时 `state.items` 会先重载，旧快照的源下标已失效）。
        self.display_indices.clear();
        self.alt_mode_item_id = None;
        cx.notify();
    }

    /// Replace the image cache so decoded thumbnails/favicons/file icons are
    /// released. Called when the popup hides or the app goes to background.
    pub(crate) fn release_images_for_hide(&mut self, cx: &mut Context<Self>) {
        self.image_cache = RetainAllImageCache::new(cx);
        cx.notify();
    }

    /// Redraw after an async thumbnail finished generating. The main list is
    /// refreshed via `ClipboardChanged`; the popup needs an explicit notify.
    pub(crate) fn notify_thumbnail_ready(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    fn row_data(&self, cx: &Context<Self>, available_indices: &[(usize, i64)]) -> Vec<RowData> {
        let settings = &self.state.read(cx).settings;
        let auto_fetch_title = settings.auto_fetch_url_title;
        let items = &self.state.read(cx).items;

        // 先过滤再分页：`slot` 始终是当前视口内的 0..4，数字徽标显示 slot + 1。
        available_indices
            .iter()
            .skip(self.first_visible)
            .take(VISIBLE_ROWS)
            .enumerate()
            .filter_map(|(slot, &(source_index, _))| {
                items.get(source_index).map(|item| (slot, item))
            })
            .map(|(slot, item)| {
                let remote_host = RichData::from_json(&item.rich_data).remote_host;
                let is_image =
                    item.content_type == ContentType::Image && !item.image_path.is_empty();
                // Use the shared 310px thumbnail when available; otherwise the
                // async thumbnail job is started and a placeholder is shown.
                // Never fall back to the full-size original.
                let preview_img_path = if is_image && remote_host.is_none() {
                    image_preview_path(item).map(|p| p.to_string_lossy().to_string())
                } else {
                    None
                };
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
                    preview_img_path,
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
    /// selected item's type. Only images, files and colors have advanced modes.
    fn update_alt_modes_for_selection(&mut self, cx: &Context<Self>) {
        self.available_alt_modes.clear();
        self.current_alt_index = 0;
        self.alt_mode_item_id = None;

        let state = self.state.read(cx);
        // 基于画面快照解析选中条目 ID，与 selected_item_id 同源；
        // 下标按当次快照钳制，窗口打开期间选中项失效时回退到最后一个有效条目。
        let Some(&(_, item_id)) = self.display_indices.get(
            self.selected_index
                .min(self.display_indices.len().saturating_sub(1)),
        ) else {
            return;
        };
        // 按稳定 ID 在当前 items 中查找：列表重载/重排后源下标会指向其他条目。
        let Some(item) = state.items.iter().find(|it| it.id == item_id) else {
            return;
        };
        let Some((modes, index, mode)) = compute_alt_modes(item, &state.settings.image_alt_mode)
        else {
            return;
        };
        self.available_alt_modes = modes;
        self.current_alt_index = index;
        self.current_alt_mode = mode;
        self.alt_mode_item_id = Some(item_id);
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
        available_indices: &[(usize, i64)],
    ) -> Option<Pixels> {
        if available_indices.is_empty() {
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

        // ── 过滤后有效 `(源下标, 稳定 ID)` 快照 ──
        // 当次渲染的显示数量、空状态、行数据、选中态与高级粘贴浮层统一复用，
        // 同时存入 `display_indices` 供交互入口解析条目身份，避免同一帧内重复扫描。
        {
            let state = self.state.read(cx);
            self.display_indices =
                collect_quick_item_indices(&state.items, std::path::Path::exists)
                    .into_iter()
                    .map(|source_index| (source_index, state.items[source_index].id))
                    .collect();
        }
        // 剪贴板刷新、排序变化或源文件失效后，把选中/视口位置收缩回有效序列内。
        let (selected, first) = normalize_quick_selection(
            self.selected_index,
            self.first_visible,
            self.display_indices.len(),
        );
        self.selected_index = selected;
        self.first_visible = first;
        if self
            .hovered_index
            .is_some_and(|h| h >= self.display_indices.len())
        {
            self.hovered_index = None;
        }
        // 归一化可能改变选中条目身份：按住高级键且浮层模式对应的条目已变化时，
        // 重新计算高级模式，避免浮层继续显示旧条目的模式（如图片模式配文本条目）。
        if self.ctrl_held {
            let current_id = snapshot_selected_id(&self.display_indices, self.selected_index);
            if current_id != self.alt_mode_item_id {
                self.update_alt_modes_for_selection(cx);
            }
        }

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
                self.display_indices.len(),
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
                        let image_cache = self.image_cache.clone();
                        let view_entity = cx.entity();
                        self.row_data(cx, &self.display_indices)
                            .into_iter()
                            .map(
                                move |(
                                    slot,
                                    _item_id,
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
                                    // 过滤后逻辑下标：选中态、hover 与鼠标交互共用同一坐标系。
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
                                                    .image_cache(&image_cache)
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
                                                // Single-click: select + paste。
                                                // 不直接发送渲染时捕获的 item_id：由视图基于
                                                // 当前画面快照重新解析 ID 并复验，避免源文件在
                                                // 渲染后失效时仍粘贴已失效条目。
                                                if ev.click_count == 1 {
                                                    let paste_id = ve2.update(cx, |view, cx| {
                                                        view.select_index(index, cx);
                                                        view.selected_item_id(cx)
                                                    });
                                                    let Some(id) = paste_id else {
                                                        // 目标已失效：画面已刷新，不发送事件。
                                                        return;
                                                    };
                                                    if ev.modifiers.shift {
                                                        ve3.update(cx, |_, cx| {
                                                            cx.emit(QuickPasteEvent::PastePlain(
                                                                id,
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
                                                                    id, mode,
                                                                ));
                                                            });
                                                        } else {
                                                            ve3.update(cx, |_, cx| {
                                                                cx.emit(QuickPasteEvent::Paste(id));
                                                            });
                                                        }
                                                    } else {
                                                        ve3.update(cx, |_, cx| {
                                                            cx.emit(QuickPasteEvent::Paste(id));
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
                                                .image_cache(&image_cache)
                                                .w(px(16.0))
                                                .h(px(16.0))
                                                .rounded(px(3.0))
                                                .into_any_element()
                                        } else if let Some(ref ficon_path) = file_icon_path {
                                            // File extension icon
                                            gpui::img(std::path::Path::new(ficon_path))
                                                .image_cache(&image_cache)
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
                let tooltip_y =
                    self.tooltip_position(has_tag_row, has_type_bar, &self.display_indices);
                let Some(tip_y) = tooltip_y else {
                    return parent;
                };
                let t = theme.clone();
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
                                    let ve2 = ve.clone();
                                    let ve3 = ve.clone();
                                    move |_ev, _window, cx| {
                                        // 点击时经视图基于画面快照解析选中 ID 并复验，
                                        // 再以目标条目重算高级模式并校验本次点击的模式：
                                        // 若选中身份在渲染后变化（如图片失效改选文本），
                                        // 模式不匹配则取消动作并刷新画面。
                                        let result = ve2.update(cx, |view, cx| {
                                            let id = view.selected_item_id(cx)?;
                                            if view.ensure_alt_modes_for_selection(cx)
                                                && view.available_alt_modes.contains(&m)
                                            {
                                                Some((id, m.clone()))
                                            } else {
                                                None
                                            }
                                        });
                                        if let Some((id, mode)) = result {
                                            ve3.update(cx, |_, cx| {
                                                cx.emit(QuickPasteEvent::PasteAlt(id, mode));
                                            });
                                        }
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

#[cfg(test)]
mod tests {
    // 注意：不使用 `use super::*`——quick_paste.rs 顶层 `use gpui::*` 会带入
    // gpui 的 `test` 属性宏，与 `#[test]` 冲突导致递归展开错误。
    use super::{
        collect_quick_item_indices, compute_alt_modes, is_quick_item_available,
        normalize_quick_selection, quick_item_available_by_id, snapshot_selected_id, VISIBLE_ROWS,
    };
    use crate::core::types::FileInfo;
    use crate::core::types::{ClipboardItem, ContentType, FileData, RichData};
    use std::path::Path;

    fn make_item(
        id: i64,
        content_type: ContentType,
        image_path: &str,
        file_data: &str,
        rich_data: &str,
    ) -> ClipboardItem {
        ClipboardItem {
            id,
            content_type,
            full_text: format!("item {id}"),
            content_hash: 0x100 + id as u64,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            image_path: image_path.to_string(),
            image_width: 0,
            image_height: 0,
            rich_data: rich_data.to_string(),
            file_data: file_data.to_string(),
            is_favorite: false,
            note: String::new(),
            source_app_name: String::new(),
            source_app_icon: String::new(),
            size: 0,
            tags: Vec::new(),
            meta_type: String::new(),
            custom_hotkey: String::new(),
            custom_hotkey_format: String::new(),
            existence_observed_at: String::new(),
        }
    }

    fn text_item(id: i64) -> ClipboardItem {
        make_item(id, ContentType::PlainText, "", "", "")
    }

    fn local_image(id: i64, path: &str) -> ClipboardItem {
        make_item(id, ContentType::Image, path, "", "")
    }

    fn remote_image(id: i64, path: &str) -> ClipboardItem {
        let rich = RichData {
            remote_host: Some("NAS01".to_string()),
            ..Default::default()
        };
        make_item(id, ContentType::Image, path, "", &rich.to_json())
    }

    fn single_file(id: i64, path: &str) -> ClipboardItem {
        let fd = FileData {
            files: vec![FileInfo {
                name: "file".to_string(),
                path: path.to_string(),
                is_dir: false,
            }],
            ..Default::default()
        };
        make_item(id, ContentType::File, "", &fd.to_json(), "")
    }

    fn multi_file(id: i64, paths: &[&str]) -> ClipboardItem {
        let fd = FileData {
            files: paths
                .iter()
                .map(|p| FileInfo {
                    name: "file".to_string(),
                    path: p.to_string(),
                    is_dir: false,
                })
                .collect(),
            ..Default::default()
        };
        make_item(id, ContentType::File, "", &fd.to_json(), "")
    }

    /// 模拟行数据管道的页内槽位：(slot, item_id)。
    fn page_slots(
        indices: &[usize],
        items: &[ClipboardItem],
        first_visible: usize,
    ) -> Vec<(usize, i64)> {
        indices
            .iter()
            .skip(first_visible)
            .take(VISIBLE_ROWS)
            .enumerate()
            .filter_map(|(slot, &source_index)| items.get(source_index).map(|it| (slot, it.id)))
            .collect()
    }

    #[test]
    fn test_filter_before_pagination_compacts_first_page() {
        let items = vec![
            text_item(1),
            text_item(2),
            local_image(3, "/missing/3.png"),
            single_file(4, "/missing/4.txt"),
            text_item(5),
            text_item(6),
            text_item(7),
        ];
        let indices = collect_quick_item_indices(&items, |_| false);
        let ids: Vec<i64> = indices.iter().map(|&i| items[i].id).collect();
        assert_eq!(ids, vec![1, 2, 5, 6, 7]);
        // 第一页槽位从 0 重新编号，无空槽。
        assert_eq!(
            page_slots(&indices, &items, 0),
            vec![(0, 1), (1, 2), (2, 5), (3, 6), (4, 7)]
        );
    }

    #[test]
    fn test_compacted_sequence_paginates_without_gaps() {
        // 原始 8 条中 2、4 无效 → 6 条有效。
        let items = vec![
            text_item(1),
            local_image(2, "/missing/2.png"),
            text_item(3),
            single_file(4, "/missing/4.txt"),
            text_item(5),
            text_item(6),
            text_item(7),
            text_item(8),
        ];
        let indices = collect_quick_item_indices(&items, |_| false);
        let ids: Vec<i64> = indices.iter().map(|&i| items[i].id).collect();
        assert_eq!(ids, vec![1, 3, 5, 6, 7, 8]);
        // 第二页（first_visible = 5）从第 6 个有效项开始，无重复、无跳过。
        assert_eq!(page_slots(&indices, &items, 5), vec![(0, 8)]);
    }

    #[test]
    fn test_visible_slot_maps_to_compacted_item() {
        let items = vec![
            text_item(1),
            text_item(2),
            local_image(3, "/missing/3.png"),
            single_file(4, "/missing/4.txt"),
            text_item(5),
            text_item(6),
            text_item(7),
        ];
        let indices = collect_quick_item_indices(&items, |_| false);
        // 数字键 3 → 视口内槽位 2（0 基）→ 过滤后第 3 项 → 原始 ID 5。
        let source = indices[2];
        assert_eq!(items[source].id, 5);
    }

    #[test]
    fn test_all_invalid_items_produce_empty_sequence() {
        let items = vec![
            local_image(1, "/missing/1.png"),
            single_file(2, "/missing/2.txt"),
        ];
        let indices = collect_quick_item_indices(&items, |_| false);
        assert!(indices.is_empty());
        // 选中 ID 解析为 None（模拟 selected_item_id 的防御性读取）。
        let selected_id = indices
            .first()
            .and_then(|&si| items.get(si))
            .map(|it| it.id);
        assert_eq!(selected_id, None);
        // 空序列时选中位置归零。
        assert_eq!(normalize_quick_selection(0, 0, indices.len()), (0, 0));
    }

    #[test]
    fn test_existing_filter_semantics_are_preserved() {
        let items = vec![
            text_item(1),
            local_image(2, "/exists/2.png"),
            local_image(3, "/missing/3.png"),
            remote_image(4, "/missing/4.png"), // 远程缺失路径 → 保留
            local_image(5, ""),                // 空 image_path → 保留
            single_file(6, "/exists/6.txt"),
            single_file(7, "/missing/7.txt"),
            multi_file(8, &["/exists/8a.txt", "/missing/8b.txt"]), // 多文件 → 保留
            make_item(9, ContentType::File, "", "not-json", ""),   // 无法解析 file_data → 保留
        ];
        let indices = collect_quick_item_indices(&items, |p| {
            p == Path::new("/exists/2.png") || p == Path::new("/exists/6.txt")
        });
        let ids: Vec<i64> = indices.iter().map(|&i| items[i].id).collect();
        assert_eq!(ids, vec![1, 2, 4, 5, 6, 8, 9]);
        // 单条目判定与收集结果一致。
        assert!(is_quick_item_available(&items[0], |_| true)); // 文本
        assert!(!is_quick_item_available(&items[2], |_| false)); // 本地缺失图片
        assert!(is_quick_item_available(&items[3], |_| true)); // 远程缺失路径豁免
        assert!(is_quick_item_available(&items[7], |_| true)); // 多文件保留
    }

    #[test]
    fn test_selected_id_comes_from_display_snapshot_not_rebuilt_sequence() {
        // P1-1 回归保护：渲染快照 [1, 2, 3]，选中下标 1（ID 2）。
        // 渲染后更靠前的 ID 1 失效。动作若重建快照，下标 1 会映射到 ID 3（错项）；
        // 修复后基于画面快照解析出 ID 2，复验仍可用 → 粘贴 ID 2（与画面一致）。
        let items = vec![local_image(1, "/exists/1.png"), text_item(2), text_item(3)];
        // 画面快照：渲染时 ID 1 有效。
        let display = collect_quick_item_indices(&items, |_| true);
        assert_eq!(display, vec![0, 1, 2]);
        // 渲染后 ID 1 的源文件失效（无重绘）。
        let items_after = vec![local_image(1, "/missing/1.png"), text_item(2), text_item(3)];
        // 旧实现行为：重建快照后下标 1 → 原始下标 2 → ID 3（错项粘贴）。
        let rebuilt = collect_quick_item_indices(&items_after, |_| false);
        assert_eq!(rebuilt[1], 2);
        assert_eq!(items_after[rebuilt[1]].id, 3);
        // 新实现行为：画面快照解析 → ID 2，复验仍可用 → 粘贴 ID 2。
        let source = display[1];
        let id = items_after[source].id;
        assert_eq!(id, 2);
        assert!(quick_item_available_by_id(&items_after, id, |_| true));
    }

    #[test]
    fn test_invalidated_selected_item_is_rejected() {
        // 选中项自身失效：复验失败，粘贴入口应拒绝而非返回后继条目。
        let items = vec![text_item(1), single_file(2, "/missing/2.txt")];
        // 画面快照（渲染时有效）。
        let display = collect_quick_item_indices(&items, |_| true);
        assert_eq!(display.len(), 2);
        let id = items[display[1]].id;
        assert_eq!(id, 2);
        // 渲染后失效：复验拒绝。
        assert!(!quick_item_available_by_id(&items, id, |_| false));
        // ID 不存在：复验拒绝。
        assert!(!quick_item_available_by_id(&items, 999, |_| true));
    }

    #[test]
    fn test_snapshot_ids_survive_items_reorder() {
        // 9.2 回归保护：快照存 `(源下标, 稳定 ID)`，交互必须用 ID 解析条目身份。
        // 渲染时 items [1, 2, 3]，快照 (0,1),(1,2),(2,3)，选中下标 1 → ID 2。
        let snapshot = vec![(0usize, 1i64), (1, 2), (2, 3)];
        assert_eq!(snapshot_selected_id(&snapshot, 1), Some(2));
        // 渲染后列表头部插入新条目：源下标 1 现在指向 ID 1，但按 ID 仍解析到原条目 2。
        let items_after = vec![text_item(9), text_item(1), text_item(2), text_item(3)];
        let id = snapshot_selected_id(&snapshot, 1).unwrap();
        assert_eq!(id, 2);
        // 按 ID 复验仍指向原条目；旧行为（源下标直接读 items）会读到 ID 1。
        assert!(quick_item_available_by_id(&items_after, id, |_| true));
        let old_read = items_after[snapshot[1].0].id;
        assert_eq!(old_read, 1);
        assert_ne!(old_read, id);
        // 越界下标返回 None。
        assert_eq!(snapshot_selected_id(&snapshot, 5), None);
    }

    #[test]
    fn test_compute_alt_modes_for_item_types() {
        // 图片：默认模式来自设置，模式列表固定。
        let image = local_image(1, "/exists/1.png");
        let (modes, index, mode) = compute_alt_modes(&image, "ocr").unwrap();
        assert_eq!(modes, vec!["bitmap", "path", "ocr"]);
        assert_eq!((index, mode.as_str()), (2, "ocr"));
        // 未知默认模式回退到第一个。
        assert_eq!(compute_alt_modes(&image, "nope").unwrap().0[0], "bitmap");
        // 单文件。
        let file = single_file(2, "/exists/2.txt");
        assert_eq!(
            compute_alt_modes(&file, "bitmap"),
            Some((vec!["file_path".to_string()], 0, "file_path".to_string()))
        );
        // 颜色：hex 文本 → rgb，非 hex → hex。
        let mut c = make_item(3, ContentType::PlainText, "", "", "");
        c.meta_type = "color".to_string();
        c.full_text = "#ff0000".to_string();
        assert_eq!(compute_alt_modes(&c, "bitmap").unwrap().0, vec!["rgb"]);
        c.full_text = "red".to_string();
        assert_eq!(compute_alt_modes(&c, "bitmap").unwrap().0, vec!["hex"]);
        // 文本等无高级模式。
        assert_eq!(compute_alt_modes(&text_item(4), "bitmap"), None);
    }

    #[test]
    fn test_selection_normalizes_after_count_shrinks() {
        // 可用数量为 0 → 归零。
        assert_eq!(normalize_quick_selection(2, 3, 0), (0, 0));
        // 选中越界 → 收缩到最后一个有效条目。
        assert_eq!(normalize_quick_selection(7, 0, 5), (4, 0));
        // 视口起点越界 → 收缩到最后一个有效页面起点。
        assert_eq!(normalize_quick_selection(6, 20, 7), (6, 5));
        // 选中在视口下方 → 视口跟随下移保证可见。
        assert_eq!(normalize_quick_selection(5, 0, 7), (5, 1));
        // 选中在视口上方 → 视口跟随上移。
        assert_eq!(normalize_quick_selection(2, 4, 7), (2, 2));
    }
}
