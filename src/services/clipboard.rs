//! Clipboard service - handles clipboard business logic

use crate::core::db::Database;
use crate::core::filters::ClipboardFilters;
use crate::core::paths::images_dir;
use crate::core::types::format_relative_time;
use crate::looper::Pollable;
use crate::platform::clipboard::ClipboardShared;
use crate::platform::favicon;
use crate::platform::file_icon;
use crate::App;
use crate::ClipboardEntry;
use base64::Engine;
use slint::{Image, Model, ModelRc, SharedString, VecModel};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone)]
pub struct ClipboardService {
    shared: ClipboardShared,
    db: Arc<Mutex<Database>>,
    app: slint::Weak<App>,
    model: Rc<VecModel<ClipboardEntry>>,
    sort_by_created: bool,
    copy_as_plain_text: bool,
    filters: ClipboardFilters,
    selected_ids: Vec<i32>,
    anchor_id: i32,
    sync_dirty: Arc<AtomicBool>,
    needs_model_refresh: Arc<AtomicBool>,
    max_items: usize,
    /// Cache of ClipboardEntry keyed by item id, with updated_at for invalidation.
    entry_cache: HashMap<i64, (String, ClipboardEntry)>,
}

impl ClipboardService {
    pub fn new(
        shared: ClipboardShared,
        db: Arc<Mutex<Database>>,
        app: slint::Weak<App>,
        sync_dirty: Arc<AtomicBool>,
        needs_model_refresh: Arc<AtomicBool>,
    ) -> Self {
        let model: Rc<VecModel<ClipboardEntry>> = Rc::new(VecModel::default());
        let service = Self {
            shared,
            db,
            app,
            model: model.clone(),
            sort_by_created: false,
            copy_as_plain_text: false,
            filters: ClipboardFilters::default(),
            selected_ids: Vec::new(),
            anchor_id: -1,
            sync_dirty,
            needs_model_refresh,
            max_items: 100, // 历史默认值
            entry_cache: HashMap::new(),
        };
        if let Some(app) = service.app.upgrade() {
            app.set_clipboard_items(ModelRc::from(model));
            app.set_item_count(0);
        }
        service
    }

    /// Toggle a content type filter and reload from database
    pub fn toggle_filter_and_refresh(&mut self, filter_type: &str) {
        self.filters.toggle_type(filter_type);
        self.refresh_with_current_filter();
    }

    /// Toggle the combined "file" filter (image + file types) atomically.
    pub fn toggle_file_filter_and_refresh(&mut self) {
        self.toggle_filter_pair_and_refresh("file", "image");
    }

    /// Toggle the combined "链接" filter (link + path types) atomically.
    pub fn toggle_link_filter_and_refresh(&mut self) {
        self.toggle_filter_pair_and_refresh("link", "path");
    }

    fn toggle_filter_pair_and_refresh(&mut self, a: &str, b: &str) {
        let a_active = self.filters.is_type_active(a);
        let b_active = self.filters.is_type_active(b);
        if a_active && b_active {
            self.filters.toggle_type(a);
            self.filters.toggle_type(b);
        } else {
            if !a_active { self.filters.toggle_type(a); }
            if !b_active { self.filters.toggle_type(b); }
        }
        self.refresh_with_current_filter();
    }

    /// Toggle favorites filter and reload from database
    pub fn toggle_favorites_filter_and_refresh(&mut self) {
        self.filters.toggle_favorites_only();
        self.refresh_with_current_filter();
    }

    /// Clear all filters and reload
    pub fn clear_filters(&mut self) {
        self.filters.clear_all();
        self.refresh_with_current_filter();
    }

    /// Set keyword search and reload from database
    pub fn set_keyword(&mut self, keyword: &str) {
        self.filters.set_keyword(keyword);
        self.refresh_with_current_filter();
    }

    /// Clear keyword search and reload
    pub fn clear_keyword(&mut self) {
        self.filters.set_keyword("");
        self.refresh_with_current_filter();
    }

    pub fn refresh_with_current_filter(&mut self) {
        self.model.clear();
        // 筛选变化时清除选中状态
        self.selected_ids.clear();
        self.anchor_id = -1;
        let order_by = if self.sort_by_created { "created_at" } else { "updated_at" };
        let limit = if self.max_items == 0 { usize::MAX } else { self.max_items };
        let items = self.db.lock().expect("db lock poisoned")
            .load_filtered_with_tags(&self.filters, limit, order_by)
            .unwrap_or_default();
        let entries: Vec<ClipboardEntry> = items.iter()
            .map(|item| self.get_or_create_entry(item))
            .collect();
        for entry in entries {
            self.model.push(entry);
        }
        if let Some(app) = self.app.upgrade() {
            app.set_item_count(self.model.row_count() as i32);
            app.set_selected_count(0);
        }
    }

    /// Set sort mode and refresh (full reload)
    pub fn set_sort_and_refresh(&mut self, sort_by_created: bool) {
        self.sort_by_created = sort_by_created;
        self.refresh_with_current_filter();
    }

    /// Toggle copy-as-plain-text mode (no reload needed — applies to incoming items)
    pub fn set_copy_as_plain_text(&mut self, enabled: bool) {
        self.copy_as_plain_text = enabled;
    }

    /// Set max items limit (0 = unlimited)
    pub fn set_max_items(&mut self, max_items: u32) {
        self.max_items = max_items as usize;
    }

    /// Load initial items from database (model starts empty; delegate to unified refresh)
    pub fn load_initial(&mut self) {
        self.refresh_with_current_filter();
    }

    /// Refresh a single row by id (e.g., after copying to update timestamp)
    pub fn refresh_row(&mut self, id: i32) {
        let item = {
            if let Ok(db) = self.db.lock() {
                db.get_by_id_with_tags(id as i64).unwrap_or(None)
            } else {
                None
            }
        };
        if let Some(item) = item {
            let entry = self.get_or_create_entry(&item);
            for i in 0..self.model.row_count() {
                if let Some(e) = self.model.row_data(i) {
                    if e.id == id {
                        self.model.set_row_data(i, entry);
                        break;
                    }
                }
            }
        }
    }

    /// Check if a filter type is active
    pub fn is_filter_active(&self, filter_type: &str) -> bool {
        self.filters.is_type_active(filter_type)
    }

    /// Check if favorites filter is active
    pub fn is_favorites_filter_active(&self) -> bool {
        self.filters.is_favorites_active()
    }

    // ── Tag methods ──

    pub fn has_tag_filters(&self) -> bool {
        self.filters.has_tag_filters()
    }

    /// Toggle a tag filter and full reload
    pub fn toggle_tag_filter_and_refresh(&mut self, tag_id: i64) {
        self.filters.toggle_tag(tag_id);
        self.refresh_with_current_filter();
    }

    /// Clear all tag filters and full reload
    /// Load all tags into the shared model with filter-checked state
    pub fn load_all_tags_for_filter(&self) {
        let tags = self.db.lock().expect("db lock poisoned")
            .get_all_tags()
            .unwrap_or_default();
        let model: Vec<crate::TagItem> = tags.iter().map(|t| {
            crate::TagItem {
                id: t.id as i32,
                name: slint::SharedString::from(t.name.clone()),
                color: hex_to_slint_color(&t.color),
                checked: self.filters.is_tag_active(t.id),
            }
        }).collect();
        if let Some(app) = self.app.upgrade() {
            app.set_all_tags(slint::ModelRc::from(model.as_slice()));
        }
    }

    /// Load all tags for the picker panel with item-checked state (sorted by recency)
    pub fn load_all_tags_for_picker(&self, item_id: i32) {
        let (mut tags, item_tags) = {
            let db = self.db.lock().expect("db lock poisoned");
            (db.get_all_tags().unwrap_or_default(), db.get_tags_for_item(item_id as i64).unwrap_or_default())
        };
        let item_tag_ids: Vec<i64> = item_tags.iter().map(|t| t.id).collect();

        // Sort: tags on the item first, then by name
        tags.sort_by(|a, b| {
            let a_has = item_tag_ids.contains(&a.id);
            let b_has = item_tag_ids.contains(&b.id);
            match (a_has, b_has) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });

        let model: Vec<crate::TagItem> = tags.iter().map(|t| {
            crate::TagItem {
                id: t.id as i32,
                name: slint::SharedString::from(t.name.clone()),
                color: hex_to_slint_color(&t.color),
                checked: item_tag_ids.contains(&t.id),
            }
        }).collect();
        if let Some(app) = self.app.upgrade() {
            app.set_all_tags(slint::ModelRc::from(model.as_slice()));
        }
    }

    /// Load all tags for batch picker (checked if all selected items have the tag)
    pub fn load_all_tags_for_batch_picker(&self) {
        let tags = self.db.lock().expect("db lock poisoned")
            .get_all_tags()
            .unwrap_or_default();

        // Get tags for all selected items
        let selected_i64: Vec<i64> = self.selected_ids.iter().map(|&id| id as i64).collect();
        let tag_map = self.db.lock().expect("db lock poisoned")
            .get_tags_for_items(&selected_i64)
            .unwrap_or_default();

        let total = self.selected_ids.len() as i64;
        let model: Vec<crate::TagItem> = tags.iter().map(|t| {
            let count = tag_map.values()
                .filter(|item_tags| item_tags.iter().any(|it| it.id == t.id))
                .count() as i64;
            crate::TagItem {
                id: t.id as i32,
                name: slint::SharedString::from(t.name.clone()),
                color: hex_to_slint_color(&t.color),
                checked: count == total && total > 0,
            }
        }).collect();
        if let Some(app) = self.app.upgrade() {
            app.set_all_tags(slint::ModelRc::from(model.as_slice()));
        }
    }

    /// Create a new tag
    pub fn create_tag(&self, name: &str) -> Option<crate::core::types::TagInfo> {
        let count = self.db.lock().expect("db lock poisoned")
            .get_all_tags()
            .map(|t| t.len())
            .unwrap_or(0);
        let color = crate::core::types::next_tag_color(count);
        let result = self.db.lock().expect("db lock poisoned")
            .create_tag(name, color)
            .ok()
            .map(|id| crate::core::types::TagInfo {
                id,
                name: name.to_string(),
                color: color.to_string(),
                updated_at: chrono::Utc::now().to_rfc3339(),
            });
        self.mark_dirty();
        result
    }

    /// Update a tag's name and color
    pub fn update_tag(&self, tag_id: i64, name: &str, color: &str) {
        let _ = self.db.lock().expect("db lock poisoned")
            .update_tag(tag_id, name, color);
        self.mark_dirty();
    }

    /// Delete a tag
    pub fn delete_tag(&mut self, tag_id: i64) {
        // Get tag name, delete, and record tombstone in one lock to avoid a
        // race where a sync cycle runs between deletion and tombstone recording.
        {
            if let Ok(mut db) = self.db.lock() {
                let tag_name = db.get_all_tags().ok()
                    .and_then(|tags| tags.iter().find(|t| t.id == tag_id).map(|t| t.name.clone()));
                let _ = db.delete_tag(tag_id);
                if let Some(name) = tag_name {
                    let now = chrono::Utc::now().to_rfc3339();
                    let device = crate::services::backends::local_folder::hostname();
                    let _ = db.record_tag_deletion(&name, &now, &device);
                }
            }
        }
        self.filters.remove_tag(tag_id);
        self.mark_dirty();
    }

    /// Toggle a tag on an item (add/remove)
    pub fn toggle_item_tag(&mut self, item_id: i32, tag_id: i64) {
        let needs_full_refresh = self.filters.has_tag_filters();
        {
            if let Ok(db) = self.db.lock() {
                let item_id_i64 = item_id as i64;
                // Check if already has tag
                let tags = db.get_tags_for_item(item_id_i64).unwrap_or_default();
                if tags.iter().any(|t| t.id == tag_id) {
                    let _ = db.remove_item_tag(item_id_i64, tag_id);
                } else {
                    let _ = db.add_item_tag(item_id_i64, tag_id);
                }
            }
        }
        if needs_full_refresh {
            self.refresh_with_current_filter();
        } else {
            self.refresh_row(item_id);
        }
    }

    /// Create a tag and add it to the current item (from picker)
    pub fn create_and_add_tag(&mut self, item_id: i32, name: &str) -> Option<crate::core::types::TagInfo> {
        let (tag_id, color) = {
            let db = self.db.lock().expect("db lock poisoned");
            let count = db.get_all_tags().map(|t| t.len()).unwrap_or(0);
            let color = crate::core::types::next_tag_color(count);
            let id = db.create_tag(name, color).ok()?;
            (id, color.to_string())
        };
        self.db.lock().expect("db lock poisoned")
            .add_item_tag(item_id as i64, tag_id)
            .ok()?;
        let needs_full = self.filters.has_tag_filters();
        if needs_full {
            self.refresh_with_current_filter();
        } else {
            self.refresh_row(item_id);
        }
        Some(crate::core::types::TagInfo {
            id: tag_id,
            name: name.to_string(),
            color: color.to_string(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
    }

    /// Batch add tag to all selected items
    pub fn batch_add_tag(&mut self, tag_id: i64) {
        if let Ok(db) = self.db.lock() {
            for &id in &self.selected_ids {
                let _ = db.add_item_tag(id as i64, tag_id);
            }
        }
        self.mark_dirty();
        self.refresh_with_current_filter();
        self.selected_ids.clear();
    }

    /// Batch remove tag from all selected items
    pub fn batch_remove_tag(&mut self, tag_id: i64) {
        if let Ok(db) = self.db.lock() {
            for &id in &self.selected_ids {
                let _ = db.remove_item_tag(id as i64, tag_id);
            }
        }
        self.mark_dirty();
        self.refresh_with_current_filter();
        self.selected_ids.clear();
    }

    /// Clear all tags from a single item
    pub fn clear_item_tags(&mut self, item_id: i32) {
        if let Ok(db) = self.db.lock() {
            let _ = db.clear_item_tags(item_id as i64);
        }
        self.mark_dirty();
        self.refresh_row(item_id);
    }

    /// Clear all tags from all selected items (batch)
    pub fn clear_selected_tags(&mut self) {
        if let Ok(db) = self.db.lock() {
            for &id in &self.selected_ids {
                let _ = db.clear_item_tags(id as i64);
            }
        }
        self.mark_dirty();
        self.refresh_with_current_filter();
        self.selected_ids.clear();
    }

    /// Toggle favorite status for an item and refresh that row
    pub fn toggle_favorite(&mut self, id: i32) {
        let needs_full_refresh = self.filters.is_favorites_active();
        {
            if let Ok(db) = self.db.lock() {
                let _ = db.toggle_favorite(id as i64);
            }
        }
        self.mark_dirty();
        if needs_full_refresh {
            self.refresh_with_current_filter();
        } else {
            self.refresh_row(id);
        }
    }

    /// Delete an item from database and remove from model
    pub fn delete_item(&mut self, id: i32) {
        // Delete and record tombstone in the same lock to avoid a race where
        // a sync cycle runs between deletion and tombstone recording.
        {
            if let Ok(db) = self.db.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    let _ = db.delete_item(id as i64);
                    let now = chrono::Utc::now().to_rfc3339();
                    let device = crate::services::backends::local_folder::hostname();
                    let _ = db.record_item_deletion(item.content_hash, &now, &device);
                }
            }
        }
        self.mark_dirty();
        self.entry_cache.remove(&(id as i64));
        for i in 0..self.model.row_count() {
            if let Some(entry) = self.model.row_data(i) {
                if entry.id == id {
                    self.model.remove(i);
                    break;
                }
            }
        }
        if let Some(app) = self.app.upgrade() {
            app.set_item_count(self.model.row_count() as i32);
        }
    }

    /// Update note for an item and refresh that row
    pub fn update_note(&mut self, id: i32, note: &str) {
        if let Ok(db) = self.db.lock() {
            let _ = db.update_note(id as i64, note);
        }
        self.mark_dirty();
        self.refresh_row(id);
    }

    /// Update content for an item, recompute hash, and refresh that row
    pub fn update_content(&mut self, id: i32, text: &str, content_type: &str) {
        if let Ok(db) = self.db.lock() {
            let _ = db.update_content(id as i64, text, content_type);
        }
        self.mark_dirty();
        self.refresh_row(id);
    }

    /// Get reference to shared buffer for platform layer
    pub fn shared(&self) -> &ClipboardShared {
        &self.shared
    }

    /// Mark sync dirty (called after any data mutation).
    fn mark_dirty(&self) {
        self.sync_dirty.store(true, Ordering::SeqCst);
    }


    // ── Selection management ──

    /// Select a single item; deselect all others
    pub fn select_single(&mut self, id: i32) {
        self.selected_ids.clear();
        self.selected_ids.push(id);
        self.anchor_id = id;
        self.sync_selection_to_model();
    }

    /// Toggle selection of an item (Ctrl+click)
    pub fn toggle_selection(&mut self, id: i32) {
        if let Some(pos) = self.selected_ids.iter().position(|&x| x == id) {
            self.selected_ids.remove(pos);
        } else {
            self.selected_ids.push(id);
            self.anchor_id = id;
        }
        self.sync_selection_to_model();
    }

    /// Range select from anchor to given id (Shift+click)
    pub fn range_select(&mut self, id: i32) {
        // Fall back to single select if no anchor exists
        if self.anchor_id < 0 || self.selected_ids.is_empty() {
            self.select_single(id);
            return;
        }
        let mut anchor_idx: Option<usize> = None;
        let mut clicked_idx: Option<usize> = None;
        for i in 0..self.model.row_count() {
            if let Some(entry) = self.model.row_data(i) {
                if entry.id == self.anchor_id {
                    anchor_idx = Some(i);
                }
                if entry.id == id {
                    clicked_idx = Some(i);
                }
            }
        }

        if let (Some(a), Some(c)) = (anchor_idx, clicked_idx) {
            self.selected_ids.clear();
            if a <= c {
                for i in a..=c {
                    if let Some(entry) = self.model.row_data(i) {
                        self.selected_ids.push(entry.id);
                    }
                }
            } else {
                for i in (c..=a).rev() {
                    if let Some(entry) = self.model.row_data(i) {
                        self.selected_ids.push(entry.id);
                    }
                }
            }
        }
        self.sync_selection_to_model();
    }

    /// Clear all selection
    pub fn clear_selection(&mut self) {
        self.selected_ids.clear();
        self.anchor_id = -1;
        self.sync_selection_to_model();
    }

    /// Sync in-memory selection state to model `selected` / `selection_order` fields
    fn sync_selection_to_model(&self) {
        // Set count BEFORE model updates so Slint bindings use the correct value
        // when re-evaluating visibility of selection-dependent elements (e.g. badge).
        if let Some(app) = self.app.upgrade() {
            app.set_selected_count(self.selected_ids.len() as i32);
        }
        for i in 0..self.model.row_count() {
            if let Some(mut entry) = self.model.row_data(i) {
                let pos = self.selected_ids.iter().position(|&x| x == entry.id);
                entry.selected = pos.is_some();
                entry.selection_order = pos.map(|p| p as i32).unwrap_or(-1);
                self.model.set_row_data(i, entry);
            }
        }
    }

    /// Get selected items in selection order
    pub fn get_selected_items(&self) -> Vec<crate::core::types::ClipboardItem> {
        let db = self.db.lock().expect("db lock poisoned");
        self.selected_ids
            .iter()
            .filter_map(|&id| db.get_by_id(id as i64).ok().flatten())
            .collect()
    }

    /// Batch toggle favorite on all selected items
    pub fn batch_toggle_favorite(&mut self) {
        let needs_full_refresh = self.filters.is_favorites_active();
        {
            let db = self.db.lock().expect("db lock poisoned");
            for &id in &self.selected_ids {
                let _ = db.toggle_favorite(id as i64);
            }
        }
        self.mark_dirty();
        if needs_full_refresh {
            self.refresh_with_current_filter();
            self.clear_selection();
        } else {
            let ids: Vec<i32> = self.selected_ids.to_vec();
            for id in ids {
                self.refresh_row(id);
            }
            // Re-sync selection after refresh
            self.sync_selection_to_model();
        }
    }

    /// Batch delete all selected items
    pub fn batch_delete(&mut self) {
        // Delete and record tombstones in the same lock to avoid a race where
        // a sync cycle runs between deletion and tombstone recording.
        {
            let db = self.db.lock().expect("db lock poisoned");
            let mut hashes: Vec<u64> = Vec::with_capacity(self.selected_ids.len());
            for &id in &self.selected_ids {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    hashes.push(item.content_hash);
                }
                let _ = db.delete_item(id as i64);
            }
            let now = chrono::Utc::now().to_rfc3339();
            let device = crate::services::backends::local_folder::hostname();
            for h in &hashes {
                let _ = db.record_item_deletion(*h, &now, &device);
            }
        }
        self.mark_dirty();
        // Remove from cache
        for &id in &self.selected_ids {
            self.entry_cache.remove(&(id as i64));
        }
        // Remove from model: collect indices, sort descending, remove
        let ids = self.selected_ids.clone();
        self.selected_ids.clear();
        self.anchor_id = -1;
        let mut indices: Vec<usize> = (0..self.model.row_count())
            .filter(|&i| self.model.row_data(i).is_some_and(|e| ids.contains(&e.id)))
            .collect();
        indices.sort_unstable_by(|a, b| b.cmp(a));
        for i in indices {
            self.model.remove(i);
        }
        if let Some(app) = self.app.upgrade() {
            app.set_item_count(self.model.row_count() as i32);
            app.set_selected_count(0);
        }
    }

    /// Get or create a cached ClipboardEntry for an item.
    /// Uses item.updated_at as cache key — if the timestamp matches, the
    /// cached entry is reused, avoiding disk I/O for images/icons/favicons.
    fn get_or_create_entry(&mut self, item: &crate::core::types::ClipboardItem) -> ClipboardEntry {
        let ts_key = item.updated_at.to_rfc3339();
        if let Some((cached_ts, cached_entry)) = self.entry_cache.get(&item.id) {
            if *cached_ts == ts_key {
                return cached_entry.clone();
            }
        }
        let entry = item_to_entry(item);
        self.entry_cache.insert(item.id, (ts_key, entry.clone()));
        entry
    }
}

impl Pollable for ClipboardService {
    fn poll(&mut self) {
        // Check if batch paste completed and wants selection cleared
        if self.shared.clear_selection_requested.swap(false, Ordering::SeqCst) {
            self.clear_selection();
        }

        // Reload model if sync externally modified the database
        if self.needs_model_refresh.swap(false, Ordering::SeqCst) {
            self.refresh_with_current_filter();
        }

        let pending = {
            let mut p = self.shared.pending.lock().expect("clipboard pending lock poisoned");
            p.drain(..).collect::<Vec<_>>()
        };

        if pending.is_empty() {
            return;
        }

        self.mark_dirty();

        enum PollAction {
            UpdateExisting(i64, crate::core::types::ClipboardItem),
            MoveToTop(i64, crate::core::types::ClipboardItem),
            InsertNew(crate::core::types::ClipboardItem),
        }

        let actions: Vec<PollAction> = {
            if let Ok(db) = self.db.lock() {
                let mut actions = Vec::new();
                for mut item in pending {
                    // Convert RichText → PlainText when copy-as-plain-text is enabled
                    if self.copy_as_plain_text && item.content_type == crate::core::types::ContentType::RichText {
                        item.content_type = crate::core::types::ContentType::PlainText;
                        item.rich_data = String::new();
                    }

                    if let Err(e) = db.upsert(&item) {
                        eprintln!("[ERROR] ClipboardService: upsert error: {:?}", e);
                        continue;
                    }

                    // Check if item matches current filters
                    if !self.filters.matches_item(&item) {
                        continue;
                    }

                    if let Ok(Some(mut existing)) = db.get_by_hash(item.content_hash) {
                        existing.tags = db.get_tags_for_item(existing.id).unwrap_or_default();
                        if self.sort_by_created {
                            actions.push(PollAction::UpdateExisting(existing.id, existing));
                        } else {
                            actions.push(PollAction::MoveToTop(existing.id, existing));
                        }
                    } else {
                        actions.push(PollAction::InsertNew(item));
                    }
                }
                actions
            } else {
                return;
            }
        }; // db lock released here

        for action in actions {
            match action {
                PollAction::UpdateExisting(id, existing) => {
                    let entry = self.get_or_create_entry(&existing);
                    if let Some(idx) = (0..self.model.row_count()).position(|i| {
                        self.model.row_data(i).map(|e| e.id as i64 == id).unwrap_or(false)
                    }) {
                        self.model.set_row_data(idx, entry);
                    }
                }
                PollAction::MoveToTop(id, existing) => {
                    let entry = self.get_or_create_entry(&existing);
                    if let Some(idx) = (0..self.model.row_count()).position(|i| {
                        self.model.row_data(i).map(|e| e.id as i64 == id).unwrap_or(false)
                    }) {
                        self.model.remove(idx);
                    }
                    self.model.insert(0, entry);
                }
                PollAction::InsertNew(item) => {
                    let entry = self.get_or_create_entry(&item);
                    self.model.insert(0, entry);
                }
            }
        }

        if self.max_items > 0 {
            while self.model.row_count() > self.max_items {
                self.model.remove(self.model.row_count() - 1);
            }
        }

        if let Some(app) = self.app.upgrade() {
            app.set_item_count(self.model.row_count() as i32);
        }
    }
}

/// Convert hex color "#EF4444" to slint::Color
fn hex_to_slint_color(hex: &str) -> slint::Color {
    crate::core::types::parse_hex_color(hex)
        .map(|(r, g, b)| slint::Color::from_rgb_u8(r, g, b))
        .unwrap_or_default()
}

/// Decode source app icon from base64 and cache on disk as PNG.
fn build_source_icon(item: &crate::core::types::ClipboardItem) -> (String, Image) {
    if item.source_app_icon.is_empty() {
        return (String::new(), Image::default());
    }
    let icon_dir = images_dir().join("icons");
    let _ = std::fs::create_dir_all(&icon_dir);
    let safe_name: String = item.source_app_name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let icon_path = if safe_name.is_empty() {
        icon_dir.join(format!("{:016x}.png", item.content_hash))
    } else {
        icon_dir.join(format!("{}.png", safe_name))
    };
    let path_str = icon_path.to_string_lossy().to_string();
    if !icon_path.exists() {
        if let Ok(png_bytes) = base64::engine::general_purpose::STANDARD.decode(&item.source_app_icon) {
            let _ = std::fs::write(&icon_path, png_bytes);
        }
    }
    let img = Image::load_from_path(std::path::Path::new(&icon_path)).unwrap_or_default();
    (path_str, img)
}

/// Build file type display fields: count, icon text, up to 3 filenames, overflow.
fn build_file_fields(item: &crate::core::types::ClipboardItem) -> (i32, String, String, String, String, i32) {
    if item.content_type != crate::core::types::ContentType::File || item.file_data.is_empty() {
        return (0, String::new(), String::new(), String::new(), String::new(), 0);
    }
    let fd = crate::core::types::FileData::from_json(&item.file_data);
    let count = fd.files.len() as i32;
    let icon_text = if count == 1 {
        if fd.files[0].is_dir {
            "文件夹".to_string()
        } else {
            crate::core::types::get_extension_label(&fd.files[0].name)
        }
    } else {
        format!("{}个文件", count)
    };
    let n1 = fd.files.first().map(|f| f.name.clone()).unwrap_or_default();
    let n2 = fd.files.get(1).map(|f| f.name.clone()).unwrap_or_default();
    let n3 = fd.files.get(2).map(|f| f.name.clone()).unwrap_or_default();
    let overflow = if count > 3 { count - 3 } else { 0 };
    (count, icon_text, n1, n2, n3, overflow)
}

/// Extract and cache OS file icon for the first file in a file-type item.
fn build_file_icon(item: &crate::core::types::ClipboardItem) -> (String, Image) {
    if item.content_type != crate::core::types::ContentType::File || item.file_data.is_empty() {
        return (String::new(), Image::default());
    }
    let fd = crate::core::types::FileData::from_json(&item.file_data);
    let first_file = match fd.files.first() {
        Some(f) => f,
        None => return (String::new(), Image::default()),
    };
    let cache_name = if first_file.is_dir {
        "ext_folder".to_string()
    } else {
        let ext = std::path::Path::new(&first_file.name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        if matches!(ext.as_str(), "exe" | "dll" | "msi" | "scr" | "cpl") {
            let stem = std::path::Path::new(&first_file.name)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_else(|| "unknown".to_string());
            format!("app_{}", stem)
        } else if ext.is_empty() {
            "ext_file".to_string()
        } else {
            format!("ext_{}", ext)
        }
    };
    let icon_base64 = match file_icon::extract_file_icon_base64(&first_file.path) {
        Some(b64) => b64,
        None => return (String::new(), Image::default()),
    };
    let icon_dir = images_dir().join("icons");
    let _ = std::fs::create_dir_all(&icon_dir);
    let icon_path = icon_dir.join(format!("{}.png", cache_name));
    let path_str = icon_path.to_string_lossy().to_string();
    if !icon_path.exists() {
        if let Ok(png_bytes) = base64::engine::general_purpose::STANDARD.decode(&icon_base64) {
            let _ = std::fs::write(&icon_path, png_bytes);
        }
    }
    let img = Image::load_from_path(std::path::Path::new(&path_str)).unwrap_or_default();
    (path_str, img)
}

/// Build link/path preview fields: domain, path, favicon, folder icon.
fn build_link_preview(item: &crate::core::types::ClipboardItem) -> (SharedString, SharedString, SharedString, Image, SharedString, Image) {
    match item.content_type {
        crate::core::types::ContentType::Link => {
            let domain = crate::core::types::url_domain(&item.full_text);
            let path = crate::core::types::url_path(&item.full_text);
            let cache_path = favicon::favicon_cache_path(&domain);
            let cp = std::path::PathBuf::from(&cache_path);
            let (fav_path_str, fav_img) = if cp.exists() {
                let img = Image::load_from_path(std::path::Path::new(&cache_path)).unwrap_or_default();
                (cache_path, img)
            } else {
                (String::new(), Image::default())
            };
            (SharedString::from(domain), SharedString::from(path),
             SharedString::from(fav_path_str), fav_img,
             SharedString::default(), Image::default())
        }
        crate::core::types::ContentType::Path => {
            let icon_dir = images_dir().join("icons");
            let _ = std::fs::create_dir_all(&icon_dir);
            let icon_path = icon_dir.join("path_folder.png");
            if !icon_path.exists() {
                let sample_dir = std::path::Path::new(&item.full_text)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| item.full_text.clone());
                if let Some(b64) = file_icon::extract_file_icon_base64(&sample_dir) {
                    if let Ok(png) = base64::engine::general_purpose::STANDARD.decode(&b64) {
                        let _ = std::fs::write(&icon_path, png);
                    }
                }
            }
            let (fol_path_str, fol_img) = if icon_path.exists() {
                let img = Image::load_from_path(std::path::Path::new(&icon_path)).unwrap_or_default();
                (icon_path.to_string_lossy().to_string(), img)
            } else {
                (String::new(), Image::default())
            };
            (SharedString::default(), SharedString::default(),
             SharedString::default(), Image::default(),
             SharedString::from(fol_path_str), fol_img)
        }
        _ => (SharedString::default(), SharedString::default(),
              SharedString::default(), Image::default(),
              SharedString::default(), Image::default())
    }
}

/// Build tag dot display data: up to 3 colored dots, names, overflow count.
fn build_tag_dots(tags: &[crate::core::types::TagInfo]) -> (bool, slint::Color, slint::Color, slint::Color, SharedString, SharedString, SharedString, i32) {
    let mut colors: Vec<slint::Color> = Vec::new();
    let mut names: Vec<SharedString> = Vec::new();
    for t in tags.iter().take(3) {
        names.push(SharedString::from(t.name.clone()));
        if let Some((r, g, b)) = crate::core::types::parse_hex_color(&t.color) {
            colors.push(slint::Color::from_rgb_u8(r, g, b));
        } else {
            colors.push(slint::Color::default());
        }
    }
    while colors.len() < 3 { colors.push(slint::Color::default()); }
    while names.len() < 3 { names.push(SharedString::default()); }
    let overflow = if tags.len() > 3 { (tags.len() - 3) as i32 } else { 0 };
    (!tags.is_empty(), colors[0], colors[1], colors[2], names.remove(0), names.remove(0), names.remove(0), overflow)
}

fn item_to_entry(item: &crate::core::types::ClipboardItem) -> ClipboardEntry {
    let (thumbnail, img_w, img_h) = if !item.image_path.is_empty() {
        let img = Image::load_from_path(std::path::Path::new(&item.image_path)).unwrap_or_default();
        let (w, h) = image::image_dimensions(&item.image_path).unwrap_or((0, 0));
        (img, w, h)
    } else {
        (Image::default(), 0, 0)
    };

    let (source_icon_path, source_app_icon_image) = build_source_icon(item);

    let color_swatch = if item.content_type == crate::core::types::ContentType::Color {
        crate::core::color::detect_color(&item.full_text)
            .map(|c| slint::Color::from_rgb_u8(c.r, c.g, c.b))
            .unwrap_or_default()
    } else {
        slint::Color::default()
    };

    let (file_count, file_icon_text, file_name_1, file_name_2, file_name_3, file_overflow) = build_file_fields(item);
    let (file_icon_path, file_icon_image) = build_file_icon(item);
    let (link_domain, link_path, favicon_path, favicon_image, folder_icon_path, folder_icon_image) = build_link_preview(item);
    let (has_tags, tag_dot_0, tag_dot_1, tag_dot_2, tag_name_0, tag_name_1, tag_name_2, tags_overflow) = build_tag_dots(&item.tags);

    ClipboardEntry {
        id: item.id as i32,
        preview: SharedString::from(item.full_text.clone()),
        content_type: SharedString::from(item.content_type.as_str()),
        time_label: SharedString::from(format_relative_time(&item.updated_at)),
        image_path: SharedString::from(item.image_path.clone()),
        thumbnail,
        preview_length: item.full_text.len() as i32,
        image_width: img_w as i32,
        image_height: img_h as i32,
        is_favorite: item.is_favorite,
        note: SharedString::from(item.note.clone()),
        source_app_name: SharedString::from(item.source_app_name.clone()),
        source_app_icon_path: SharedString::from(source_icon_path),
        source_app_icon_image,
        color_swatch,
        link_domain,
        link_path,
        favicon_path,
        favicon_image,
        folder_icon_path,
        folder_icon_image,
        selected: false,
        selection_order: -1,
        file_count,
        file_icon_text: SharedString::from(file_icon_text),
        file_name_1: SharedString::from(file_name_1),
        file_name_2: SharedString::from(file_name_2),
        file_name_3: SharedString::from(file_name_3),
        file_overflow,
        file_icon_path: SharedString::from(file_icon_path),
        file_icon_image,
        has_tags,
        tag_dot_0,
        tag_dot_1,
        tag_dot_2,
        tag_name_0,
        tag_name_1,
        tag_name_2,
        tags_overflow,
    }
}
