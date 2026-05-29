//! Clipboard service - handles clipboard business logic

use crate::core::db::Database;
use crate::core::filters::ClipboardFilters;
use crate::core::i18n;
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
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone)]
pub struct ClipboardService {
    shared: ClipboardShared,
    db: Arc<Mutex<Database>>,
    app: slint::Weak<App>,
    model: Rc<VecModel<ClipboardEntry>>,
    sort_by_created: bool,
    copy_as_plain_text: bool,
    ocr_enabled: bool,
    filters: ClipboardFilters,
    pinned_tag_ids: Vec<i64>,
    sidebar_model: Rc<VecModel<crate::TagItem>>,
    unchecked_unpinned_since: HashMap<i64, Instant>,
    pending_animate_tags: HashSet<i64>,
    selected_ids: Vec<i32>,
    anchor_id: i32,
    sync_dirty: Arc<AtomicBool>,
    needs_model_refresh: Arc<AtomicBool>,
    needs_release: Arc<AtomicBool>,
    needs_reload: Arc<AtomicBool>,
    max_items: usize,
    image_cache: HashMap<String, Image>,
    poll_count: u64,
}

impl ClipboardService {
    pub fn new(
        shared: ClipboardShared,
        db: Arc<Mutex<Database>>,
        app: slint::Weak<App>,
        sync_dirty: Arc<AtomicBool>,
        needs_model_refresh: Arc<AtomicBool>,
        needs_release: Arc<AtomicBool>,
        needs_reload: Arc<AtomicBool>,
    ) -> Self {
        let model: Rc<VecModel<ClipboardEntry>> = Rc::new(VecModel::default());
        let sidebar_model: Rc<VecModel<crate::TagItem>> = Rc::new(VecModel::default());
        let service = Self {
            shared,
            db,
            app: app.clone(),
            model: model.clone(),
            sort_by_created: false,
            copy_as_plain_text: false,
            ocr_enabled: true,
            filters: ClipboardFilters::default(),
            pinned_tag_ids: Vec::new(),
            sidebar_model: sidebar_model.clone(),
            unchecked_unpinned_since: HashMap::new(),
            pending_animate_tags: HashSet::new(),
            selected_ids: Vec::new(),
            anchor_id: -1,
            sync_dirty,
            needs_model_refresh,
            needs_release,
            needs_reload,
            max_items: 100,
            image_cache: HashMap::new(),
            poll_count: 0,
        };
        if let Some(app) = service.app.upgrade() {
            app.set_clipboard_items(ModelRc::from(model));
            app.set_sidebar_tags(ModelRc::from(sidebar_model));
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
            if !a_active {
                self.filters.toggle_type(a);
            }
            if !b_active {
                self.filters.toggle_type(b);
            }
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
        self.selected_ids.clear();
        self.anchor_id = -1;
        let order_by = if self.sort_by_created {
            "created_at"
        } else {
            "updated_at"
        };

        // Use a generous limit to ensure enough non-favorites are loaded after
        // favorites are separated out. The DB prune keeps total under control.
        let query_limit = if self.max_items == 0 {
            usize::MAX
        } else {
            self.max_items.saturating_mul(2).max(200)
        };

        let items = self
            .db
            .lock()
            .expect("db lock poisoned")
            .load_filtered_with_tags(&self.filters, query_limit, order_by)
            .unwrap_or_default();

        // Separate favorites and non-favorites, preserving sort order
        let non_fav_allowed = if self.max_items == 0 {
            usize::MAX
        } else {
            self.max_items
        };
        let mut non_fav_seen = 0usize;
        for item in &items {
            if item.is_favorite {
                let entry = self.item_to_entry(item);
                self.model.push(entry);
            } else if non_fav_seen < non_fav_allowed {
                let entry = self.item_to_entry(item);
                self.model.push(entry);
                non_fav_seen += 1;
            }
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
        let old = self.max_items;
        self.max_items = max_items as usize;
        // If value decreased, prune immediately
        if max_items > 0 && (max_items as usize) < old {
            self.prune_and_clean_model();
        }
    }

    pub fn set_pinned_tag_ids(&mut self, ids: Vec<i64>) {
        self.pinned_tag_ids = ids;
    }

    pub fn set_ocr_enabled(&mut self, enabled: bool) {
        self.ocr_enabled = enabled;
    }

    /// Prune excess non-favorite items from DB and sync model.
    fn prune_and_clean_model(&mut self) {
        if self.max_items == 0 {
            return;
        }
        let pruned_ids = match self.db.lock() {
            Ok(db) => db
                .prune_excess_non_favorites(self.max_items as u32)
                .unwrap_or_default(),
            Err(_) => return,
        };
        if pruned_ids.is_empty() {
            return;
        }
        // Remove pruned items from model (scan from end — pruned items are oldest)
        let mut removed = 0;
        let mut i = self.model.row_count();
        while i > 0 && removed < pruned_ids.len() {
            i -= 1;
            if let Some(entry) = self.model.row_data(i) {
                if pruned_ids.contains(&(entry.id as i64)) {
                    self.model.remove(i);
                    removed += 1;
                }
            }
        }
        self.selected_ids
            .retain(|id| !pruned_ids.contains(&(*id as i64)));
        if let Some(app) = self.app.upgrade() {
            app.set_item_count(self.model.row_count() as i32);
        }
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
            let entry = self.item_to_entry(&item);
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

    /// Toggle tag filter mode (AND/OR) and full reload
    pub fn toggle_tag_mode_and_refresh(&mut self) {
        self.filters.toggle_tag_mode();
        self.refresh_with_current_filter();
    }

    /// Clear all tag filters and full reload
    pub fn clear_tag_filters_and_refresh(&mut self) {
        self.filters.clear_tag_filters();
        self.load_all_tags_for_filter();
        self.refresh_with_current_filter();
    }

    /// Current tag match mode (true = AND)
    pub fn tag_match_all(&self) -> bool {
        self.filters.is_tag_match_all()
    }

    /// Build and push the sidebar-tags model.
    /// Pinned tags come first (in pin order), then active-but-unpinned.
    /// Unpinned tags that just became unchecked stay briefly so the slide-out
    /// animation plays before they disappear.
    pub fn refresh_sidebar_tags(&mut self, pinned_tag_ids: &[i64]) {
        self.pinned_tag_ids = pinned_tag_ids.to_vec();
        let tags = self
            .db
            .lock()
            .expect("db lock poisoned")
            .get_all_tags()
            .unwrap_or_default();
        let active_ids = &self.filters.tag_ids;
        let now = Instant::now();

        // Pinned first (in pin order), then active non-pinned
        let mut ordered: Vec<&crate::core::types::TagInfo> = Vec::new();
        for pid in pinned_tag_ids {
            if let Some(t) = tags.iter().find(|t| t.id == *pid) {
                ordered.push(t);
            }
        }
        for t in &tags {
            if active_ids.contains(&t.id) && !pinned_tag_ids.contains(&t.id) {
                ordered.push(t);
            }
        }

        // Track unpinned tags that just became unchecked — they stay in the
        // model briefly so the slide-out animation can play.
        for t in &tags {
            if !active_ids.contains(&t.id) && !pinned_tag_ids.contains(&t.id) {
                if self.unchecked_unpinned_since.contains_key(&t.id) {
                    // Still pending removal — keep it in the model
                    ordered.push(t);
                } else if self.sidebar_model.iter().any(|item| item.id == t.id as i32) {
                    // Just unchecked: start the removal timer
                    self.unchecked_unpinned_since.insert(t.id, now);
                    ordered.push(t);
                }
            }
        }

        // Clean up tracking for tags that are no longer in pending state
        self.unchecked_unpinned_since.retain(|id, _| {
            !active_ids.contains(id) && !pinned_tag_ids.contains(id)
        });

        let desired: Vec<crate::TagItem> = ordered
            .iter()
            .map(|t| crate::TagItem {
                id: t.id as i32,
                name: slint::SharedString::from(t.name.clone()),
                color: hex_to_slint_color(&t.color),
                checked: active_ids.contains(&t.id),
                pinned: pinned_tag_ids.contains(&t.id),
            })
            .collect();

        // Update VecModel in-place so Slint preserves elements and animations fire.
        let current_len = self.sidebar_model.row_count();

        // Clear pending animations for tags no longer in the desired set
        let desired_ids: HashSet<i64> = desired.iter().map(|t| t.id as i64).collect();
        self.pending_animate_tags.retain(|id| desired_ids.contains(id));

        for (i, tag) in desired.iter().enumerate() {
            if i < current_len {
                self.sidebar_model.set_row_data(i, tag.clone());
            } else {
                // New tag: push collapsed so the slide-in animation plays
                let mut item = tag.clone();
                if item.checked {
                    item.checked = false;
                    self.pending_animate_tags.insert(item.id as i64);
                }
                self.sidebar_model.push(item);
            }
        }
        // Remove excess entries
        while self.sidebar_model.row_count() > desired.len() {
            self.sidebar_model.remove(desired.len());
        }
    }

    /// Remove unpinned unchecked tags whose slide-out animation has finished.
    fn cleanup_expired_sidebar_tags(&mut self) {
        let cutoff = Instant::now() - std::time::Duration::from_millis(300);
        let expired: Vec<i64> = self
            .unchecked_unpinned_since
            .iter()
            .filter(|(_, t)| **t < cutoff)
            .map(|(id, _)| *id)
            .collect();
        for id in &expired {
            self.unchecked_unpinned_since.remove(id);
        }
        if expired.is_empty() {
            return;
        }
        // Remove directly from model — don't call refresh_sidebar_tags which
        // would re-insert the tag (it's still in the model but no longer tracked).
        let mut i = 0;
        while i < self.sidebar_model.row_count() {
            if let Some(item) = self.sidebar_model.row_data(i) {
                if expired.contains(&(item.id as i64)) {
                    self.sidebar_model.remove(i);
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
    }

    /// Clear all tag filters and full reload
    /// Load all tags into the shared model with filter-checked state
    pub fn load_all_tags_for_filter(&self) {
        let tags = self
            .db
            .lock()
            .expect("db lock poisoned")
            .get_all_tags()
            .unwrap_or_default();
        let model: Vec<crate::TagItem> = tags
            .iter()
            .map(|t| crate::TagItem {
                id: t.id as i32,
                name: slint::SharedString::from(t.name.clone()),
                color: hex_to_slint_color(&t.color),
                checked: self.filters.is_tag_active(t.id),
                pinned: false,
            })
            .collect();
        if let Some(app) = self.app.upgrade() {
            app.set_all_tags(slint::ModelRc::from(model.as_slice()));
        }
    }

    /// Load all tags for the picker panel with item-checked state (sorted by recency)
    pub fn load_all_tags_for_picker(&self, item_id: i32) {
        let (mut tags, item_tags) = {
            let db = self.db.lock().expect("db lock poisoned");
            (
                db.get_all_tags().unwrap_or_default(),
                db.get_tags_for_item(item_id as i64).unwrap_or_default(),
            )
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

        let model: Vec<crate::TagItem> = tags
            .iter()
            .map(|t| crate::TagItem {
                id: t.id as i32,
                name: slint::SharedString::from(t.name.clone()),
                color: hex_to_slint_color(&t.color),
                checked: item_tag_ids.contains(&t.id),
                pinned: false,
            })
            .collect();
        if let Some(app) = self.app.upgrade() {
            app.set_all_tags(slint::ModelRc::from(model.as_slice()));
        }
    }

    /// Load all tags for batch picker (checked if all selected items have the tag)
    pub fn load_all_tags_for_batch_picker(&self) {
        let tags = self
            .db
            .lock()
            .expect("db lock poisoned")
            .get_all_tags()
            .unwrap_or_default();

        // Get tags for all selected items
        let selected_i64: Vec<i64> = self.selected_ids.iter().map(|&id| id as i64).collect();
        let tag_map = self
            .db
            .lock()
            .expect("db lock poisoned")
            .get_tags_for_items(&selected_i64)
            .unwrap_or_default();

        let total = self.selected_ids.len() as i64;
        let model: Vec<crate::TagItem> = tags
            .iter()
            .map(|t| {
                let count = tag_map
                    .values()
                    .filter(|item_tags| item_tags.iter().any(|it| it.id == t.id))
                    .count() as i64;
                crate::TagItem {
                    id: t.id as i32,
                    name: slint::SharedString::from(t.name.clone()),
                    color: hex_to_slint_color(&t.color),
                    checked: count == total && total > 0,
                    pinned: false,
                }
            })
            .collect();
        if let Some(app) = self.app.upgrade() {
            app.set_all_tags(slint::ModelRc::from(model.as_slice()));
        }
    }

    /// Create a new tag
    pub fn create_tag(&self, name: &str) -> Option<crate::core::types::TagInfo> {
        let count = self
            .db
            .lock()
            .expect("db lock poisoned")
            .get_all_tags()
            .map(|t| t.len())
            .unwrap_or(0);
        let color = crate::core::types::next_tag_color(count);
        let result = self
            .db
            .lock()
            .expect("db lock poisoned")
            .create_tag(name, color)
            .map_err(|e| eprintln!("创建标签失败: {e}"))
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
        let _ = self
            .db
            .lock()
            .expect("db lock poisoned")
            .update_tag(tag_id, name, color);
        self.mark_dirty();
    }

    /// Delete a tag
    pub fn delete_tag(&mut self, tag_id: i64) {
        // Get tag name, delete, and record tombstone in one lock to avoid a
        // race where a sync cycle runs between deletion and tombstone recording.
        {
            if let Ok(mut db) = self.db.lock() {
                let tag_name = db
                    .get_all_tags()
                    .ok()
                    .and_then(|tags| tags.iter().find(|t| t.id == tag_id).map(|t| t.name.clone()));
                if let Err(e) = db.delete_tag(tag_id) {
                    log::error!("delete_tag({tag_id}): {e}");
                }
                if let Some(name) = tag_name {
                    let now = chrono::Utc::now().to_rfc3339();
                    let device = crate::services::backends::local_folder::hostname();
                    if let Err(e) = db.record_tag_deletion(&name, &now, &device) {
                        log::error!("record_tag_deletion({name}): {e}");
                    }
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
                    if let Err(e) = db.remove_item_tag(item_id_i64, tag_id) {
                        log::error!("remove_item_tag({item_id_i64}, {tag_id}): {e}");
                    }
                } else {
                    if let Err(e) = db.add_item_tag(item_id_i64, tag_id) {
                        log::error!("add_item_tag({item_id_i64}, {tag_id}): {e}");
                    }
                }
            }
        }
        self.mark_dirty();
        if needs_full_refresh {
            self.refresh_with_current_filter();
        } else {
            self.refresh_row(item_id);
        }
    }

    /// Create a tag and add it to the current item (from picker)
    pub fn create_and_add_tag(
        &mut self,
        item_id: i32,
        name: &str,
    ) -> Option<crate::core::types::TagInfo> {
        let (tag_id, color) = {
            let db = self.db.lock().expect("db lock poisoned");
            let count = db.get_all_tags().map(|t| t.len()).unwrap_or(0);
            let color = crate::core::types::next_tag_color(count);
            let id = db.create_tag(name, color).ok()?;
            (id, color.to_string())
        };
        self.db
            .lock()
            .expect("db lock poisoned")
            .add_item_tag(item_id as i64, tag_id)
            .ok()?;
        self.mark_dirty();
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
                if let Err(e) = db.add_item_tag(id as i64, tag_id) {
                    log::error!("batch_add_tag({id}, {tag_id}): {e}");
                }
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
                if let Err(e) = db.remove_item_tag(id as i64, tag_id) {
                    log::error!("batch_remove_tag({id}, {tag_id}): {e}");
                }
            }
        }
        self.mark_dirty();
        self.refresh_with_current_filter();
        self.selected_ids.clear();
    }

    /// Clear all tags from a single item
    pub fn clear_item_tags(&mut self, item_id: i32) {
        if let Ok(db) = self.db.lock() {
            if let Err(e) = db.clear_item_tags(item_id as i64) {
                log::error!("clear_item_tags({item_id}): {e}");
            }
        }
        self.mark_dirty();
        self.refresh_row(item_id);
    }

    /// Clear all tags from all selected items (batch)
    pub fn clear_selected_tags(&mut self) {
        if let Ok(db) = self.db.lock() {
            for &id in &self.selected_ids {
                if let Err(e) = db.clear_item_tags(id as i64) {
                    log::error!("clear_selected_tags({id}): {e}");
                }
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
                // Read current state before toggling
                let was_fav = db
                    .get_by_id(id as i64)
                    .ok()
                    .flatten()
                    .is_some_and(|item| item.is_favorite);
                if let Err(e) = db.toggle_favorite(id as i64) {
                    log::error!("toggle_favorite({id}): {e}");
                }
                // Record or remove unfavorite tombstone
                if was_fav {
                    // Was favorited, now unfavorited — record tombstone
                    if let Ok(Some(item)) = db.get_by_id(id as i64) {
                        let now = chrono::Utc::now().to_rfc3339();
                        let device = crate::services::backends::local_folder::hostname();
                        if let Err(e) = db.record_unfavorite(item.content_hash, &now, &device) {
                            log::error!("record_unfavorite({}): {e}", item.content_hash);
                        }
                    }
                } else {
                    // Was unfavorited, now favorited — remove tombstone
                    if let Ok(Some(item)) = db.get_by_id(id as i64) {
                        if let Err(e) = db.remove_unfavorite(item.content_hash) {
                            log::error!("remove_unfavorite({}): {e}", item.content_hash);
                        }
                    }
                }
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
                    if let Err(e) = db.delete_item(id as i64) {
                        log::error!("delete_item({id}): {e}");
                    }
                    let now = chrono::Utc::now().to_rfc3339();
                    let device = crate::services::backends::local_folder::hostname();
                    if let Err(e) = db.record_item_deletion(item.content_hash, &now, &device) {
                        log::error!("record_item_deletion({}): {e}", item.content_hash);
                    }
                }
            }
        }
        self.mark_dirty();
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
            if let Err(e) = db.update_note(id as i64, note) {
                log::error!("update_note({id}): {e}");
            }
        }
        self.mark_dirty();
        self.refresh_row(id);
    }

    /// Update content for an item, recompute hash, and refresh that row
    pub fn update_content(&mut self, id: i32, text: &str, content_type: &str, meta_type: &str) {
        if let Ok(db) = self.db.lock() {
            if let Err(e) = db.update_content(id as i64, text, content_type, meta_type) {
                log::error!("update_content({id}): {e}");
            }
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
            let now = chrono::Utc::now().to_rfc3339();
            let device = crate::services::backends::local_folder::hostname();
            for &id in &self.selected_ids {
                let was_fav = db
                    .get_by_id(id as i64)
                    .ok()
                    .flatten()
                    .is_some_and(|item| item.is_favorite);
                if let Err(e) = db.toggle_favorite(id as i64) {
                    log::error!("toggle_favorite({id}): {e}");
                }
                if was_fav {
                    if let Ok(Some(item)) = db.get_by_id(id as i64) {
                        if let Err(e) = db.record_unfavorite(item.content_hash, &now, &device) {
                            log::error!("batch record_unfavorite({}): {e}", item.content_hash);
                        }
                    }
                } else {
                    if let Ok(Some(item)) = db.get_by_id(id as i64) {
                        if let Err(e) = db.remove_unfavorite(item.content_hash) {
                            log::error!("batch remove_unfavorite({}): {e}", item.content_hash);
                        }
                    }
                }
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
                if let Err(e) = db.delete_item(id as i64) {
                    log::error!("batch_delete_item({id}): {e}");
                }
            }
            let now = chrono::Utc::now().to_rfc3339();
            let device = crate::services::backends::local_folder::hostname();
            for h in &hashes {
                if let Err(e) = db.record_item_deletion(*h, &now, &device) {
                    log::error!("batch record_item_deletion({h}): {e}");
                }
            }
        }
        self.mark_dirty();
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

    /// Release all model resources — replaces the model with a fresh empty one,
    /// allowing the old model and all GPU textures to be dropped.
    pub fn release_model_resources(&mut self) {
        let new_model: Rc<VecModel<ClipboardEntry>> = Rc::new(VecModel::default());
        if let Some(app) = self.app.upgrade() {
            app.set_clipboard_items(ModelRc::from(new_model.clone()));
            app.set_item_count(0);
            app.set_selected_count(0);
        }
        self.model = new_model;
        self.selected_ids.clear();
        self.anchor_id = -1;
        self.image_cache.clear();

        crate::platform::util::trim_process_working_set();
    }
}

impl Pollable for ClipboardService {
    fn poll(&mut self) {
        self.poll_count += 1;

        // Release model resources when window is hidden
        if self.needs_release.swap(false, Ordering::SeqCst) {
            self.release_model_resources();
        }

        // Periodic WAL truncation (every ~60s at 200ms) to prevent WAL
        // file growth from consuming process memory during long background runs.
        if self.poll_count.is_multiple_of(300) {
            if let Ok(db) = self.db.lock() {
                let _ = db.checkpoint();
            }
        }

        // Reload model when window is shown (always reload — pending items
        // may have been processed between release and show, making the model
        // non-empty but incomplete).
        if self.needs_reload.swap(false, Ordering::SeqCst) {
            self.refresh_with_current_filter();
            let pinned = self.pinned_tag_ids.clone();
            self.refresh_sidebar_tags(&pinned);
        }

        // Settle pending tag animations: tags were pushed collapsed so the
        // slide-in animation plays. Now set their real checked state.
        if !self.pending_animate_tags.is_empty() {
            let active_ids = &self.filters.tag_ids;
            for i in 0..self.sidebar_model.row_count() {
                if let Some(mut item) = self.sidebar_model.row_data(i) {
                    let tid = item.id as i64;
                    if self.pending_animate_tags.remove(&tid) {
                        item.checked = active_ids.contains(&tid);
                        self.sidebar_model.set_row_data(i, item);
                    }
                }
            }
            self.pending_animate_tags.clear();
        }

        // Clean up unpinned tags whose slide-out animation has finished
        self.cleanup_expired_sidebar_tags();

        // Check if batch paste completed and wants selection cleared
        if self
            .shared
            .clear_selection_requested
            .swap(false, Ordering::SeqCst)
        {
            self.clear_selection();
        }

        // Reload model if sync externally modified the database
        if self.needs_model_refresh.swap(false, Ordering::SeqCst) {
            self.refresh_with_current_filter();
        }

        let pending = {
            let mut p = self
                .shared
                .pending
                .lock()
                .expect("clipboard pending lock poisoned");
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
                    if self.copy_as_plain_text
                        && item.content_type == crate::core::types::ContentType::RichText
                    {
                        item.content_type = crate::core::types::ContentType::PlainText;
                        item.rich_data = String::new();
                    }

                    // Compute size once before upsert (file: byte sum, text: char count)
                    item.size = compute_size(&item);

                    // Preserve existing OCR text in rich_data before upsert overwrites it.
                    // Also check whether auto-OCR is needed for this image item.
                    let mut need_ocr = false;
                    let mut ocr_img_path = String::new();
                    let mut ocr_item_id: i64 = 0;
                    let mut ocr_existing_rich = String::new();

                    if self.ocr_enabled
                        && item.content_type == crate::core::types::ContentType::Image
                        && !item.image_path.is_empty()
                    {
                        let rd = crate::core::types::RichData::from_json(&item.rich_data);
                        if rd.ocr_text.is_none() {
                            // New item may overwrite cached OCR — check DB for existing result
                            if let Ok(Some(ref existing)) = db.get_by_hash(item.content_hash) {
                                let erd = crate::core::types::RichData::from_json(&existing.rich_data);
                                if let Some(ref cached) = erd.ocr_text {
                                    // Carry cached OCR into the incoming item before upsert
                                    let mut rd = rd;
                                    rd.ocr_text = Some(cached.clone());
                                    item.rich_data = rd.to_json();
                                } else {
                                    need_ocr = true;
                                    ocr_img_path = item.image_path.clone();
                                    ocr_item_id = existing.id;
                                    ocr_existing_rich = existing.rich_data.clone();
                                }
                            } else {
                                // Brand-new image — will get id after upsert
                            }
                        }
                    }

                    if let Err(e) = db.upsert(&item) {
                        log::error!("ClipboardService: upsert error: {:?}", e);
                        continue;
                    }

                    // Auto OCR for brand-new image items (no prior DB record)
                    if need_ocr && ocr_item_id == 0 {
                        if let Ok(Some(existing)) = db.get_by_hash(item.content_hash) {
                            ocr_item_id = existing.id;
                        }
                    }

                    if need_ocr && ocr_item_id != 0 {
                        let img_path = ocr_img_path;
                        let db_clone = self.db.clone();
                        let needs_refresh = self.needs_model_refresh.clone();
                        let existing_rich = ocr_existing_rich;
                        std::thread::spawn(move || {
                            let engine = crate::core::ocr::create_ocr_engine();
                            match engine.recognize(std::path::Path::new(&img_path)) {
                                Ok(text) if !text.trim().is_empty() => {
                                    let mut rd = crate::core::types::RichData::from_json(&existing_rich);
                                    rd.ocr_text = Some(text);
                                    let json = rd.to_json();
                                    if let Ok(db) = db_clone.lock() {
                                        let _ = db.update_rich_data(ocr_item_id, &json);
                                        needs_refresh.store(true, std::sync::atomic::Ordering::SeqCst);
                                    }
                                }
                                Ok(_) => { /* empty result, skip */ }
                                Err(e) => log::error!("OCR error for item {}: {}", ocr_item_id, e),
                            }
                        });
                    }

                    if !self.filters.matches_item(&item) {
                        continue;
                    }

                    if let Ok(Some(mut existing)) = db.get_by_hash(item.content_hash) {
                        existing.tags = db.get_tags_for_item(existing.id).unwrap_or_default();
                        // Carry over in-memory image dimensions from fresh detection
                        if item.image_width > 0 {
                            existing.image_width = item.image_width;
                            existing.image_height = item.image_height;
                        }
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
        };

        for action in actions {
            match action {
                PollAction::UpdateExisting(id, existing) => {
                    let entry = self.item_to_entry(&existing);
                    if let Some(idx) = (0..self.model.row_count()).position(|i| {
                        self.model
                            .row_data(i)
                            .map(|e| e.id as i64 == id)
                            .unwrap_or(false)
                    }) {
                        self.model.set_row_data(idx, entry);
                    }
                }
                PollAction::MoveToTop(id, existing) => {
                    let entry = self.item_to_entry(&existing);
                    if let Some(idx) = (0..self.model.row_count()).position(|i| {
                        self.model
                            .row_data(i)
                            .map(|e| e.id as i64 == id)
                            .unwrap_or(false)
                    }) {
                        self.model.remove(idx);
                    }
                    self.model.insert(0, entry);
                }
                PollAction::InsertNew(item) => {
                    let entry = self.item_to_entry(&item);
                    self.model.insert(0, entry);
                }
            }
        }

        // Prune excess non-favorite items from DB and sync model
        self.prune_and_clean_model();

        // Safety net: trim model from end, skipping favorites
        if self.max_items > 0 {
            let mut non_fav_count: usize = (0..self.model.row_count())
                .filter(|i| self.model.row_data(*i).is_some_and(|e| !e.is_favorite))
                .count();
            let mut i = self.model.row_count();
            while i > 0 && non_fav_count > self.max_items {
                i -= 1;
                if let Some(entry) = self.model.row_data(i) {
                    if !entry.is_favorite {
                        self.model.remove(i);
                        non_fav_count -= 1;
                    }
                }
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
fn build_source_icon(
    item: &crate::core::types::ClipboardItem,
    cache: &mut HashMap<String, Image>,
) -> (String, Image) {
    if item.source_app_icon.is_empty() {
        return (String::new(), Image::default());
    }
    let icon_dir = images_dir().join("icons");
    let _ = std::fs::create_dir_all(&icon_dir);
    let safe_name: String = item
        .source_app_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let icon_path = if safe_name.is_empty() {
        icon_dir.join(format!("{:016x}.png", item.content_hash))
    } else {
        icon_dir.join(format!("{}.png", safe_name))
    };
    let path_str = icon_path.to_string_lossy().to_string();
    if !icon_path.exists() {
        if let Ok(png_bytes) =
            base64::engine::general_purpose::STANDARD.decode(&item.source_app_icon)
        {
            let _ = std::fs::write(&icon_path, png_bytes);
        }
    }
    let img = if let Some(cached) = cache.get(&path_str) {
        cached.clone()
    } else {
        let img = Image::load_from_path(std::path::Path::new(&path_str)).unwrap_or_default();
        cache.insert(path_str.clone(), img.clone());
        img
    };
    (path_str, img)
}

/// Truncate a filename: keep head + "..." + extension so the result fits
/// within `max_len` characters. The stem middle is elided.
/// Build size label for the bottom-right corner tag from the stored `size` field.
/// File types: total file size in bytes → human-readable (e.g. "2.5 MB").
/// Text types: character count → human-readable (e.g. "1,234字").
fn build_size_label(item: &crate::core::types::ClipboardItem) -> String {
    if item.size <= 0 {
        return String::new();
    }
    match item.content_type {
        crate::core::types::ContentType::File => format_file_size(item.size as u64),
        crate::core::types::ContentType::PlainText | crate::core::types::ContentType::RichText => {
            format_char_count(item.size as usize)
        }
        _ => String::new(),
    }
}

/// Compute the `size` value for a clipboard item to store in DB.
/// File types: sum of all file sizes. Text types: char count. Other types: 0.
fn compute_size(item: &crate::core::types::ClipboardItem) -> i64 {
    match item.content_type {
        // File size is now computed during detection (in the background listener thread).
        // No fs::metadata() here to avoid blocking the main thread on network/cloud volumes.
        crate::core::types::ContentType::File => item.size,
        crate::core::types::ContentType::PlainText | crate::core::types::ContentType::RichText => {
            item.full_text.chars().count() as i64
        }
        _ => 0,
    }
}

fn format_file_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    if unit_idx == 0 {
        format!("{bytes} B")
    } else {
        format!("{:.1} {}", size, UNITS[unit_idx])
    }
}

fn format_char_count(count: usize) -> String {
    if count < 1000 {
        if i18n::is_en() {
            format!("{count} chars")
        } else {
            format!("{count}字")
        }
    } else if i18n::is_en() {
        let k = count as f64 / 1000.0;
        if k < 100.0 {
            format!("{:.1}K chars", k)
        } else {
            format!("{:.0}K chars", k)
        }
    } else if count < 10000 {
        format!("{:.1}千字", count as f64 / 1000.0)
    } else {
        let wan = count as f64 / 10000.0;
        if wan < 10.0 {
            format!("{:.1}万字", wan)
        } else {
            format!("{:.0}万字", wan)
        }
    }
}

/// Build file type display fields: count, icon text, up to 3 truncated filenames,
/// raw names, overflow.
fn split_name_for_display(name: &str, is_dir: bool) -> (String, String) {
    if is_dir {
        return (name.to_string(), String::new());
    }
    let path = std::path::Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let ext_with_dot = if ext.is_empty() {
        String::new()
    } else {
        format!(".{}", ext)
    };
    (stem.to_string(), ext_with_dot)
}

fn build_file_fields(
    item: &crate::core::types::ClipboardItem,
) -> (
    i32,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i32,
) {
    if item.content_type != crate::core::types::ContentType::File || item.file_data.is_empty() {
        return (
            0,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            0,
        );
    }
    let fd = crate::core::types::FileData::from_json(&item.file_data);
    let count = fd.files.len() as i32;
    let icon_text = if count == 1 {
        if fd.files[0].is_dir {
            i18n::tr("文件夹", "Folder").to_string()
        } else {
            crate::core::types::get_extension_label(&fd.files[0].name)
        }
    } else {
        count.to_string()
    };

    let (n1, e1) = fd
        .files
        .first()
        .map(|f| split_name_for_display(&f.name, f.is_dir))
        .unwrap_or_default();
    let (n2, e2) = fd
        .files
        .get(1)
        .map(|f| split_name_for_display(&f.name, f.is_dir))
        .unwrap_or_default();
    let (n3, e3) = fd
        .files
        .get(2)
        .map(|f| split_name_for_display(&f.name, f.is_dir))
        .unwrap_or_default();

    let n1_raw = fd.files.first().map(|f| f.name.clone()).unwrap_or_default();
    let n2_raw = fd.files.get(1).map(|f| f.name.clone()).unwrap_or_default();
    let n3_raw = fd.files.get(2).map(|f| f.name.clone()).unwrap_or_default();
    let overflow = if count > 3 { count - 3 } else { 0 };
    (
        count, icon_text, n1, n2, n3, n1_raw, n2_raw, n3_raw, e1, e2, e3, overflow,
    )
}

/// OS icon for a single file. Cached in memory (HashMap) and on disk (PNG).
/// Disk cache is checked first — avoids the expensive Shell API call when
/// the icon was already extracted in a previous session.
fn icon_for_file(
    fi: &crate::core::types::FileInfo,
    cache: &mut HashMap<String, Image>,
) -> (String, Image) {
    let cache_name = if fi.is_dir {
        "ext_folder".to_string()
    } else {
        let ext = std::path::Path::new(&fi.name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        if matches!(ext.as_str(), "exe" | "dll" | "msi" | "scr" | "cpl") {
            let stem = std::path::Path::new(&fi.name)
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
    let icon_path = images_dir()
        .join("icons")
        .join(format!("{}.png", cache_name));
    let path_str = icon_path.to_string_lossy().to_string();

    // Memory cache hit — instant
    if let Some(cached) = cache.get(&path_str) {
        return (path_str, cached.clone());
    }

    // Disk cache hit — load without calling Shell API
    if icon_path.exists() {
        let img = Image::load_from_path(std::path::Path::new(&path_str)).unwrap_or_default();
        cache.insert(path_str.clone(), img.clone());
        return (path_str, img);
    }

    // Cold path: extract from OS, write to disk cache
    let icon_base64 = match file_icon::extract_file_icon_base64(&fi.path) {
        Some(b64) => b64,
        None => return (String::new(), Image::default()),
    };
    let _ = std::fs::create_dir_all(icon_path.parent().unwrap());
    if let Ok(png_bytes) = base64::engine::general_purpose::STANDARD.decode(&icon_base64) {
        let _ = std::fs::write(&icon_path, png_bytes);
    }
    let img = Image::load_from_path(std::path::Path::new(&path_str)).unwrap_or_default();
    cache.insert(path_str.clone(), img.clone());
    (path_str, img)
}

/// Extract OS file icons for up to 3 previewed files (cached by extension/app).
fn build_file_icons(
    item: &crate::core::types::ClipboardItem,
    cache: &mut HashMap<String, Image>,
) -> (String, Image, String, Image, String, Image) {
    if item.content_type != crate::core::types::ContentType::File || item.file_data.is_empty() {
        return (
            String::new(),
            Image::default(),
            String::new(),
            Image::default(),
            String::new(),
            Image::default(),
        );
    }
    let fd = crate::core::types::FileData::from_json(&item.file_data);
    let (p1, img1) = fd
        .files
        .first()
        .map(|f| icon_for_file(f, cache))
        .unwrap_or_default();
    let (p2, img2) = fd
        .files
        .get(1)
        .map(|f| icon_for_file(f, cache))
        .unwrap_or_default();
    let (p3, img3) = fd
        .files
        .get(2)
        .map(|f| icon_for_file(f, cache))
        .unwrap_or_default();
    (p1, img1, p2, img2, p3, img3)
}

/// Build link/path preview fields: domain, path, favicon, folder icon.
fn build_link_preview(
    item: &crate::core::types::ClipboardItem,
    cache: &mut HashMap<String, Image>,
) -> (
    SharedString,
    SharedString,
    SharedString,
    Image,
    SharedString,
    Image,
) {
    match item.content_type {
        crate::core::types::ContentType::Link => {
            let domain = crate::core::types::url_domain(&item.full_text);
            let path = crate::core::types::url_path(&item.full_text);
            let cache_path = favicon::favicon_cache_path(&domain);
            let cp = std::path::PathBuf::from(&cache_path);
            let (fav_path_str, fav_img) = if cp.exists() {
                let img = if let Some(cached) = cache.get(&cache_path) {
                    cached.clone()
                } else {
                    let img = Image::load_from_path(std::path::Path::new(&cache_path))
                        .unwrap_or_default();
                    cache.insert(cache_path.clone(), img.clone());
                    img
                };
                (cache_path, img)
            } else {
                (String::new(), Image::default())
            };
            (
                SharedString::from(domain),
                SharedString::from(path),
                SharedString::from(fav_path_str),
                fav_img,
                SharedString::default(),
                Image::default(),
            )
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
            let path_str = icon_path.to_string_lossy().to_string();
            let (fol_path_str, fol_img) = if icon_path.exists() {
                let img = if let Some(cached) = cache.get(&path_str) {
                    cached.clone()
                } else {
                    let img =
                        Image::load_from_path(std::path::Path::new(&icon_path)).unwrap_or_default();
                    cache.insert(path_str.clone(), img.clone());
                    img
                };
                (path_str, img)
            } else {
                (String::new(), Image::default())
            };
            (
                SharedString::default(),
                SharedString::default(),
                SharedString::default(),
                Image::default(),
                SharedString::from(fol_path_str),
                fol_img,
            )
        }
        _ => (
            SharedString::default(),
            SharedString::default(),
            SharedString::default(),
            Image::default(),
            SharedString::default(),
            Image::default(),
        ),
    }
}

/// Build tag dot display data: up to 3 colored dots, names, overflow count.
fn build_tag_dots(
    tags: &[crate::core::types::TagInfo],
) -> (
    bool,
    slint::Color,
    slint::Color,
    slint::Color,
    SharedString,
    SharedString,
    SharedString,
    i32,
) {
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
    while colors.len() < 3 {
        colors.push(slint::Color::default());
    }
    while names.len() < 3 {
        names.push(SharedString::default());
    }
    let overflow = if tags.len() > 3 {
        (tags.len() - 3) as i32
    } else {
        0
    };
    (
        !tags.is_empty(),
        colors[0],
        colors[1],
        colors[2],
        names.remove(0),
        names.remove(0),
        names.remove(0),
        overflow,
    )
}

/// Derive thumbnail path from full image path.
/// "…/images/0123456789abcdef.png" → "…/images/thumb_0123456789abcdef.png"
fn thumb_path(image_path: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(image_path);
    if let Some(stem) = p.file_stem() {
        p.with_file_name(format!("thumb_{}.png", stem.to_string_lossy()))
    } else {
        p.to_path_buf()
    }
}

impl ClipboardService {
    fn item_to_entry(&mut self, item: &crate::core::types::ClipboardItem) -> ClipboardEntry {
        // Thumbnails are unique per image — load directly without caching so the
        // decoded RGBA buffer is freed when the model entry is dropped.
        let (thumbnail, img_w, img_h) = if !item.image_path.is_empty() {
            let tp = thumb_path(&item.image_path);
            let load_path = if tp.exists() {
                &tp
            } else {
                std::path::Path::new(&item.image_path)
            };
            let img = Image::load_from_path(load_path).unwrap_or_default();
            // Prefer in-memory dimensions (fresh capture); fall back to file read (DB reload)
            let (w, h) = if item.image_width > 0 && item.image_height > 0 {
                (item.image_width, item.image_height)
            } else {
                image::image_dimensions(load_path).unwrap_or((0, 0))
            };
            (img, w, h)
        } else {
            (Image::default(), 0, 0)
        };

        let (source_icon_path, source_app_icon_image) =
            build_source_icon(item, &mut self.image_cache);

        let color_swatch = if item.content_type == crate::core::types::ContentType::Color {
            crate::core::color::detect_color(&item.full_text)
                .map(|c| slint::Color::from_rgb_u8(c.r, c.g, c.b))
                .unwrap_or_default()
        } else {
            slint::Color::default()
        };

        let (
            file_count,
            file_icon_text,
            file_name_1,
            file_name_2,
            file_name_3,
            file_name_1_raw,
            file_name_2_raw,
            file_name_3_raw,
            file_ext_1,
            file_ext_2,
            file_ext_3,
            file_overflow,
        ) = build_file_fields(item);
        let (
            file_icon_path,
            file_icon_image,
            file_icon_path_2,
            file_icon_image_2,
            file_icon_path_3,
            file_icon_image_3,
        ) = build_file_icons(item, &mut self.image_cache);
        let (
            link_domain,
            link_path,
            favicon_path,
            favicon_image,
            folder_icon_path,
            folder_icon_image,
        ) = build_link_preview(item, &mut self.image_cache);
        let (
            has_tags,
            tag_dot_0,
            tag_dot_1,
            tag_dot_2,
            tag_name_0,
            tag_name_1,
            tag_name_2,
            tags_overflow,
        ) = build_tag_dots(&item.tags);
        let size_label = build_size_label(item);

        ClipboardEntry {
            id: item.id as i32,
            preview: SharedString::from(item.full_text.clone()),
            content_type: SharedString::from(item.content_type.as_str()),
            meta_type: SharedString::from(item.meta_type.clone()),
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
            file_name_1_raw: SharedString::from(file_name_1_raw),
            file_name_2_raw: SharedString::from(file_name_2_raw),
            file_name_3_raw: SharedString::from(file_name_3_raw),
            file_ext_1: SharedString::from(file_ext_1),
            file_ext_2: SharedString::from(file_ext_2),
            file_ext_3: SharedString::from(file_ext_3),
            file_overflow,
            file_icon_path: SharedString::from(file_icon_path),
            file_icon_image,
            file_icon_path_2: SharedString::from(file_icon_path_2),
            file_icon_image_2,
            file_icon_path_3: SharedString::from(file_icon_path_3),
            file_icon_image_3,
            has_tags,
            tag_dot_0,
            tag_dot_1,
            tag_dot_2,
            tag_name_0,
            tag_name_1,
            tag_name_2,
            tags_overflow,
            size_label: SharedString::from(size_label),
        }
    }
}
