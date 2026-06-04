//! Application-wide state entity.
//!
//! `AppState` is the root entity that holds all shared application data.
//! It is created once at startup and passed to all UI components via GPUI's
//! entity subscription/observation mechanism.

use crate::core::db::Database;
use crate::core::filters::ClipboardFilters;
use crate::core::settings::AppSettings;
use crate::core::types::ClipboardItem;
use crate::core::types::TagInfo;
use crate::core::types::next_tag_color;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

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
    /// Whether the main window is visible
    pub window_visible: bool,
    /// Tag editing state for TagFilterPanel overlay
    pub editing_tag_id: i64,
    pub editing_tag_name: String,
    pub editing_tag_color: String,
    /// Shared with clipboard listener — set true during batch paste
    /// to prevent recording intermediate writes (newline separators).
    pub batch_pasting: Arc<AtomicBool>,
}

impl AppState {
    /// Open the database and load initial data.
    pub fn new(settings: AppSettings) -> Self {
        let db_path = settings.resolve_db_path();
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

        let items = db.load_filtered_with_tags(&ClipboardFilters::default(), query_limit, order_by)
            .unwrap_or_else(|e| {
                log::error!("Failed to load initial items: {e}");
                Vec::new()
            });

        let tags = db.get_all_tags().unwrap_or_else(|e| {
            log::error!("Failed to load tags: {e}");
            Vec::new()
        });

        Self {
            settings,
            db,
            items,
            tags,
            filters: ClipboardFilters::default(),
            selected_ids: Vec::new(),
            window_visible: false,
            editing_tag_id: -1,
            editing_tag_name: String::new(),
            editing_tag_color: "#3B82F6".into(),
            batch_pasting: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Reload items from database with current filters.
    pub fn reload_items(&mut self) {
        match self.db.load_filtered_with_tags(&self.filters, self.query_limit(), self.order_by()) {
            Ok(items) => self.items = items,
            Err(e) => log::error!("Failed to reload items: {e}"),
        }
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
    pub fn toggle_type_filter(&mut self, type_name: &str) {
        if type_name == "file" {
            let activate = !self.filters.is_type_active("file") && !self.filters.is_type_active("image");
            for expanded in ["file", "image"] {
                let is_active = self.filters.is_type_active(expanded);
                if activate != is_active {
                    self.filters.toggle_type(expanded);
                }
            }
        } else {
            self.filters.toggle_type(type_name);
        }
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
        if let Some(pos) = self.settings.pinned_tag_ids.iter().position(|&id| id == tag_id) {
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

    /// Toggle selection of an item by ID (Ctrl+click).
    pub fn toggle_selection(&mut self, id: i64) {
        if let Some(pos) = self.selected_ids.iter().position(|&x| x == id) {
            self.selected_ids.remove(pos);
        } else {
            self.selected_ids.push(id);
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
            Ok(_) => self.reload_tags(),
            Err(e) => log::error!("Failed to create tag: {e}"),
        }
    }

    /// Delete a tag by id.
    pub fn delete_tag(&mut self, tag_id: i64) {
        match self.db.delete_tag(tag_id) {
            Ok(_) => {
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
                self.cancel_edit_tag();
                self.reload_tags();
            }
            Err(e) => log::error!("Failed to update tag: {e}"),
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
            (self.settings.max_items as usize).saturating_mul(2).max(200)
        }
    }

    /// Copy a single item to the system clipboard (no paste simulation).
    pub fn copy_item(&self, id: i64, copy_as_plain_text: bool) {
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
        crate::services::clipboard_ops::write_item_to_clipboard(&item, copy_as_plain_text);
    }

    /// Paste a single item: write to clipboard, restore focus, simulate Ctrl+V.
    pub fn paste_item(&self, id: i64, copy_as_plain_text: bool) {
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

        let is_file = item.content_type == ContentType::File;
        let expected = item.full_text.clone();
        crate::services::clipboard_ops::write_item_to_clipboard(&item, copy_as_plain_text);

        if !expected.is_empty() && !is_file {
            crate::services::clipboard_ops::verify_clipboard_content(&expected, 200);
        }
        if is_file {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        restore_paste_target();
        paste_after_delay();
    }

    /// Convert a color item from HEX to RGB and paste.
    pub fn paste_as_rgb(&self, id: i64) {
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
            let rgb_text = color.to_rgb();
            if let Ok(ctx) = clipboard_rs::ClipboardContext::new() {
                let _ = clipboard_rs::Clipboard::set_text(&ctx, rgb_text.clone());
            }
            crate::services::clipboard_ops::verify_clipboard_content(&rgb_text, 200);
            restore_paste_target();
            paste_after_delay();
        }
    }

    /// Convert a color item from RGB to HEX and paste.
    pub fn paste_as_hex(&self, id: i64) {
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
            let hex_text = color.to_css_hex();
            if let Ok(ctx) = clipboard_rs::ClipboardContext::new() {
                let _ = clipboard_rs::Clipboard::set_text(&ctx, hex_text.clone());
            }
            crate::services::clipboard_ops::verify_clipboard_content(&hex_text, 200);
            restore_paste_target();
            paste_after_delay();
        }
    }

    /// Batch paste multiple items sequentially.
    pub fn batch_paste(&self, ids: &[i64], copy_as_plain_text: bool) {
        use crate::core::types::ContentType;
        use crate::platform::paste::{paste_after_delay, paste_sync, restore_paste_target};

        // Suppress clipboard recording during batch paste to prevent
        // intermediate writes (newline separators) from being captured.
        self.batch_pasting
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let items: Vec<crate::core::types::ClipboardItem> = ids
            .iter()
            .filter_map(|&id| self.db.get_by_id(id).ok().flatten())
            .collect();

        let n = items.len();
        for (i, item) in items.iter().enumerate() {
            // Newline separator between items (not before first)
            if i > 0 {
                if let Ok(ctx) = clipboard_rs::ClipboardContext::new() {
                    let _ = clipboard_rs::Clipboard::set_text(&ctx, "\n".to_string());
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
                restore_paste_target();
                paste_sync();
                std::thread::sleep(std::time::Duration::from_millis(60));
            }

            let expected = item.full_text.clone();
            crate::services::clipboard_ops::write_item_to_clipboard(item, copy_as_plain_text);

            // Verify clipboard before pasting
            if item.content_type == ContentType::Image {
                if let Ok(meta) = std::fs::metadata(&item.image_path) {
                    let size = meta.len();
                    if !crate::services::clipboard_ops::verify_clipboard_image(size, 300) {
                        log::warn!("batch_paste: image verification failed for item {}", item.id);
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            } else if item.content_type != ContentType::File {
                if !crate::services::clipboard_ops::verify_clipboard_content(&expected, 300) {
                    log::warn!("batch_paste: text verification timed out for item {}", item.id);
                }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }

            restore_paste_target();

            if i < n - 1 {
                paste_sync();
                let delay = if item.content_type == ContentType::Image {
                    let file_size = std::fs::metadata(&item.image_path)
                        .map(|m| m.len())
                        .unwrap_or(0);
                    let size_delay = (file_size / 10_000) as u64;
                    size_delay.clamp(200, 3000)
                } else {
                    100
                };
                std::thread::sleep(std::time::Duration::from_millis(delay));
            } else {
                paste_after_delay();
            }
        }
        // Restore clipboard recording — batch paste is complete.
        self.batch_pasting
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Update the note field for a clipboard item.
    /// Writes to DB (includes updated_at) and syncs the in-memory items list.
    pub fn update_note(&mut self, id: i64, note: &str) {
        match self.db.update_note(id, note) {
            Ok(_) => {
                if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
                    item.note = note.to_string();
                    item.updated_at = chrono::Utc::now();
                }
            }
            Err(e) => log::error!("update_note({id}): {e}"),
        }
    }
}
