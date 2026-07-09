//! --- Application-wide state entity. ---
//!
//! --- `AppState` is the root entity that holds all shared application data. ---
//! It is created once at startup and passed to all UI components via GPUI's
//! --- entity subscription/observation mechanism. ---

use crate::core::db::Database;
use crate::core::filters::ClipboardFilters;
use crate::core::html_text;
use crate::core::i18n_keys::I18nKey;
use crate::core::settings::AppSettings;
use crate::core::types::next_tag_color;
use crate::core::types::ClipboardItem;
use crate::core::types::ContentType;
use crate::core::types::DisplayKind;
use crate::core::types::FileData;
use crate::core::types::RichData;
use crate::core::types::TagInfo;
use crate::services::update::{UpdateInfo, UpdatePhase};
use crate::state::sync::SyncState;
use pinyin::ToPinyin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const KEYWORD_SEARCH_PAGE_SIZE: usize = 128;
const KEYWORD_SEARCH_MIN_SCAN_LIMIT: usize = 1_000;
const KEYWORD_SEARCH_MAX_SCAN_LIMIT: usize = 10_000;
const KEYWORD_SEARCH_SCAN_MULTIPLIER: usize = 8;
const LIST_FULL_TEXT_LIMIT: usize = 8192;
const LIST_RICH_HTML_LIMIT: usize = 4096;
const LIST_RICH_AUX_LIMIT: usize = 2048;
const LIST_NOTE_LIMIT: usize = 2048;

/// Root application state entity.
///
/// Held in an `Entity<AppState>` by the root view. Child views access it
/// via `cx.read_entity()` / `cx.update_entity()` or through observation.
pub struct AppState {
    /// Persisted settings (TOML-backed)
    pub settings: AppSettings,
    /// SQLite database
    pub db: Database,
    /// Clipboard items loaded from DB
    pub items: Vec<ClipboardItem>,
    /// All tags loaded from DB
    pub tags: Vec<TagInfo>,
    /// Active filter state
    pub filters: ClipboardFilters,
    /// Currently selected item IDs (for batch operations)
    pub selected_ids: Vec<i64>,
    /// Tag editing state for TagFilterPanel overlay
    pub editing_tag_id: i64,
    pub editing_tag_name: String,
    pub editing_tag_color: String,
    /// Clipboard item editing state for the GPUI edit panel.
    pub editing_item_id: i64,
    pub editing_item: Option<ClipboardItem>,
    /// Shared with clipboard listener — set true during batch paste
    /// to prevent recording intermediate writes (newline separators).
    pub batch_pasting: Arc<AtomicBool>,
    /// Shared with clipboard listener — set true before writing OCR/color
    /// conversion text to clipboard. The listener skips one cycle and
    /// updates the baseline sequence number so the internal write is never
    /// detected as a new history entry (unlike batch_pasting which only
    /// provides a time window).
    pub skip_next: Arc<AtomicBool>,
    /// Shared with SyncManager — true when local data has changed.
    /// [FUTURE] When SyncManager is migrated to GPUI, pass this Arc to
    /// SyncManager::new() so it can detect local changes and trigger sync
    /// cycles. Tombstone recording (record_item_deletion, record_unfavorite,
    /// remove_unfavorite) is already handled in the data mutation methods
    /// below — SyncManager only needs to observe this flag.
    pub sync_dirty: Arc<AtomicBool>,
    pub bitmap_paste_finished: Arc<AtomicBool>,
    pub toast_message: Option<String>,
    /// true = warning (red), false = info (green).
    pub toast_is_warning: bool,
    /// Foreground app info (updated by WindowManager poll loop, consumed by hotkey settings tab).
    pub foreground_app_name: String,
    pub foreground_window_title: String,
    /// Base64-encoded PNG icon of the current foreground app.
    pub foreground_app_icon_base64: String,
    /// Whether a hotkey recording is in progress (set by settings UI, cleared by WM poll).
    pub hotkey_recording: bool,
    /// Whether a quick-window hotkey recording is in progress.
    pub recording_quick_hotkey: bool,
    /// GPUI-facing sync status and backend snapshots.
    pub sync: SyncState,
    /// Update available info (set by WM poll, consumed by RootView + settings).
    pub update_available: Option<UpdateInfo>,
    /// Current update phase (for UI display).
    pub update_phase: UpdatePhase,
}

/// Pinyin-aware text matching.
///
/// This function checks three forms in order:
/// 1. Direct lowercase substring match (covers English text)
/// 2. Full pinyin match — "zhongguo" matches "中国"
/// 3. Pinyin initial match — "zg" matches "中国"
pub fn pinyin_match(text: &str, keyword: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let kw = keyword.to_lowercase();

    // 1. Direct text match — covers English, numbers, symbols
    if text.to_lowercase().contains(&kw) {
        return true;
    }

    // 2. Full pinyin match
    let full_py: String = text.to_pinyin().flatten().map(|p| p.plain()).collect();
    if full_py.contains(&kw) {
        return true;
    }

    // 3. Pinyin initials match
    let initials: String = text
        .to_pinyin()
        .flatten()
        .filter_map(|p| p.plain().chars().next())
        .collect();
    initials.contains(&kw)
}

fn item_matches_keyword(item: &crate::core::types::ClipboardItem, keyword: &str) -> bool {
    let full_text_matches = if matches!(item.display_kind(), DisplayKind::Html) {
        html_text::has_visible_match(&item.full_text, |text| pinyin_match(text, keyword))
    } else {
        pinyin_match(&item.full_text, keyword)
    };
    if full_text_matches {
        return true;
    }

    if !item.rich_data.is_empty() {
        let rich = RichData::from_json(&item.rich_data);

        if let Some(html) = rich.html.as_deref() {
            if html_text::has_visible_match(html, |text| pinyin_match(text, keyword)) {
                return true;
            }
        }

        let rich_texts = [
            rich.rtf.as_deref(),
            rich.ocr_text.as_deref(),
            rich.qr_text.as_deref(),
            rich.page_title.as_deref(),
        ];
        if rich_texts
            .into_iter()
            .flatten()
            .any(|text| pinyin_match(text, keyword))
        {
            return true;
        }
    }

    if !item.note.is_empty() && pinyin_match(&item.note, keyword) {
        return true;
    }

    item.tags.iter().any(|tag| pinyin_match(&tag.name, keyword))
}

fn item_matches_keywords(item: &crate::core::types::ClipboardItem, keywords: &[String]) -> bool {
    keywords
        .iter()
        .all(|keyword| item_matches_keyword(item, keyword))
}

fn truncate_chars(text: &mut String, limit: usize) {
    if let Some((idx, _)) = text.char_indices().nth(limit) {
        text.truncate(idx);
    }
}

fn truncated_owned(text: String, limit: usize) -> String {
    let mut text = text;
    truncate_chars(&mut text, limit);
    text
}

fn item_can_be_merged(item: &ClipboardItem) -> bool {
    matches!(
        item.content_type,
        ContentType::PlainText | ContentType::RichText
    ) && !item.full_text.is_empty()
}

fn shrink_item_for_list(item: &mut ClipboardItem) {
    truncate_chars(&mut item.full_text, LIST_FULL_TEXT_LIMIT);
    truncate_chars(&mut item.note, LIST_NOTE_LIMIT);

    if item.rich_data.is_empty() {
        return;
    }

    let rich = RichData::from_json(&item.rich_data);
    let preview = RichData {
        html: rich
            .html
            .filter(|text| !text.is_empty())
            .map(|text| truncated_owned(text, LIST_RICH_HTML_LIMIT)),
        rtf: rich
            .rtf
            .filter(|text| !text.is_empty())
            .map(|text| truncated_owned(text, LIST_RICH_HTML_LIMIT)),
        ocr_text: rich
            .ocr_text
            .filter(|text| !text.is_empty())
            .map(|text| truncated_owned(text, LIST_RICH_AUX_LIMIT)),
        qr_text: rich
            .qr_text
            .filter(|text| !text.is_empty())
            .map(|text| truncated_owned(text, LIST_RICH_AUX_LIMIT)),
        page_title: rich
            .page_title
            .filter(|text| !text.is_empty())
            .map(|text| truncated_owned(text, LIST_RICH_AUX_LIMIT)),
        drive_label: rich
            .drive_label
            .filter(|text| !text.is_empty())
            .map(|text| truncated_owned(text, LIST_RICH_AUX_LIMIT)),
    };
    item.rich_data = preview.to_json();
}

impl AppState {
    /// Open the database and load initial data.
    pub fn new(settings: AppSettings) -> Self {
        let db_path = settings.resolve_db_path();
        crate::core::paths::init_images_dir(&settings.db_path);
        let order_by = if settings.sort_by_created {
            "created_at"
        } else {
            "updated_at"
        };
        let query_limit = if settings.max_items == 0 {
            200
        } else {
            (settings.max_items as usize).saturating_mul(2).max(200)
        };
        let db = Database::open(&db_path.to_string_lossy())
            .unwrap_or_else(|e| panic!("Failed to open database at {db_path:?}: {e}"));
        if settings.cleanup_interval != "never" || settings.retention_days > 0 {
            crate::core::cache_cleanup::run_cleanup(&db, settings.retention_days);
        }

        let items = db
            .load_filtered_list_with_tags(&ClipboardFilters::default(), query_limit, order_by)
            .unwrap_or_else(|e| {
                log::error!("Failed to load initial items: {e}");
                Vec::new()
            });

        let tags = db.get_all_tags().unwrap_or_else(|e| {
            log::error!("Failed to load tags: {e}");
            Vec::new()
        });

        let sync = SyncState::from_settings(&settings);

        Self {
            settings,
            db,
            items,
            tags,
            filters: ClipboardFilters::default(),
            selected_ids: Vec::new(),
            editing_tag_id: -1,
            editing_tag_name: String::new(),
            editing_tag_color: "#3B82F6".into(),
            editing_item_id: -1,
            editing_item: None,
            batch_pasting: Arc::new(AtomicBool::new(false)),
            skip_next: Arc::new(AtomicBool::new(false)),
            sync_dirty: Arc::new(AtomicBool::new(false)),
            bitmap_paste_finished: Arc::new(AtomicBool::new(false)),
            toast_message: None,
            toast_is_warning: false,
            foreground_app_name: String::new(),
            foreground_window_title: String::new(),
            foreground_app_icon_base64: String::new(),
            hotkey_recording: false,
            recording_quick_hotkey: false,
            sync,
            update_available: None,
            update_phase: UpdatePhase::Idle,
        }
    }

    /// Check whether a mutation on this item should mark sync as dirty.
    ///
    /// Image/file types are never synced. In favorites-only mode, only
    /// favorite items trigger sync cycles.
    pub fn should_mark_sync_dirty(&self, item: &ClipboardItem) -> bool {
        if matches!(item.content_type, ContentType::Image | ContentType::File) {
            return false;
        }
        if self.settings.sync_favorites_only && !item.is_favorite {
            return false;
        }
        true
    }

    /// Reload items from database with current filters.
    pub fn reload_items(&mut self) {
        let result = if self.filters.has_keyword() {
            self.load_keyword_filtered_items()
        } else {
            self.db
                .load_filtered_list_with_tags(&self.filters, self.query_limit(), self.order_by())
        };

        let keyword_filtered = self.filters.has_keyword();

        match result {
            Ok(mut items) => {
                let keywords = self.filters.keyword_terms();
                if !keyword_filtered && !keywords.is_empty() {
                    items.retain(|item| item_matches_keywords(item, &keywords));
                }
                // Hide non-native platform paths when the setting is enabled.
                if self.settings.filter_foreign_paths {
                    items.retain(|item| {
                        if item.meta_type == "path" {
                            crate::core::types::path_is_native(&item.full_text)
                        } else {
                            true
                        }
                    });
                }
                self.items = items;
            }
            Err(e) => log::error!("Failed to reload items: {e}"),
        }
    }

    fn load_keyword_filtered_items(&self) -> rusqlite::Result<Vec<ClipboardItem>> {
        let keywords = self.filters.keyword_terms();
        if keywords.is_empty() {
            return self.db.load_filtered_with_tags(
                &self.filters,
                self.query_limit(),
                self.order_by(),
            );
        }

        let result_limit = self.query_limit();
        let scan_limit = result_limit
            .saturating_mul(KEYWORD_SEARCH_SCAN_MULTIPLIER)
            .clamp(KEYWORD_SEARCH_MIN_SCAN_LIMIT, KEYWORD_SEARCH_MAX_SCAN_LIMIT);
        let mut matches = Vec::new();
        let mut offset = 0;

        while offset < scan_limit && matches.len() < result_limit {
            let page_limit = KEYWORD_SEARCH_PAGE_SIZE.min(scan_limit - offset);
            let mut page = self.db.load_filtered_page_with_tags(
                &self.filters,
                page_limit,
                offset,
                self.order_by(),
            )?;
            if page.is_empty() {
                break;
            }

            for item in page.drain(..) {
                if self.settings.filter_foreign_paths
                    && item.meta_type == "path"
                    && !crate::core::types::path_is_native(&item.full_text)
                {
                    continue;
                }
                if item_matches_keywords(&item, &keywords) {
                    let mut item = item;
                    shrink_item_for_list(&mut item);
                    matches.push(item);
                    if matches.len() >= result_limit {
                        break;
                    }
                }
            }

            if page_limit < KEYWORD_SEARCH_PAGE_SIZE {
                break;
            }
            offset += page_limit;
        }

        Ok(matches)
    }

    /// Clear all items from memory to free resources while window is hidden.
    /// Items are reloaded from DB on next `reload_items()` call.
    pub fn clear_items(&mut self) {
        self.items.clear();
        self.items.shrink_to_fit();
        self.selected_ids.clear();
    }

    /// Update keyword filter and reload visible items.
    pub fn set_keyword(&mut self, keyword: &str) {
        self.filters.set_keyword(keyword);
        self.selected_ids.clear();
        self.reload_items();
    }

    /// Toggle a content-type filter and reload visible items.
    /// Each type filter is now independent (image/file are separate).
    pub fn toggle_type_filter(&mut self, type_name: &str) {
        self.filters.toggle_type(type_name);
        self.selected_ids.clear();
        self.reload_items();
    }

    /// Toggle favorites-only filter and reload visible items.
    pub fn toggle_favorites_filter(&mut self) {
        self.filters.toggle_favorites_only();
        self.selected_ids.clear();
        self.reload_items();
    }

    /// Toggle a tag filter and reload visible items.
    pub fn toggle_tag_filter(&mut self, tag_id: i64) {
        self.filters.toggle_tag(tag_id);
        self.selected_ids.clear();
        self.reload_items();
    }

    /// Clear tag filters and reload visible items.
    pub fn clear_tag_filters(&mut self) {
        self.filters.clear_tag_filters();
        self.selected_ids.clear();
        self.reload_items();
    }

    /// Toggle tag matching mode and reload visible items.
    pub fn toggle_tag_match_mode(&mut self) {
        self.filters.toggle_tag_mode();
        self.selected_ids.clear();
        self.reload_items();
    }

    /// Toggle whether a tag is pinned in the side tag bar, then persist settings.
    pub fn toggle_pinned_tag(&mut self, tag_id: i64) {
        if let Some(pos) = self
            .settings
            .pinned_tag_ids
            .iter()
            .position(|&id| id == tag_id)
        {
            self.settings.pinned_tag_ids.remove(pos);
        } else {
            self.settings.pinned_tag_ids.push(tag_id);
        }
        self.settings.save();
    }

    /// Reload tags from database.
    pub fn reload_tags(&mut self) {
        match self.db.get_all_tags() {
            Ok(tags) => self.tags = tags,
            Err(e) => log::error!("Failed to reload tags: {e}"),
        }
    }

    /// Select a single item, clearing previous selection.
    pub fn select_single(&mut self, id: i64) {
        self.selected_ids.clear();
        self.selected_ids.push(id);
    }

    /// Replace selection with a range of IDs (for Ctrl+click toggle and Shift+click range).
    pub fn range_select(&mut self, ids: &[i64]) {
        self.selected_ids = ids.to_vec();
    }

    /// Clear all selections.
    pub fn clear_selection(&mut self) {
        self.selected_ids.clear();
    }

    /// Start editing a tag (opens TagEditPanel overlay).
    pub fn start_edit_tag(&mut self, id: i64, name: &str, color: &str) {
        self.editing_tag_id = id;
        self.editing_tag_name = name.to_string();
        self.editing_tag_color = color.to_string();
    }

    /// Cancel tag editing.
    pub fn cancel_edit_tag(&mut self) {
        self.editing_tag_id = -1;
        self.editing_tag_name.clear();
    }

    /// Update the preview color during tag editing (no save).
    pub fn set_edit_tag_color(&mut self, color: &str) {
        self.editing_tag_color = color.to_string();
    }

    /// Create a new tag with round-robin color from presets.
    pub fn create_tag(&mut self, name: &str) {
        let count = self.tags.len();
        let color = next_tag_color(count);
        match self.db.create_tag(name, color) {
            Ok(_) => {
                self.sync_dirty.store(true, Ordering::SeqCst);
                self.reload_tags();
            }
            Err(e) => log::error!("Failed to create tag: {e}"),
        }
    }

    /// Delete a tag by id.
    pub fn delete_tag(&mut self, tag_id: i64) {
        let tag = match self.db.get_tag_by_id(tag_id) {
            Ok(tag) => tag,
            Err(e) => {
                log::error!("Failed to load tag before deletion: {e}");
                None
            }
        };
        match self.db.delete_tag(tag_id) {
            Ok(_) => {
                if let Some(tag) = tag {
                    let now = chrono::Utc::now().to_rfc3339();
                    let device = crate::services::backends::local_folder::hostname();
                    if let Err(e) = self
                        .db
                        .record_tag_deletion(&tag.uid, &tag.name, &now, &device)
                    {
                        log::error!("Failed to record tag deletion: {e}");
                    }
                }
                self.sync_dirty.store(true, Ordering::SeqCst);
                self.filters.tag_ids.retain(|&id| id != tag_id);
                self.reload_tags();
                self.reload_items();
            }
            Err(e) => log::error!("Failed to delete tag: {e}"),
        }
    }

    /// Update tag name and color.
    pub fn update_tag(&mut self, tag_id: i64, name: &str, color: &str) {
        match self.db.update_tag(tag_id, name, color) {
            Ok(_) => {
                self.sync_dirty.store(true, Ordering::SeqCst);
                self.cancel_edit_tag();
                self.reload_tags();
                // Incremental update: update tag name/color on all items
                // in-place so clipboard cards show the new values immediately
                // without reloading from DB (which would reorder by updated_at).
                for item in &mut self.items {
                    if let Some(tag) = item.tags.iter_mut().find(|t| t.id == tag_id) {
                        tag.name = name.to_string();
                        tag.color = color.to_string();
                    }
                }
            }
            Err(e) => log::error!("Failed to update tag: {e}"),
        }
    }

    pub fn start_edit_item(&mut self, id: i64) -> bool {
        match self.db.get_by_id_with_tags(id) {
            Ok(Some(item)) => {
                self.editing_item_id = id;
                self.editing_item = Some(item);
                true
            }
            Ok(None) => {
                log::warn!("start_edit_item({id}): item not found");
                false
            }
            Err(e) => {
                log::error!("start_edit_item({id}): {e}");
                false
            }
        }
    }

    pub fn cancel_edit_item(&mut self) {
        self.editing_item_id = -1;
        self.editing_item = None;
    }

    pub fn save_edited_item(&mut self, id: i64, text: &str, editor_type: &str) -> bool {
        let (content_type, meta_type, rich_data) = Self::storage_for_editor_type(editor_type, text);
        // Pre-flight: is the item in sync scope? Check before DB write.
        let mark_dirty = self
            .items
            .iter()
            .find(|it| it.id == id)
            .is_some_and(|item| self.should_mark_sync_dirty(item));
        match self
            .db
            .update_content_with_rich_data(id, text, content_type, meta_type, &rich_data)
        {
            Ok(_) => {
                if mark_dirty {
                    self.sync_dirty.store(true, Ordering::SeqCst);
                }
                self.cancel_edit_item();
                // Incremental update: preserve scroll position (consistent with
                // update_note / toggle_favorite). The item keeps its current
                // position; re-sort happens on next window open via reload_items().
                if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
                    item.full_text = text.to_string();
                    item.content_type = ContentType::from_str(content_type);
                    item.meta_type = meta_type.to_string();
                    item.rich_data = rich_data;
                    item.image_path.clear();
                    item.file_data.clear();
                    item.image_width = 0;
                    item.image_height = 0;
                    item.size = text.chars().count() as i64;
                    item.updated_at = chrono::Utc::now();
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    std::hash::Hash::hash(&text, &mut hasher);
                    item.content_hash = std::hash::Hasher::finish(&hasher);
                }
                true
            }
            Err(e) => {
                log::error!("save_edited_item({id}): {e}");
                false
            }
        }
    }

    fn storage_for_editor_type(
        editor_type: &str,
        text: &str,
    ) -> (&'static str, &'static str, String) {
        match editor_type {
            "markdown" => ("rich_text", "markdown", String::new()),
            "html" => {
                let rich = RichData {
                    html: Some(text.to_string()),
                    ..Default::default()
                };
                ("rich_text", "html", rich.to_json())
            }
            "link" => ("plain_text", "link", String::new()),
            "path" => ("plain_text", "path", String::new()),
            "color" => ("plain_text", "color", String::new()),
            "email" => ("plain_text", "email", String::new()),
            "phone" => ("plain_text", "phone", String::new()),
            _ => ("plain_text", "", String::new()),
        }
    }

    pub fn clear_toast(&mut self) {
        self.toast_message = None;
        self.toast_is_warning = false;
    }

    pub fn show_toast(&mut self, message: impl Into<String>) {
        self.toast_message = Some(message.into());
        self.toast_is_warning = false;
    }

    pub fn show_warning_toast(&mut self, message: impl Into<String>) {
        self.toast_message = Some(message.into());
        self.toast_is_warning = true;
    }

    pub fn take_bitmap_paste_finished(&self) -> bool {
        self.bitmap_paste_finished.swap(false, Ordering::SeqCst)
    }

    fn touch_item_usage(&mut self, id: i64) {
        let mark_dirty = match self.db.get_by_id(id) {
            Ok(Some(item)) => self.should_mark_sync_dirty(&item),
            Ok(None) => {
                log::warn!("touch_item_usage: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("touch_item_usage: db error for {id}: {e}");
                return;
            }
        };

        match self.db.touch_item(id) {
            Ok(_) => {
                if mark_dirty {
                    self.sync_dirty.store(true, Ordering::SeqCst);
                }
                self.reload_items();
            }
            Err(e) => log::error!("touch_item_usage({id}): {e}"),
        }
    }

    pub fn toggle_item_tag(&mut self, item_id: i64, tag_id: i64) {
        let item = match self.items.iter().find(|item| item.id == item_id) {
            Some(item) => item,
            None => return,
        };
        let mark_dirty = self.should_mark_sync_dirty(item);
        let has_tag = item.tags.iter().any(|tag| tag.id == tag_id);
        let result = if has_tag {
            self.db.remove_item_tag(item_id, tag_id)
        } else {
            self.db.add_item_tag(item_id, tag_id)
        };
        if let Err(e) = result {
            log::error!("toggle_item_tag({item_id}, {tag_id}): {e}");
            return;
        }
        if mark_dirty {
            self.sync_dirty.store(true, Ordering::SeqCst);
        }
        // Incremental update: re-fetch tags for the affected item instead of
        // reloading all items, so scroll position is preserved.
        if let Ok(Some(updated)) = self.db.get_by_id_with_tags(item_id) {
            if let Some(item) = self.items.iter_mut().find(|it| it.id == item_id) {
                item.tags = updated.tags;
                item.updated_at = updated.updated_at;
            }
        }
    }

    pub fn batch_add_tag(&mut self, ids: &[i64], tag_id: i64) {
        let mark_dirty = ids.iter().any(|&id| {
            self.items
                .iter()
                .find(|it| it.id == id)
                .is_some_and(|item| self.should_mark_sync_dirty(item))
        });
        for &id in ids {
            if let Err(e) = self.db.add_item_tag(id, tag_id) {
                log::error!("batch_add_tag({id}, {tag_id}): {e}");
            }
        }
        if mark_dirty {
            self.sync_dirty.store(true, Ordering::SeqCst);
        }
        // Incremental update: re-fetch tags for affected items (preserve scroll position)
        for &id in ids {
            if let Ok(Some(updated)) = self.db.get_by_id_with_tags(id) {
                if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
                    item.tags = updated.tags;
                    item.updated_at = updated.updated_at;
                }
            }
        }
    }

    pub fn batch_remove_tag(&mut self, ids: &[i64], tag_id: i64) {
        let mark_dirty = ids.iter().any(|&id| {
            self.items
                .iter()
                .find(|it| it.id == id)
                .is_some_and(|item| self.should_mark_sync_dirty(item))
        });
        for &id in ids {
            if let Err(e) = self.db.remove_item_tag(id, tag_id) {
                log::error!("batch_remove_tag({id}, {tag_id}): {e}");
            }
        }
        if mark_dirty {
            self.sync_dirty.store(true, Ordering::SeqCst);
        }
        // Incremental update: re-fetch tags for affected items (preserve scroll position)
        for &id in ids {
            if let Ok(Some(updated)) = self.db.get_by_id_with_tags(id) {
                if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
                    item.tags = updated.tags;
                    item.updated_at = updated.updated_at;
                }
            }
        }
    }

    pub fn clear_item_tags(&mut self, item_id: i64) {
        let mark_dirty = self
            .items
            .iter()
            .find(|it| it.id == item_id)
            .is_some_and(|item| self.should_mark_sync_dirty(item));
        if let Err(e) = self.db.clear_item_tags(item_id) {
            log::error!("clear_item_tags({item_id}): {e}");
            return;
        }
        if mark_dirty {
            self.sync_dirty.store(true, Ordering::SeqCst);
        }
        // Incremental update: re-fetch tags for the affected item (preserve scroll position)
        if let Ok(Some(updated)) = self.db.get_by_id_with_tags(item_id) {
            if let Some(item) = self.items.iter_mut().find(|it| it.id == item_id) {
                item.tags = updated.tags;
                item.updated_at = updated.updated_at;
            }
        }
    }

    pub fn clear_tags_for_items(&mut self, ids: &[i64]) {
        let mark_dirty = ids.iter().any(|&id| {
            self.items
                .iter()
                .find(|it| it.id == id)
                .is_some_and(|item| self.should_mark_sync_dirty(item))
        });
        for &id in ids {
            if let Err(e) = self.db.clear_item_tags(id) {
                log::error!("clear_tags_for_items({id}): {e}");
            }
        }
        if mark_dirty {
            self.sync_dirty.store(true, Ordering::SeqCst);
        }
        // Incremental update: re-fetch tags for affected items (preserve scroll position)
        for &id in ids {
            if let Ok(Some(updated)) = self.db.get_by_id_with_tags(id) {
                if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
                    item.tags = updated.tags;
                    item.updated_at = updated.updated_at;
                }
            }
        }
    }

    fn order_by(&self) -> &'static str {
        if self.settings.sort_by_created {
            "created_at"
        } else {
            "updated_at"
        }
    }

    fn query_limit(&self) -> usize {
        if self.settings.max_items == 0 {
            200
        } else {
            (self.settings.max_items as usize)
                .saturating_mul(2)
                .max(200)
        }
    }

    pub fn open_original_image(&self, id: i64) {
        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("open_original_image: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("open_original_image: db error for {id}: {e}");
                return;
            }
        };
        if !item.image_path.is_empty() {
            let path = item.image_path.clone();
            // --- Spawn on a background thread — ShellExecuteW can pump Windows ---
            // messages internally (DDE/COM) and deadlock if called from the
            // --- GPUI main thread event handler. ---
            std::thread::spawn(move || {
                open_system_target(&path);
            });
        }
    }

    /// Paste the image file path as plain text.
    pub fn paste_image_path(&mut self, id: i64) {
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("paste_image_path: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("paste_image_path: db error for {id}: {e}");
                return;
            }
        };
        if item.image_path.is_empty() {
            return;
        }
        self.touch_item_usage(id);
        self.write_text_to_clipboard_internal(&item.image_path);
        restore_paste_target();
        let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
        paste_after_delay(shortcuts);
    }

    /// Paste the file paths as plain text.
    pub fn paste_file_path(&mut self, id: i64) {
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("paste_file_path: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("paste_file_path: db error for {id}: {e}");
                return;
            }
        };
        if item.file_data.is_empty() {
            return;
        }
        let file_data = FileData::from_json(&item.file_data);
        if file_data.files.is_empty() {
            return;
        }
        let paths_text: String = file_data
            .files
            .iter()
            .map(|f| f.path.clone())
            .collect::<Vec<_>>()
            .join("\n");

        self.touch_item_usage(id);
        self.write_text_to_clipboard_internal(&paths_text);
        restore_paste_target();
        let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
        paste_after_delay(shortcuts);
    }

    pub fn open_item_location(&self, id: i64) {
        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("open_item_location: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("open_item_location: db error for {id}: {e}");
                return;
            }
        };

        // ── Link: open in browser ─────────────────────────────────
        if item.meta_type == "link" {
            if item.full_text.is_empty() {
                log::warn!("open_item_location: link item {id} has empty full_text");
                return;
            }
            let mut text = item.full_text.clone();
            // Protocol-less URLs need a scheme for ShellExecuteW to
            // dispatch them to the browser instead of treating them as
            // file-system paths.
            if !text.starts_with("http://") && !text.starts_with("https://") {
                text = format!("https://{}", text);
            }
            let target = text;

            // ── Lazy URL metadata backfill for old/synced link items ─────
            let mark_sync_dirty = self.should_mark_sync_dirty(&item);
            crate::services::url_assets::spawn_link_open_backfill(
                item.full_text.clone(),
                item.content_hash,
                self.settings.db_path.clone(),
                item.rich_data.clone(),
                self.settings.auto_fetch_url_title,
                self.sync_dirty.clone(),
                mark_sync_dirty,
            );

            // --- Spawn on a background thread to avoid ShellExecuteW ---
            // --- deadlock on the GPUI main thread (DDE/COM message pumping). ---
            std::thread::spawn(move || {
                open_system_target(&target);
            });
            return;
        }

        // ── Path: reveal in Explorer (not open/execute) ──────────
        if item.meta_type == "path" {
            if item.full_text.is_empty() {
                log::warn!("open_item_location: path item {id} has empty full_text");
                return;
            }

            // ── Lazy drive-label fill for old path items ──────────
            let rd = RichData::from_json(&item.rich_data);
            if rd.drive_label.is_none() {
                if let Some(label) = crate::core::types::path_drive_label(&item.full_text) {
                    let resolved = crate::core::paths::resolve_db_path(&self.settings.db_path);
                    if let Ok(db) = crate::core::db::Database::open(&resolved.to_string_lossy()) {
                        let mut new_rd = rd;
                        new_rd.drive_label = Some(label);
                        let _ = db.update_rich_data(item.id, &new_rd.to_json());
                    }
                }
            }

            // Reveal in Explorer if the path exists locally;
            // silently skip non-existent paths.
            if crate::core::types::path_exists(&item.full_text) {
                let path = item.full_text.clone();
                std::thread::spawn(move || {
                    reveal_file_location(&path);
                });
            }
            return;
        }

        // ── File list: reveal first file in Explorer ─────────────────
        if item.content_type == ContentType::File {
            let file_data = FileData::from_json(&item.file_data);
            if let Some(first) = file_data.files.first() {
                let path = first.path.clone();
                // --- Spawn on a background thread to avoid ShellExecuteW ---
                // --- deadlock on the GPUI main thread (DDE/COM message pumping). ---
                std::thread::spawn(move || {
                    reveal_file_location(&path);
                });
            }
            return;
        }

        log::warn!(
            "open_item_location: item {id} has no openable location \
             (content_type={:?}, meta_type={:?})",
            item.content_type,
            item.meta_type,
        );
    }

    pub fn qr_action(&mut self, id: i64) {
        let qr_text = match self.db.get_by_id(id) {
            Ok(Some(item)) => RichData::from_json(&item.rich_data).qr_text,
            Ok(None) => {
                log::warn!("qr_action: item {id} not found");
                None
            }
            Err(e) => {
                log::error!("qr_action: db error for {id}: {e}");
                None
            }
        };

        if let Some(text) = qr_text {
            self.handle_qr_text(text);
        } else {
            self.show_toast(I18nKey::ToastNoQr.text());
        }
    }

    pub fn qr_detect(&mut self, id: i64) {
        let qr_text = match self.db.get_by_id(id) {
            Ok(Some(item)) => {
                let rich = RichData::from_json(&item.rich_data);
                if rich.qr_text.is_some() {
                    rich.qr_text
                } else if !item.image_path.is_empty() {
                    match crate::core::qr::detect_qr(std::path::Path::new(&item.image_path)) {
                        Ok(Some(text)) => {
                            let mut next_rich = RichData::from_json(&item.rich_data);
                            next_rich.qr_text = Some(text.clone());
                            if let Err(e) = self.db.update_rich_data(id, &next_rich.to_json()) {
                                log::error!("qr_detect: update rich_data failed for {id}: {e}");
                            }
                            self.reload_items();
                            Some(text)
                        }
                        Ok(None) => None,
                        Err(e) => {
                            log::error!("qr_detect: {e}");
                            None
                        }
                    }
                } else {
                    None
                }
            }
            Ok(None) => {
                log::warn!("qr_detect: item {id} not found");
                None
            }
            Err(e) => {
                log::error!("qr_detect: db error for {id}: {e}");
                None
            }
        };

        if let Some(text) = qr_text {
            self.handle_qr_text(text);
        } else {
            self.show_toast(I18nKey::ToastNoQr.text());
        }
    }

    pub fn paste_ocr(&mut self, id: i64) {
        let ocr_load = match self.db.get_by_id(id) {
            Ok(Some(item)) => {
                let rich = RichData::from_json(&item.rich_data);
                match rich.ocr_text.filter(|text| !text.trim().is_empty()) {
                    Some(cached) => Some((cached, true, String::new(), String::new())),
                    None if item.image_path.is_empty() => None,
                    None => Some((
                        String::new(),
                        false,
                        item.image_path.clone(),
                        item.rich_data.clone(),
                    )),
                }
            }
            Ok(None) => {
                log::warn!("paste_ocr: item {id} not found");
                None
            }
            Err(e) => {
                log::error!("paste_ocr: db error for {id}: {e}");
                None
            }
        };

        let ocr_text = match ocr_load {
            Some((cached, true, _, _)) => Some(cached),
            Some((_, false, img_path, existing_rich)) => {
                let engine = crate::core::ocr::create_ocr_engine();
                match engine.recognize(std::path::Path::new(&img_path)) {
                    Ok(text) if !text.trim().is_empty() => {
                        let mut next_rich = RichData::from_json(&existing_rich);
                        next_rich.ocr_text = Some(text.clone());
                        if let Err(e) = self.db.update_rich_data(id, &next_rich.to_json()) {
                            log::error!("paste_ocr: update rich_data failed for {id}: {e}");
                        }
                        self.reload_items();
                        Some(text)
                    }
                    Ok(_) => None,
                    Err(e) => {
                        log::error!("OCR error for item {id}: {e}");
                        None
                    }
                }
            }
            _ => None,
        };

        if let Some(text) = ocr_text {
            // --- Use skip_next (not batch_pasting) — the OCR text written to ---
            // --- clipboard is internal and should be "consumed" by the listener ---
            // --- (skip one cycle + update baseline seq#) rather than recorded ---
            // as a new history entry. This matches the Slint-era behaviour.
            self.touch_item_usage(id);
            self.write_text_to_clipboard_internal(&text);
            crate::platform::paste::restore_paste_target();
            let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
            crate::platform::paste::paste_after_delay(shortcuts);
        } else {
            self.show_toast(I18nKey::ToastNoOcr.text());
        }
    }

    fn handle_qr_text(&mut self, text: String) {
        if text.starts_with("http://") || text.starts_with("https://") {
            // --- Spawn on a background thread — open_releases_page calls ---
            // --- ShellExecuteW which can pump Windows messages internally ---
            // (DDE/COM) and deadlock if called from the GPUI main thread.
            let url = text;
            std::thread::spawn(move || {
                crate::services::update::open_releases_page(&url);
            });
            return;
        }
        if self.write_text_to_clipboard_internal(&text) {
            self.show_toast(I18nKey::ToastQrCopied.text());
        }
    }

    fn with_internal_clipboard_write<T>(
        batch_pasting: &Arc<AtomicBool>,
        skip_next: &Arc<AtomicBool>,
        operation: impl FnOnce() -> T,
    ) -> T {
        batch_pasting.store(true, Ordering::SeqCst);
        let result = operation();
        skip_next.store(true, Ordering::SeqCst);
        batch_pasting.store(false, Ordering::SeqCst);
        result
    }

    fn write_item_to_clipboard_internal(&self, item: &ClipboardItem, copy_as_plain_text: bool) {
        Self::with_internal_clipboard_write(&self.batch_pasting, &self.skip_next, || {
            crate::services::clipboard_ops::write_item_to_clipboard(item, copy_as_plain_text);
        });
    }

    fn write_text_to_clipboard_internal(&self, text: &str) -> bool {
        Self::with_internal_clipboard_write(&self.batch_pasting, &self.skip_next, || {
            crate::services::clipboard_ops::write_text_to_clipboard(text)
        })
    }

    /// Copy a single item to the system clipboard (no paste simulation).
    pub fn copy_item(&mut self, id: i64, copy_as_plain_text: bool) {
        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("copy_item: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("copy_item: db error for {id}: {e}");
                return;
            }
        };
        self.touch_item_usage(id);
        self.write_item_to_clipboard_internal(&item, copy_as_plain_text);
    }

    /// Paste a single item: write to clipboard, restore focus, simulate Ctrl+V.
    pub fn paste_item(&mut self, id: i64, copy_as_plain_text: bool) {
        use crate::core::types::ContentType;
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("paste_item: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("paste_item: db error for {id}: {e}");
                return;
            }
        };

        self.touch_item_usage(id);
        let is_file = item.content_type == ContentType::File
            || (item.content_type == ContentType::Image && !item.image_path.is_empty());
        let expected = item.full_text.clone();
        self.write_item_to_clipboard_internal(&item, copy_as_plain_text);

        if !expected.is_empty() && !is_file {
            crate::services::clipboard_ops::verify_clipboard_content(&expected, 200);
        }
        if is_file {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        restore_paste_target();
        let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
        paste_after_delay(shortcuts);
    }

    /// Paste an image as bitmap data for apps that do not accept file references.
    pub fn paste_image_as_bitmap(&mut self, id: i64) {
        use crate::core::types::ContentType;
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("paste_image_as_bitmap: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("paste_image_as_bitmap: db error for {id}: {e}");
                return;
            }
        };

        if item.content_type != ContentType::Image || item.image_path.is_empty() {
            return;
        }

        self.touch_item_usage(id);
        self.bitmap_paste_finished.store(false, Ordering::SeqCst);
        self.show_toast(I18nKey::ToastPreparingBitmapImage.text());
        let image_path = item.image_path.clone();
        let item_id = item.id;
        let batch_pasting = self.batch_pasting.clone();
        let skip_next = self.skip_next.clone();
        let bitmap_paste_finished = self.bitmap_paste_finished.clone();
        let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());

        std::thread::spawn(move || {
            let Some(img_data) = crate::services::clipboard_ops::load_image_bitmap(&image_path)
            else {
                bitmap_paste_finished.store(true, Ordering::SeqCst);
                return;
            };

            let wrote = Self::with_internal_clipboard_write(&batch_pasting, &skip_next, || {
                crate::services::clipboard_ops::write_bitmap_image_to_clipboard(img_data)
            });

            if !wrote {
                log::warn!("paste_image_as_bitmap: failed to write image for item {item_id}");
                bitmap_paste_finished.store(true, Ordering::SeqCst);
                return;
            }

            if !crate::services::clipboard_ops::verify_clipboard_image(500) {
                log::warn!("paste_image_as_bitmap: image verification failed for item {item_id}");
                std::thread::sleep(std::time::Duration::from_millis(100));
            }

            bitmap_paste_finished.store(true, Ordering::SeqCst);
            restore_paste_target();
            paste_after_delay(shortcuts);
        });
    }

    /// Paste a single item as plain text: write plain text to clipboard, restore focus, simulate Ctrl+V.
    pub fn paste_item_plain(&mut self, id: i64) {
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("paste_item_plain: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("paste_item_plain: db error for {id}: {e}");
                return;
            }
        };

        self.touch_item_usage(id);
        let is_file_or_image = item.content_type == ContentType::File
            || (item.content_type == ContentType::Image && !item.image_path.is_empty());
        let expected = if item.content_type == ContentType::Image && !item.image_path.is_empty() {
            item.image_path.clone()
        } else if item.content_type == ContentType::File && !item.file_data.is_empty() {
            let file_data = FileData::from_json(&item.file_data);
            file_data
                .files
                .iter()
                .map(|f| f.path.clone())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            item.full_text.clone()
        };
        self.write_item_to_clipboard_internal(&item, true);

        if !expected.is_empty() {
            crate::services::clipboard_ops::verify_clipboard_content(&expected, 200);
        }
        if is_file_or_image {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        restore_paste_target();
        let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
        paste_after_delay(shortcuts);
    }

    /// Convert a color item from HEX to RGB and paste.
    pub fn paste_as_rgb(&mut self, id: i64) {
        use crate::core::color::detect_color;
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            _ => {
                log::warn!("paste_as_rgb: item {id} not found");
                return;
            }
        };

        if let Some(color) = detect_color(&item.full_text) {
            self.touch_item_usage(id);
            let rgb_text = color.to_rgb();
            self.write_text_to_clipboard_internal(&rgb_text);
            crate::services::clipboard_ops::verify_clipboard_content(&rgb_text, 200);
            restore_paste_target();
            let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
            paste_after_delay(shortcuts);
        }
    }

    /// Convert a color item from RGB to HEX and paste.
    pub fn paste_as_hex(&mut self, id: i64) {
        use crate::core::color::detect_color;
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            _ => {
                log::warn!("paste_as_hex: item {id} not found");
                return;
            }
        };

        if let Some(color) = detect_color(&item.full_text) {
            self.touch_item_usage(id);
            let hex_text = color.to_css_hex();
            self.write_text_to_clipboard_internal(&hex_text);
            crate::services::clipboard_ops::verify_clipboard_content(&hex_text, 200);
            restore_paste_target();
            let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
            paste_after_delay(shortcuts);
        }
    }

    /// Batch paste multiple items sequentially.
    pub fn batch_paste(&mut self, ids: &[i64], copy_as_plain_text: bool) {
        use crate::core::types::ContentType;
        use crate::platform::paste::{paste_after_delay, paste_sync, restore_paste_target};

        // --- Suppress clipboard recording during batch paste to prevent ---
        // --- intermediate writes (newline separators) from being captured. ---
        self.batch_pasting
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let items: Vec<crate::core::types::ClipboardItem> = ids
            .iter()
            .filter_map(|&id| self.db.get_by_id(id).ok().flatten())
            .collect();

        for item in &items {
            self.touch_item_usage(item.id);
        }

        let n = items.len();
        for (i, item) in items.iter().enumerate() {
            // --- Newline separator between items (not before first) ---
            if i > 0 {
                crate::services::clipboard_ops::write_text_to_clipboard("\n");
                std::thread::sleep(std::time::Duration::from_millis(20));
                restore_paste_target();
                let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
                paste_sync(shortcuts);
                std::thread::sleep(std::time::Duration::from_millis(60));
            }

            let expected = item.full_text.clone();
            crate::services::clipboard_ops::write_item_to_clipboard(item, copy_as_plain_text);

            // --- Verify clipboard before pasting ---
            if item.content_type == ContentType::Image || item.content_type == ContentType::File {
                if !crate::services::clipboard_ops::verify_clipboard_files(300) {
                    log::warn!("batch_paste: file verification failed for item {}", item.id);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            } else if item.content_type != ContentType::File {
                if !crate::services::clipboard_ops::verify_clipboard_content(&expected, 300) {
                    log::warn!(
                        "batch_paste: text verification timed out for item {}",
                        item.id
                    );
                }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }

            restore_paste_target();

            if i < n - 1 {
                let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
                paste_sync(shortcuts);
                std::thread::sleep(std::time::Duration::from_millis(100));
            } else {
                let shortcuts = std::sync::Arc::new(self.settings.paste_shortcuts.clone());
                paste_after_delay(shortcuts);
            }
        }
        // --- Restore clipboard recording — batch paste is complete. ---
        self.skip_next
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.batch_pasting
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Merge selected text items into one new entry (seamless concatenation, no separator).
    /// Returns the new item's ID so the UI can scroll to it.
    ///
    /// When all selected items are rich text with HTML content, the merged entry
    /// preserves rich formatting by concatenating the HTML body contents.
    /// Otherwise it falls back to plain-text concatenation of `full_text`.
    pub fn merge_selected_items(&mut self) -> Option<i64> {
        if self.selected_ids.len() < 2 {
            return None;
        }

        // Preserve the explicit selection order. The list is usually sorted by
        // newest first, which can differ from the user's merge order.
        let items: Vec<&ClipboardItem> = self
            .selected_ids
            .iter()
            .filter_map(|id| self.items.iter().find(|item| item.id == *id))
            .collect();

        if items.len() < 2 {
            return None;
        }
        if !items.iter().all(|item| item_can_be_merged(item)) {
            log::warn!("merge_selected_items: selected items include non-text or empty content");
            return None;
        }

        let now = chrono::Utc::now();

        // ── Check whether all items are rich text with HTML ──
        let all_rich = items.iter().all(|item| {
            item.content_type == ContentType::RichText
                && !item.rich_data.is_empty()
                && RichData::from_json(&item.rich_data).html.is_some()
        });

        let (content_type, merged_text, rich_data, hash) = if all_rich {
            // ── Rich text path: merge HTML body contents ──
            let bodies: Vec<String> = items
                .iter()
                .map(|item| {
                    let rich = RichData::from_json(&item.rich_data);
                    let html = rich.html.as_deref().unwrap_or("");
                    extract_html_body(html)
                })
                .collect();
            let merged_body = bodies.concat();
            let merged_html =
                format!("<!DOCTYPE html>\n<html>\n<head><meta charset=\"utf-8\"></head>\n<body>\n{merged_body}\n</body>\n</html>");
            let merged_plain = item_texts_for_merge(&items);

            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&"merge", &mut hasher);
            std::hash::Hash::hash(&now.timestamp_nanos_opt(), &mut hasher);
            std::hash::Hash::hash(&self.selected_ids, &mut hasher);
            std::hash::Hash::hash(&merged_html, &mut hasher);
            let hash = std::hash::Hasher::finish(&hasher);

            let rd = RichData {
                html: Some(merged_html),
                ..Default::default()
            };

            (ContentType::RichText, merged_plain, rd.to_json(), hash)
        } else {
            // ── Plain-text path: concatenate full_text ──
            let merged = item_texts_for_merge(&items);

            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&"merge", &mut hasher);
            std::hash::Hash::hash(&now.timestamp_nanos_opt(), &mut hasher);
            std::hash::Hash::hash(&self.selected_ids, &mut hasher);
            std::hash::Hash::hash(&merged, &mut hasher);
            let hash = std::hash::Hasher::finish(&hasher);

            (ContentType::PlainText, merged, String::new(), hash)
        };

        let text_size = merged_text.chars().count() as i64;
        let item = ClipboardItem {
            id: 0,
            content_type,
            meta_type: String::new(),
            full_text: merged_text,
            content_hash: hash,
            created_at: now,
            updated_at: now,
            image_path: String::new(),
            image_width: 0,
            image_height: 0,
            rich_data,
            file_data: String::new(),
            is_favorite: false,
            note: String::new(),
            source_app_name: String::new(),
            source_app_icon: String::new(),
            size: text_size,
            tags: vec![],
        };

        // Write to DB.
        if let Err(e) = self.db.upsert(&item) {
            log::error!("merge_selected_items: upsert failed: {e}");
            return None;
        }

        // Retrieve the auto-generated ID.
        let new_id = match self.db.get_by_hash(hash) {
            Ok(Some(item)) => item.id,
            other => {
                log::error!("merge_selected_items: get_by_hash returned {other:?}");
                return None;
            }
        };

        let merged_count = items.len();

        // Mark sync dirty and refresh the in-memory list.
        self.sync_dirty
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.reload_items();

        // Clear multi-selection, select the new item.
        self.select_single(new_id);

        log::info!(
            "merge_selected_items: merged {merged_count} items → id={new_id} ({text_size} chars)"
        );

        Some(new_id)
    }

    pub fn can_merge_selected_items(&self) -> bool {
        if self.selected_ids.len() < 2 {
            return false;
        }

        let mut count = 0;
        for item in self
            .selected_ids
            .iter()
            .filter_map(|id| self.items.iter().find(|item| item.id == *id))
        {
            if !item_can_be_merged(item) {
                return false;
            }
            count += 1;
        }

        count >= 2
    }

    /// Update the note field for a clipboard item.
    /// Writes to DB (includes updated_at) and syncs the in-memory items list.
    pub fn update_note(&mut self, id: i64, note: &str) {
        let mark_dirty = self
            .items
            .iter()
            .find(|it| it.id == id)
            .is_some_and(|item| self.should_mark_sync_dirty(item));
        match self.db.update_note(id, note) {
            Ok(_) => {
                if mark_dirty {
                    self.sync_dirty.store(true, Ordering::SeqCst);
                }
                if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
                    item.note = note.to_string();
                    item.updated_at = chrono::Utc::now();
                }
            }
            Err(e) => log::error!("update_note({id}): {e}"),
        }
    }

    /// Toggle favorite status for a single item.
    ///
    /// # Tombstones (sync)
    /// - Favorited → unfavorited: records `unfavorited_items` tombstone
    /// - Unfavorited → favorited: removes existing `unfavorited_items` tombstone
    /// - Sets `sync_dirty = true`
    ///
    /// # Incremental update
    /// Updates `item.is_favorite` and `item.updated_at` in `self.items` directly,
    /// unless the favorites filter is active (needs full reload for accuracy).
    pub fn toggle_favorite(&mut self, id: i64) {
        let needs_full_refresh = self.filters.is_favorites_active();

        // Read current state before toggling (needed for tombstone direction)
        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("toggle_favorite({id}): item not found");
                return;
            }
            Err(e) => {
                log::error!("toggle_favorite({id}): {e}");
                return;
            }
        };
        let was_fav = item.is_favorite;
        let hash = item.content_hash;

        if let Err(e) = self.db.toggle_favorite(id) {
            log::error!("toggle_favorite({id}): {e}");
            return;
        }

        // --- Tombstone management ---
        if was_fav {
            // --- Was favorited, now unfavorited — record tombstone ---
            let now = chrono::Utc::now().to_rfc3339();
            let device = crate::services::backends::local_folder::hostname();
            if let Err(e) = self.db.record_unfavorite(hash, &now, &device) {
                log::error!("record_unfavorite({hash}): {e}");
            }
        } else {
            // --- Was unfavorited, now favorited — remove tombstone ---
            if let Err(e) = self.db.remove_unfavorite(hash) {
                log::error!("remove_unfavorite({hash}): {e}");
            }
        }

        self.sync_dirty.store(true, Ordering::SeqCst);

        if needs_full_refresh {
            self.reload_items();
            self.clear_selection();
        } else {
            // --- Incremental update: flip is_favorite + bump updated_at ---
            if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
                item.is_favorite = !item.is_favorite;
                item.updated_at = chrono::Utc::now();
            }
        }
    }

    /// Delete a single item and record deletion tombstone for sync.
    ///
    /// # Tombstones (sync)
    /// - Records `deleted_items` tombstone with content_hash, timestamp, device_name
    /// - Sets `sync_dirty = true`
    ///
    /// # Side effects
    /// - Removes item from `self.items`
    /// - Removes id from `self.selected_ids`
    pub fn delete_item(&mut self, id: i64) {
        // Read item first to get content_hash for tombstone
        if let Ok(Some(item)) = self.db.get_by_id(id) {
            let hash = item.content_hash;
            let now = chrono::Utc::now().to_rfc3339();
            let device = crate::services::backends::local_folder::hostname();

            if let Err(e) = self.db.delete_item(id) {
                log::error!("delete_item({id}): {e}");
                return;
            }

            // Record deletion tombstone only for items in sync scope.
            // Image/file items are never synced; unfavorited items in
            // favorites-only mode have already left the sync scope via
            // the unfavorite tombstone.
            if self.should_mark_sync_dirty(&item) {
                if let Err(e) = self.db.record_item_deletion(hash, &now, &device) {
                    log::error!("record_item_deletion({hash}): {e}");
                }
                self.sync_dirty.store(true, Ordering::SeqCst);
            }
        } else {
            log::warn!("delete_item({id}): item not found");
            return;
        }

        // --- Remove from in-memory items and selection ---
        self.items.retain(|it| it.id != id);
        self.selected_ids.retain(|&sid| sid != id);
    }

    /// Batch toggle favorite on all selected items.
    /// Loops selected_ids, applies the same toggle + tombstone logic per item.
    pub fn batch_toggle_favorite(&mut self) {
        let needs_full_refresh = self.filters.is_favorites_active();
        let now = chrono::Utc::now().to_rfc3339();
        let device = crate::services::backends::local_folder::hostname();
        let updated_at = chrono::Utc::now();

        let ids: Vec<i64> = self.selected_ids.clone();
        for &id in &ids {
            let item = match self.db.get_by_id(id) {
                Ok(Some(item)) => item,
                Ok(None) => {
                    log::warn!("batch toggle_favorite({id}): item not found");
                    continue;
                }
                Err(e) => {
                    log::error!("batch get_by_id({id}): {e}");
                    continue;
                }
            };
            let was_fav = item.is_favorite;
            let hash = item.content_hash;

            if let Err(e) = self.db.toggle_favorite(id) {
                log::error!("batch toggle_favorite({id}): {e}");
                continue;
            }

            if was_fav {
                if let Err(e) = self.db.record_unfavorite(hash, &now, &device) {
                    log::error!("batch record_unfavorite({hash}): {e}");
                }
            } else if let Err(e) = self.db.remove_unfavorite(hash) {
                log::error!("batch remove_unfavorite({hash}): {e}");
            }
        }

        if needs_full_refresh {
            self.reload_items();
            self.clear_selection();
        } else {
            // Incremental update: flip is_favorite + bump updated_at for each
            for id in &ids {
                if let Some(item) = self.items.iter_mut().find(|it| &it.id == id) {
                    item.is_favorite = !item.is_favorite;
                    item.updated_at = updated_at;
                }
            }
        }

        self.sync_dirty.store(true, Ordering::SeqCst);
    }

    /// Batch delete all selected items.
    /// Records deletion tombstones only for items in sync scope.
    pub fn batch_delete(&mut self) {
        let now = chrono::Utc::now().to_rfc3339();
        let device = crate::services::backends::local_folder::hostname();

        // --- Collect hashes only for items in sync scope. ---
        let mut hashes: Vec<u64> = Vec::with_capacity(self.selected_ids.len());
        let mut has_sync_item = false;
        for &id in &self.selected_ids {
            if let Ok(Some(item)) = self.db.get_by_id(id) {
                let in_sync = self.should_mark_sync_dirty(&item);
                if in_sync {
                    has_sync_item = true;
                }
                match self.db.delete_item(id) {
                    Ok(_) => {
                        if in_sync {
                            hashes.push(item.content_hash);
                        }
                    }
                    Err(e) => log::error!("batch delete_item({id}): {e}"),
                }
            }
        }

        // Record tombstones for sync
        for h in &hashes {
            if let Err(e) = self.db.record_item_deletion(*h, &now, &device) {
                log::error!("batch record_item_deletion({h}): {e}");
            }
        }

        if has_sync_item {
            self.sync_dirty.store(true, Ordering::SeqCst);
        }

        // --- Remove from in-memory items ---
        let ids: Vec<i64> = self.selected_ids.drain(..).collect();
        self.items.retain(|it| !ids.contains(&it.id));
    }
}

fn open_system_target(target: &str) {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;

        let operation: Vec<u16> = "open\0".encode_utf16().collect();
        let target_utf16: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let ret = ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                target_utf16.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOW,
            );
            // ShellExecuteW returns a value > 32 on success; <= 32 is an error.
            if (ret as isize) <= 32 {
                log::error!(
                    "ShellExecuteW failed for '{}': error code {}",
                    target,
                    ret as isize,
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(target).spawn();
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(target).spawn();
    }
}

fn reveal_file_location(path: &str) {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;

        let operation: Vec<u16> = "open\0".encode_utf16().collect();
        let explorer: Vec<u16> = "explorer\0".encode_utf16().collect();
        let arg = format!("/select,\"{}\"", path);
        let arg_utf16: Vec<u16> = arg.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                explorer.as_ptr(),
                arg_utf16.as_ptr(),
                std::ptr::null(),
                SW_SHOW,
            );
        }
    }

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Some(parent) = std::path::Path::new(path).parent() {
            let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
        }
    }
}

/// Extract body content from an HTML string for merging.
///
/// If the HTML has `<body>…</body>` tags, returns the inner content;
/// otherwise returns the input as-is.
fn extract_html_body(html: &str) -> String {
    let lower = html.to_lowercase();
    if let Some(body_start) = lower.find("<body") {
        if let Some(tag_end) = html[body_start..].find('>') {
            let content_start = body_start + tag_end + 1;
            if let Some(body_end) = lower[content_start..].find("</body>") {
                return html[content_start..content_start + body_end]
                    .trim()
                    .to_string();
            }
        }
    }
    html.trim().to_string()
}

/// Concatenate the `full_text` of all items (no separator).
fn item_texts_for_merge(items: &[&ClipboardItem]) -> String {
    items.iter().map(|item| item.full_text.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db::Database;
    use crate::core::filters::ClipboardFilters;
    use crate::core::settings::AppSettings;
    use crate::core::types::{ClipboardItem, ContentType, RichData};
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    fn make_item(
        id: i64,
        content_type: ContentType,
        is_favorite: bool,
        full_text: &str,
    ) -> ClipboardItem {
        ClipboardItem {
            id,
            content_type,
            full_text: full_text.to_string(),
            content_hash: 0x100 + id as u64,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            image_path: String::new(),
            image_width: 0,
            image_height: 0,
            rich_data: String::new(),
            file_data: String::new(),
            is_favorite,
            note: String::new(),
            source_app_name: String::new(),
            source_app_icon: String::new(),
            size: full_text.len() as i64,
            tags: Vec::new(),
            meta_type: String::new(),
        }
    }

    fn test_state() -> (AppState, Arc<AtomicBool>) {
        let db = Database::open(":memory:").unwrap();
        let dirty = Arc::new(AtomicBool::new(false));
        let settings = AppSettings {
            db_path: ":memory:".to_string(),
            ..Default::default()
        };
        let state = AppState {
            settings,
            db,
            items: Vec::new(),
            tags: Vec::new(),
            filters: ClipboardFilters::default(),
            selected_ids: Vec::new(),
            editing_tag_id: -1,
            editing_tag_name: String::new(),
            editing_tag_color: "#3B82F6".into(),
            editing_item_id: -1,
            editing_item: None,
            batch_pasting: Arc::new(AtomicBool::new(false)),
            skip_next: Arc::new(AtomicBool::new(false)),
            sync_dirty: dirty.clone(),
            bitmap_paste_finished: Arc::new(AtomicBool::new(false)),
            toast_message: None,
            toast_is_warning: false,
            foreground_app_name: String::new(),
            foreground_window_title: String::new(),
            foreground_app_icon_base64: String::new(),
            hotkey_recording: false,
            recording_quick_hotkey: false,
            sync: SyncState::default(),
            update_available: None,
            update_phase: UpdatePhase::Idle,
        };
        (state, dirty)
    }

    // ── should_mark_sync_dirty ──────────────────────────────────────

    #[test]
    fn dirty_image_type_always_false() {
        let (state, _dirty) = test_state();
        let item = make_item(1, ContentType::Image, false, "img.png");
        assert!(!state.should_mark_sync_dirty(&item));
    }

    #[test]
    fn dirty_file_type_always_false() {
        let (state, _dirty) = test_state();
        let item = make_item(1, ContentType::File, false, "doc.pdf");
        assert!(!state.should_mark_sync_dirty(&item));
    }

    #[test]
    fn dirty_favorites_only_non_fav_false() {
        let (mut state, _dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let item = make_item(1, ContentType::PlainText, false, "hello");
        assert!(!state.should_mark_sync_dirty(&item));
    }

    #[test]
    fn dirty_favorites_only_fav_true() {
        let (mut state, _dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let item = make_item(1, ContentType::PlainText, true, "hello");
        assert!(state.should_mark_sync_dirty(&item));
    }

    #[test]
    fn dirty_normal_mode_non_fav_true() {
        let (mut state, _dirty) = test_state();
        state.settings.sync_favorites_only = false;
        let item = make_item(1, ContentType::PlainText, false, "hello");
        assert!(state.should_mark_sync_dirty(&item));
    }

    #[test]
    fn dirty_image_favorite_still_false() {
        let (mut state, _dirty) = test_state();
        // Image is never synced, regardless of favorite status or sync mode
        state.settings.sync_favorites_only = false;
        let item = make_item(1, ContentType::Image, true, "img.png");
        assert!(!state.should_mark_sync_dirty(&item));
    }

    // ── save_edited_item sync_dirty ────────────────────────────────

    #[test]
    fn save_edited_item_sets_dirty_in_normal_mode() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = false;
        // Insert item into DB and in-memory list
        let item = make_item(1, ContentType::PlainText, false, "original");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        let ok = state.save_edited_item(1, "edited", "plain");
        assert!(ok);
        assert!(
            dirty.load(Ordering::SeqCst),
            "normal mode: should set dirty"
        );
    }

    #[test]
    fn save_edited_item_sets_dirty_for_favorite_in_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let item = make_item(1, ContentType::PlainText, true, "original");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        let ok = state.save_edited_item(1, "edited", "plain");
        assert!(ok);
        assert!(
            dirty.load(Ordering::SeqCst),
            "favorite item in fav-only mode: should set dirty"
        );
    }

    #[test]
    fn save_edited_item_skips_dirty_for_non_favorite_in_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let item = make_item(1, ContentType::PlainText, false, "original");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        let ok = state.save_edited_item(1, "edited", "plain");
        assert!(ok);
        assert!(
            !dirty.load(Ordering::SeqCst),
            "non-favorite item in fav-only mode: should NOT set dirty"
        );
    }

    // ── toggle_item_tag sync_dirty ─────────────────────────────────

    fn setup_tag(state: &mut AppState) -> i64 {
        // Create a tag
        state.db.create_tag("test-tag", "#FF0000").unwrap()
    }

    #[test]
    fn keyword_search_scans_all_records_and_matches_rich_data_and_tags() {
        let (mut state, _dirty) = test_state();
        let now = chrono::Utc::now();

        for i in 0..205 {
            let mut item = make_item(i + 1, ContentType::PlainText, false, &format!("noise {i}"));
            item.created_at = now - chrono::Duration::seconds(i);
            item.updated_at = item.created_at;
            state.db.upsert(&item).unwrap();
        }

        let mut text_item = make_item(300, ContentType::PlainText, false, "buried plain match");
        text_item.created_at = now - chrono::Duration::days(10);
        text_item.updated_at = text_item.created_at;
        state.db.upsert(&text_item).unwrap();

        let mut ocr_item = make_item(301, ContentType::Image, false, "screenshot");
        ocr_item.rich_data = RichData {
            ocr_text: Some("buried ocr match".to_string()),
            ..Default::default()
        }
        .to_json();
        ocr_item.created_at = now - chrono::Duration::days(11);
        ocr_item.updated_at = ocr_item.created_at;
        state.db.upsert(&ocr_item).unwrap();

        let mut tagged_item = make_item(302, ContentType::PlainText, false, "tagged item");
        tagged_item.created_at = now - chrono::Duration::days(12);
        tagged_item.updated_at = tagged_item.created_at;
        state.db.upsert(&tagged_item).unwrap();
        let tagged_id = state
            .db
            .get_by_hash(tagged_item.content_hash)
            .unwrap()
            .unwrap()
            .id;
        let tag_id = state.db.create_tag("buried tag match", "#FF0000").unwrap();
        state.db.add_item_tag(tagged_id, tag_id).unwrap();

        state.filters.set_keyword("buried");
        state.reload_items();

        let texts: Vec<&str> = state
            .items
            .iter()
            .map(|item| item.full_text.as_str())
            .collect();
        assert!(texts.contains(&"buried plain match"));
        assert!(texts.contains(&"screenshot"));
        assert!(texts.contains(&"tagged item"));
    }

    #[test]
    fn keyword_search_requires_all_space_separated_terms() {
        let (mut state, _dirty) = test_state();
        state
            .db
            .upsert(&make_item(
                1,
                ContentType::PlainText,
                false,
                "railway ticket order",
            ))
            .unwrap();
        state
            .db
            .upsert(&make_item(
                2,
                ContentType::PlainText,
                false,
                "railway notice",
            ))
            .unwrap();

        state.filters.set_keyword("railway order");
        state.reload_items();

        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].full_text, "railway ticket order");
    }

    #[test]
    fn keyword_search_matches_cached_page_title() {
        let (mut state, _dirty) = test_state();
        let mut item = make_item(1, ContentType::PlainText, false, "https://example.com/a");
        item.meta_type = "link".to_string();
        item.rich_data = RichData {
            page_title: Some("Railway Order Details".to_string()),
            ..Default::default()
        }
        .to_json();
        state.db.upsert(&item).unwrap();

        state.filters.set_keyword("order details");
        state.reload_items();

        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].full_text, "https://example.com/a");
    }

    #[test]
    fn keyword_search_keeps_match_after_preview_truncation() {
        let (mut state, _dirty) = test_state();
        let content = format!("{} tailmatch", "x".repeat(LIST_FULL_TEXT_LIMIT + 512));
        let item = make_item(1, ContentType::PlainText, false, &content);
        state.db.upsert(&item).unwrap();

        state.filters.set_keyword("tailmatch");
        state.reload_items();

        assert_eq!(state.items.len(), 1);
        assert!(state.items[0].full_text.len() <= LIST_FULL_TEXT_LIMIT);
    }

    #[test]
    fn list_reload_keeps_preview_light_and_db_full_item_intact() {
        let (mut state, _dirty) = test_state();
        let full_text = "x".repeat(12_000);
        let html = format!("<p>{}</p>", "h".repeat(8_000));
        let mut item = make_item(1, ContentType::RichText, false, &full_text);
        item.meta_type = "html".to_string();
        item.rich_data = RichData {
            html: Some(html.clone()),
            page_title: Some("t".repeat(3_000)),
            ..Default::default()
        }
        .to_json();
        item.note = "n".repeat(3_000);
        item.source_app_name = "Example".to_string();
        item.source_app_icon = "a".repeat(10_000);
        let hash = item.content_hash;
        state.db.upsert(&item).unwrap();

        state.reload_items();

        assert_eq!(state.items.len(), 1);
        let preview = &state.items[0];
        assert!(preview.full_text.len() <= LIST_FULL_TEXT_LIMIT);
        assert!(preview.note.len() <= LIST_NOTE_LIMIT);
        assert_eq!(preview.source_app_icon.len(), 10_000);
        let rich_preview = RichData::from_json(&preview.rich_data);
        assert!(rich_preview.html.unwrap().len() <= LIST_RICH_HTML_LIMIT);
        assert!(rich_preview.page_title.unwrap().len() <= LIST_RICH_AUX_LIMIT);

        let full = state.db.get_by_hash(hash).unwrap().unwrap();
        assert_eq!(full.full_text.len(), full_text.len());
        assert_eq!(RichData::from_json(&full.rich_data).html.unwrap(), html);
        assert_eq!(full.source_app_icon.len(), 10_000);
    }

    #[test]
    fn keyword_search_html_uses_visible_text_not_markup_attributes() {
        let (mut state, _dirty) = test_state();
        let mut item = make_item(1, ContentType::RichText, false, "plain fallback");
        item.meta_type = "html".to_string();
        item.rich_data = RichData {
            html: Some(r#"<div data-key="hidden">plain visible text</div>"#.to_string()),
            ..Default::default()
        }
        .to_json();
        state.db.upsert(&item).unwrap();

        state.filters.set_keyword("key");
        state.reload_items();
        assert_eq!(state.items.len(), 0);

        state.filters.set_keyword("visible");
        state.reload_items();
        assert_eq!(state.items.len(), 1);
    }

    #[test]
    fn keyword_search_html_full_text_ignores_markup_attributes() {
        let (mut state, _dirty) = test_state();
        let mut item = make_item(
            1,
            ContentType::RichText,
            false,
            r#"<div data-key="api">plain visible text</div>"#,
        );
        item.meta_type = "html".to_string();
        state.db.upsert(&item).unwrap();

        state.filters.set_keyword("api");
        state.reload_items();

        assert_eq!(state.items.len(), 0);
    }

    #[test]
    fn keyword_search_html_full_text_matches_visible_substring() {
        let (mut state, _dirty) = test_state();
        let mut item = make_item(
            1,
            ContentType::RichText,
            false,
            r#"<div><span style="color:#bbbebf"> rapid=</span><span style="color:#569cd6">false</span></div>"#,
        );
        item.meta_type = "html".to_string();
        state.db.upsert(&item).unwrap();

        state.filters.set_keyword("api");
        state.reload_items();

        assert_eq!(state.items.len(), 1);
    }

    #[test]
    fn keyword_search_html_matches_visible_text_inside_rich_data() {
        let (mut state, _dirty) = test_state();
        let mut item = make_item(1, ContentType::RichText, false, "plain fallback");
        item.meta_type = "html".to_string();
        item.rich_data = RichData {
            html: Some(r#"<div><span style="color:#ff00aa">API Key</span></div>"#.to_string()),
            ..Default::default()
        }
        .to_json();
        state.db.upsert(&item).unwrap();

        state.filters.set_keyword("key");
        state.reload_items();

        assert_eq!(state.items.len(), 1);
    }

    #[test]
    fn keyword_search_matches_non_ascii_rich_text_outside_full_text() {
        let (mut state, _dirty) = test_state();
        let mut item = make_item(1, ContentType::RichText, false, "plain preview");
        item.rich_data = RichData {
            html: Some("<p>富文本命中</p>".to_string()),
            ..Default::default()
        }
        .to_json();
        state.db.upsert(&item).unwrap();

        state.filters.set_keyword("富文本");
        state.reload_items();

        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].full_text, "plain preview");
    }

    #[test]
    fn keyword_search_matches_note_with_pinyin_and_initials() {
        let (mut state, _dirty) = test_state();
        let item = make_item(1, ContentType::PlainText, false, "普通正文内容");
        state.db.upsert(&item).unwrap();
        let item_id = state.db.get_by_hash(item.content_hash).unwrap().unwrap().id;
        // 模拟真实流程：通过 update_note 写入备注
        state.db.update_note(item_id, "工作计划").unwrap();
        state.reload_items();

        // 直接文本匹配
        state.filters.set_keyword("工作");
        state.reload_items();
        assert_eq!(state.items.len(), 1, "直接文本匹配失败");

        // 全拼匹配: gongzuo
        state.filters.set_keyword("gongzuo");
        state.reload_items();
        assert_eq!(state.items.len(), 1, "全拼匹配失败");

        // 首字母匹配: gzjh
        state.filters.set_keyword("gzjh");
        state.reload_items();
        assert_eq!(state.items.len(), 1, "首字母全匹配失败");

        // 部分首字母匹配: gz
        state.filters.set_keyword("gz");
        state.reload_items();
        assert_eq!(state.items.len(), 1, "部分首字母匹配失败");

        // 确认备注字段中的中文匹配不依赖正文
        state.filters.set_keyword("计划");
        state.reload_items();
        assert_eq!(state.items.len(), 1, "备注中文匹配失败");

        // 确保未写入备注的不匹配
        state.filters.set_keyword("xyznotexist");
        state.reload_items();
        assert_eq!(state.items.len(), 0, "不应匹配的关键词却匹配了");
    }

    #[test]
    fn toggle_item_tag_sets_dirty_normal_mode() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = false;
        let tag_id = setup_tag(&mut state);
        let item = make_item(1, ContentType::PlainText, false, "hello");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.toggle_item_tag(1, tag_id);
        assert!(
            dirty.load(Ordering::SeqCst),
            "normal mode: should set dirty"
        );
    }

    #[test]
    fn toggle_item_tag_sets_dirty_for_favorite_in_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let tag_id = setup_tag(&mut state);
        let item = make_item(1, ContentType::PlainText, true, "hello");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.toggle_item_tag(1, tag_id);
        assert!(
            dirty.load(Ordering::SeqCst),
            "favorite item in fav-only mode: should set dirty"
        );
    }

    #[test]
    fn toggle_item_tag_skips_dirty_for_non_favorite_in_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let tag_id = setup_tag(&mut state);
        let item = make_item(1, ContentType::PlainText, false, "hello");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.toggle_item_tag(1, tag_id);
        assert!(
            !dirty.load(Ordering::SeqCst),
            "non-favorite item in fav-only mode: should NOT set dirty"
        );
    }

    // ── clear_item_tags sync_dirty ─────────────────────────────────

    #[test]
    fn clear_item_tags_skips_dirty_for_non_favorite_in_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let tag_id = setup_tag(&mut state);
        let item = make_item(1, ContentType::PlainText, false, "hello");
        state.db.upsert(&item).unwrap();
        state.db.add_item_tag(1, tag_id).unwrap();
        state.items.push(item);

        state.clear_item_tags(1);
        assert!(
            !dirty.load(Ordering::SeqCst),
            "non-favorite item in fav-only mode: should NOT set dirty"
        );
    }

    #[test]
    fn clear_item_tags_sets_dirty_for_favorite_in_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let tag_id = setup_tag(&mut state);
        let item = make_item(1, ContentType::PlainText, true, "hello");
        state.db.upsert(&item).unwrap();
        state.db.add_item_tag(1, tag_id).unwrap();
        state.items.push(item);

        state.clear_item_tags(1);
        assert!(
            dirty.load(Ordering::SeqCst),
            "favorite item in fav-only mode: should set dirty"
        );
    }

    // ── toggle_favorite ALWAYS sets dirty ──────────────────────────

    #[test]
    fn toggle_favorite_always_sets_dirty_even_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let item = make_item(1, ContentType::PlainText, false, "hello");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.toggle_favorite(1);
        assert!(
            dirty.load(Ordering::SeqCst),
            "toggle_favorite should always set dirty (tombstones)"
        );
    }

    #[test]
    fn delete_item_skips_tombstone_outside_sync_scope() {
        // favorites_only=true, item not favorited → outside sync scope
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let item = make_item(1, ContentType::PlainText, false, "hello");
        let hash = item.content_hash;
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.delete_item(1);
        assert!(
            !state.db.is_item_tombstoned(hash).unwrap(),
            "delete outside sync scope should NOT record tombstone"
        );
        assert!(
            !dirty.load(Ordering::SeqCst),
            "delete outside sync scope should NOT set dirty (no tombstone needed)"
        );
    }

    #[test]
    fn delete_item_records_tombstone_in_sync_scope() {
        // Normal mode: PlainText item → in sync scope
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = false;
        let item = make_item(1, ContentType::PlainText, false, "hello");
        let hash = item.content_hash;
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.delete_item(1);
        assert!(
            state.db.is_item_tombstoned(hash).unwrap(),
            "delete in sync scope should record tombstone"
        );
        assert!(
            dirty.load(Ordering::SeqCst),
            "delete in sync scope should set dirty (tombstone)"
        );
    }

    #[test]
    fn delete_item_skips_tombstone_for_image() {
        // Image items are never synced
        let (mut state, dirty) = test_state();
        let item = make_item(1, ContentType::Image, false, "image_data");
        let hash = item.content_hash;
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.delete_item(1);
        assert!(
            !state.db.is_item_tombstoned(hash).unwrap(),
            "delete image should NOT record tombstone"
        );
        assert!(
            !dirty.load(Ordering::SeqCst),
            "delete image should NOT set dirty"
        );
    }

    #[test]
    fn batch_delete_records_tombstones_only_for_sync_scope() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;

        let favorite = make_item(1, ContentType::PlainText, true, "favorite");
        let non_favorite = make_item(2, ContentType::PlainText, false, "non-favorite");
        let image = make_item(3, ContentType::Image, true, "image_data");
        let favorite_hash = favorite.content_hash;
        let non_favorite_hash = non_favorite.content_hash;
        let image_hash = image.content_hash;

        state.db.upsert(&favorite).unwrap();
        state.db.upsert(&non_favorite).unwrap();
        state.db.upsert(&image).unwrap();
        let favorite_id = state.db.get_by_hash(favorite_hash).unwrap().unwrap().id;
        let non_favorite_id = state.db.get_by_hash(non_favorite_hash).unwrap().unwrap().id;
        let image_id = state.db.get_by_hash(image_hash).unwrap().unwrap().id;
        state.db.set_favorite(favorite_id, true).unwrap();
        state.db.set_favorite(image_id, true).unwrap();
        state.items.extend([favorite, non_favorite, image]);
        state.selected_ids = vec![favorite_id, non_favorite_id, image_id];

        state.batch_delete();

        assert!(state.db.is_item_tombstoned(favorite_hash).unwrap());
        assert!(!state.db.is_item_tombstoned(non_favorite_hash).unwrap());
        assert!(!state.db.is_item_tombstoned(image_hash).unwrap());
        assert!(
            dirty.load(Ordering::SeqCst),
            "batch delete should set dirty when at least one selected item is in sync scope"
        );
    }

    #[test]
    fn batch_delete_skips_dirty_and_tombstones_when_all_items_outside_sync_scope() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;

        let non_favorite = make_item(1, ContentType::PlainText, false, "non-favorite");
        let image = make_item(2, ContentType::Image, true, "image_data");
        let non_favorite_hash = non_favorite.content_hash;
        let image_hash = image.content_hash;

        state.db.upsert(&non_favorite).unwrap();
        state.db.upsert(&image).unwrap();
        let non_favorite_id = state.db.get_by_hash(non_favorite_hash).unwrap().unwrap().id;
        let image_id = state.db.get_by_hash(image_hash).unwrap().unwrap().id;
        state.db.set_favorite(image_id, true).unwrap();
        state.items.extend([non_favorite, image]);
        state.selected_ids = vec![non_favorite_id, image_id];

        state.batch_delete();

        assert!(!state.db.is_item_tombstoned(non_favorite_hash).unwrap());
        assert!(!state.db.is_item_tombstoned(image_hash).unwrap());
        assert!(
            !dirty.load(Ordering::SeqCst),
            "batch delete outside sync scope should NOT set dirty"
        );
    }

    #[test]
    fn merge_selected_items_rejects_non_text_items() {
        let (mut state, _dirty) = test_state();
        let text = make_item(1, ContentType::PlainText, false, "text");
        let image = make_item(2, ContentType::Image, false, "image_data");
        let text_hash = text.content_hash;
        let image_hash = image.content_hash;

        state.db.upsert(&text).unwrap();
        state.db.upsert(&image).unwrap();
        let text_id = state.db.get_by_hash(text_hash).unwrap().unwrap().id;
        let image_id = state.db.get_by_hash(image_hash).unwrap().unwrap().id;
        state.reload_items();
        state.selected_ids = vec![text_id, image_id];

        assert!(!state.can_merge_selected_items());
        assert!(state.merge_selected_items().is_none());
        state.reload_items();
        assert_eq!(state.items.len(), 2);
    }

    #[test]
    fn merge_selected_items_creates_new_entry_even_when_text_already_exists() {
        let (mut state, _dirty) = test_state();
        let existing = make_item(1, ContentType::PlainText, false, "ab");
        let first = make_item(2, ContentType::PlainText, false, "a");
        let second = make_item(3, ContentType::PlainText, false, "b");
        let existing_hash = existing.content_hash;
        let first_hash = first.content_hash;
        let second_hash = second.content_hash;

        state.db.upsert(&existing).unwrap();
        state.db.upsert(&first).unwrap();
        state.db.upsert(&second).unwrap();
        let existing_id = state.db.get_by_hash(existing_hash).unwrap().unwrap().id;
        let first_id = state.db.get_by_hash(first_hash).unwrap().unwrap().id;
        let second_id = state.db.get_by_hash(second_hash).unwrap().unwrap().id;
        state.reload_items();
        state.selected_ids = vec![first_id, second_id];

        assert!(state.can_merge_selected_items());
        let merged_id = state.merge_selected_items().unwrap();

        assert_ne!(merged_id, existing_id);
        let merged = state.db.get_by_id(merged_id).unwrap().unwrap();
        assert_eq!(merged.full_text, "ab");
        assert_eq!(
            state
                .items
                .iter()
                .filter(|item| item.full_text == "ab")
                .count(),
            2
        );
    }

    // ── update_note sync_dirty ─────────────────────────────────────

    #[test]
    fn update_note_sets_dirty_in_normal_mode() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = false;
        let item = make_item(1, ContentType::PlainText, true, "hello");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.update_note(1, "new note");
        assert!(
            dirty.load(Ordering::SeqCst),
            "normal mode: should set dirty"
        );
    }

    #[test]
    fn update_note_skips_dirty_for_non_favorite_in_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let item = make_item(1, ContentType::PlainText, false, "hello");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.update_note(1, "new note");
        assert!(
            !dirty.load(Ordering::SeqCst),
            "non-favorite in fav-only: should NOT set dirty"
        );
    }

    #[test]
    fn update_note_sets_dirty_for_favorite_in_favorites_only() {
        let (mut state, dirty) = test_state();
        state.settings.sync_favorites_only = true;
        let item = make_item(1, ContentType::PlainText, true, "hello");
        state.db.upsert(&item).unwrap();
        state.items.push(item);

        state.update_note(1, "new note");
        assert!(
            dirty.load(Ordering::SeqCst),
            "favorite in fav-only: should set dirty"
        );
    }
}
