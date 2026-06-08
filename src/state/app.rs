//! Application-wide state entity.
//!
//! `AppState` is the root entity that holds all shared application data.
//! It is created once at startup and passed to all UI components via GPUI's
//! entity subscription/observation mechanism.

use crate::core::db::Database;
use crate::core::filters::ClipboardFilters;
use crate::core::settings::AppSettings;
use crate::core::types::next_tag_color;
use crate::core::types::ClipboardItem;
use crate::core::types::ContentType;
use crate::core::types::FileData;
use crate::core::types::RichData;
use crate::core::types::TagInfo;
use clipboard_rs::{Clipboard, ClipboardContext};
use std::sync::atomic::{AtomicBool, Ordering};
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
    pub toast_message: Option<String>,
    /// Foreground app info (updated by WindowManager poll loop, consumed by hotkey settings tab).
    pub foreground_app_name: String,
    pub foreground_window_title: String,
    /// Base64-encoded PNG icon of the current foreground app.
    pub foreground_app_icon_base64: String,
    /// Whether a hotkey recording is in progress (set by settings UI, cleared by WM poll).
    pub hotkey_recording: bool,
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

        let items = db
            .load_filtered_with_tags(&ClipboardFilters::default(), query_limit, order_by)
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
            editing_item_id: -1,
            editing_item: None,
            batch_pasting: Arc::new(AtomicBool::new(false)),
            skip_next: Arc::new(AtomicBool::new(false)),
            sync_dirty: Arc::new(AtomicBool::new(false)),
            toast_message: None,
            foreground_app_name: String::new(),
            foreground_window_title: String::new(),
            foreground_app_icon_base64: String::new(),
            hotkey_recording: false,
        }
    }

    /// Reload items from database with current filters.
    pub fn reload_items(&mut self) {
        match self
            .db
            .load_filtered_with_tags(&self.filters, self.query_limit(), self.order_by())
        {
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
            let activate =
                !self.filters.is_type_active("file") && !self.filters.is_type_active("image");
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
        match self
            .db
            .update_content_with_rich_data(id, text, content_type, meta_type, &rich_data)
        {
            Ok(_) => {
                self.sync_dirty.store(true, Ordering::SeqCst);
                self.cancel_edit_item();
                self.reload_items();
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
            "link" => ("link", "", String::new()),
            "path" => ("path", "", String::new()),
            "color" => ("color", "", String::new()),
            "email" => ("plain_text", "email", String::new()),
            "phone" => ("plain_text", "phone", String::new()),
            _ => ("plain_text", "", String::new()),
        }
    }

    pub fn clear_toast(&mut self) {
        self.toast_message = None;
    }

    fn show_toast(&mut self, message: impl Into<String>) {
        self.toast_message = Some(message.into());
    }

    pub fn toggle_item_tag(&mut self, item_id: i64, tag_id: i64) {
        let has_tag = self
            .items
            .iter()
            .find(|item| item.id == item_id)
            .is_some_and(|item| item.tags.iter().any(|tag| tag.id == tag_id));
        let result = if has_tag {
            self.db.remove_item_tag(item_id, tag_id)
        } else {
            self.db.add_item_tag(item_id, tag_id)
        };
        if let Err(e) = result {
            log::error!("toggle_item_tag({item_id}, {tag_id}): {e}");
            return;
        }
        self.sync_dirty.store(true, Ordering::SeqCst);
        self.reload_items();
    }

    pub fn batch_add_tag(&mut self, ids: &[i64], tag_id: i64) {
        for &id in ids {
            if let Err(e) = self.db.add_item_tag(id, tag_id) {
                log::error!("batch_add_tag({id}, {tag_id}): {e}");
            }
        }
        self.sync_dirty.store(true, Ordering::SeqCst);
        self.reload_items();
    }

    pub fn batch_remove_tag(&mut self, ids: &[i64], tag_id: i64) {
        for &id in ids {
            if let Err(e) = self.db.remove_item_tag(id, tag_id) {
                log::error!("batch_remove_tag({id}, {tag_id}): {e}");
            }
        }
        self.sync_dirty.store(true, Ordering::SeqCst);
        self.reload_items();
    }

    pub fn clear_item_tags(&mut self, item_id: i64) {
        if let Err(e) = self.db.clear_item_tags(item_id) {
            log::error!("clear_item_tags({item_id}): {e}");
            return;
        }
        self.sync_dirty.store(true, Ordering::SeqCst);
        self.reload_items();
    }

    pub fn clear_tags_for_items(&mut self, ids: &[i64]) {
        for &id in ids {
            if let Err(e) = self.db.clear_item_tags(id) {
                log::error!("clear_tags_for_items({id}): {e}");
            }
        }
        self.sync_dirty.store(true, Ordering::SeqCst);
        self.reload_items();
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
            // Spawn on a background thread — ShellExecuteW can pump Windows
            // messages internally (DDE/COM) and deadlock if called from the
            // GPUI main thread event handler.
            std::thread::spawn(move || {
                open_system_target(&path);
            });
        }
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

        match item.content_type {
            ContentType::Link | ContentType::Path => {
                if !item.full_text.is_empty() {
                    let text = item.full_text.clone();
                    // Spawn on a background thread to avoid ShellExecuteW
                    // deadlock on the GPUI main thread (DDE/COM message pumping).
                    std::thread::spawn(move || {
                        open_system_target(&text);
                    });
                }
            }
            ContentType::File => {
                let file_data = FileData::from_json(&item.file_data);
                if let Some(first) = file_data.files.first() {
                    let path = first.path.clone();
                    // Spawn on a background thread to avoid ShellExecuteW
                    // deadlock on the GPUI main thread (DDE/COM message pumping).
                    std::thread::spawn(move || {
                        reveal_file_location(&path);
                    });
                }
            }
            _ => {}
        }
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
            self.show_toast("No QR code detected");
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
            self.show_toast("No QR code detected");
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
            // Use skip_next (not batch_pasting) — the OCR text written to
            // clipboard is internal and should be "consumed" by the listener
            // (skip one cycle + update baseline seq#) rather than recorded
            // as a new history entry. This matches the Slint-era behaviour.
            self.skip_next.store(true, Ordering::SeqCst);
            if let Ok(ctx) = ClipboardContext::new() {
                let _ = ctx.set_text(text);
            }
            crate::platform::paste::restore_paste_target();
            crate::platform::paste::paste_after_delay();
        } else {
            self.show_toast("No OCR text detected");
        }
    }

    fn handle_qr_text(&mut self, text: String) {
        if text.starts_with("http://") || text.starts_with("https://") {
            // Spawn on a background thread — open_releases_page calls
            // ShellExecuteW which can pump Windows messages internally
            // (DDE/COM) and deadlock if called from the GPUI main thread.
            let url = text;
            std::thread::spawn(move || {
                crate::services::update::open_releases_page(&url);
            });
            return;
        }
        if let Ok(ctx) = ClipboardContext::new() {
            let _ = ctx.set_text(text);
            self.show_toast("QR code content copied to clipboard");
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
                        log::warn!(
                            "batch_paste: image verification failed for item {}",
                            item.id
                        );
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(50));
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
        let was_fav = self
            .db
            .get_by_id(id)
            .ok()
            .flatten()
            .is_some_and(|item| item.is_favorite);

        if let Err(e) = self.db.toggle_favorite(id) {
            log::error!("toggle_favorite({id}): {e}");
            return;
        }

        // Tombstone management
        if was_fav {
            // Was favorited, now unfavorited — record tombstone
            if let Ok(Some(item)) = self.db.get_by_id(id) {
                let now = chrono::Utc::now().to_rfc3339();
                let device = crate::services::backends::local_folder::hostname();
                if let Err(e) = self.db.record_unfavorite(item.content_hash, &now, &device) {
                    log::error!("record_unfavorite({}): {e}", item.content_hash);
                }
            }
        } else {
            // Was unfavorited, now favorited — remove tombstone
            if let Ok(Some(item)) = self.db.get_by_id(id) {
                if let Err(e) = self.db.remove_unfavorite(item.content_hash) {
                    log::error!("remove_unfavorite({}): {e}", item.content_hash);
                }
            }
        }

        self.sync_dirty.store(true, Ordering::SeqCst);

        if needs_full_refresh {
            self.reload_items();
            self.clear_selection();
        } else {
            // Incremental update: flip is_favorite + bump updated_at
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

            // Record deletion tombstone for sync propagation
            if let Err(e) = self.db.record_item_deletion(hash, &now, &device) {
                log::error!("record_item_deletion({hash}): {e}");
            }
        } else {
            log::warn!("delete_item({id}): item not found");
            return;
        }

        self.sync_dirty.store(true, Ordering::SeqCst);

        // Remove from in-memory items and selection
        self.items.retain(|it| it.id != id);
        self.selected_ids.retain(|&sid| sid != id);
    }

    /// Batch toggle favorite on all selected items.
    /// Loops selected_ids, applies the same toggle + tombstone logic per item.
    pub fn batch_toggle_favorite(&mut self) {
        let needs_full_refresh = self.filters.is_favorites_active();
        let now = chrono::Utc::now().to_rfc3339();
        let device = crate::services::backends::local_folder::hostname();

        let ids: Vec<i64> = self.selected_ids.clone();
        for &id in &ids {
            let was_fav = self
                .db
                .get_by_id(id)
                .ok()
                .flatten()
                .is_some_and(|item| item.is_favorite);

            if let Err(e) = self.db.toggle_favorite(id) {
                log::error!("batch toggle_favorite({id}): {e}");
                continue;
            }

            if was_fav {
                if let Ok(Some(item)) = self.db.get_by_id(id) {
                    if let Err(e) = self.db.record_unfavorite(item.content_hash, &now, &device) {
                        log::error!("batch record_unfavorite({}): {e}", item.content_hash);
                    }
                }
            } else {
                if let Ok(Some(item)) = self.db.get_by_id(id) {
                    if let Err(e) = self.db.remove_unfavorite(item.content_hash) {
                        log::error!("batch remove_unfavorite({}): {e}", item.content_hash);
                    }
                }
            }
        }

        self.sync_dirty.store(true, Ordering::SeqCst);

        if needs_full_refresh {
            self.reload_items();
            self.clear_selection();
        } else {
            // Incremental update: flip is_favorite + bump updated_at for each
            for id in &ids {
                if let Some(item) = self.items.iter_mut().find(|it| &it.id == id) {
                    item.is_favorite = !item.is_favorite;
                    item.updated_at = chrono::Utc::now();
                }
            }
        }
    }

    /// Batch delete all selected items.
    /// Records deletion tombstones for each deleted item.
    pub fn batch_delete(&mut self) {
        let now = chrono::Utc::now().to_rfc3339();
        let device = crate::services::backends::local_folder::hostname();

        // Collect hashes before deleting
        let mut hashes: Vec<u64> = Vec::with_capacity(self.selected_ids.len());
        for &id in &self.selected_ids {
            if let Ok(Some(item)) = self.db.get_by_id(id) {
                hashes.push(item.content_hash);
            }
            if let Err(e) = self.db.delete_item(id) {
                log::error!("batch delete_item({id}): {e}");
            }
        }

        // Record tombstones for sync
        for h in &hashes {
            if let Err(e) = self.db.record_item_deletion(*h, &now, &device) {
                log::error!("batch record_item_deletion({h}): {e}");
            }
        }

        self.sync_dirty.store(true, Ordering::SeqCst);

        // Remove from in-memory items
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
            ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                target_utf16.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOW,
            );
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
        if let Some(parent) = std::path::Path::new(path).parent() {
            let _ = std::process::Command::new("open").arg(parent).spawn();
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Some(parent) = std::path::Path::new(path).parent() {
            let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
        }
    }
}
