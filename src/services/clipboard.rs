//! Clipboard service - handles clipboard business logic

use crate::core::db::Database;
use crate::core::filters::ClipboardFilters;
use crate::core::paths::images_dir;
use crate::core::types::format_relative_time;
use crate::looper::Pollable;
use crate::platform::clipboard::ClipboardShared;
use crate::App;
use crate::ClipboardEntry;
use base64::Engine;
use slint::{Image, Model, ModelRc, SharedString, VecModel};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// Maximum items to keep in memory
const MAX_ITEMS: usize = 100;

#[derive(Clone)]
pub struct ClipboardService {
    shared: ClipboardShared,
    db: Arc<Mutex<Database>>,
    app: slint::Weak<App>,
    model: Rc<VecModel<ClipboardEntry>>,
    sort_by_created: bool,
    copy_as_plain_text: bool,
    filters: ClipboardFilters,
}

impl ClipboardService {
    pub fn new(
        shared: ClipboardShared,
        db: Arc<Mutex<Database>>,
        app: slint::Weak<App>,
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

    fn refresh_with_current_filter(&mut self) {
        self.model.clear();
        let order_by = if self.sort_by_created { "created_at" } else { "updated_at" };
        let items = self.db.lock().expect("db lock poisoned")
            .load_filtered(&self.filters, MAX_ITEMS, order_by)
            .unwrap_or_default();
        for item in items {
            self.model.push(item_to_entry(&item));
        }
        if let Some(app) = self.app.upgrade() {
            app.set_item_count(self.model.row_count() as i32);
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

    /// Load initial items from database (model starts empty; delegate to unified refresh)
    pub fn load_initial(&mut self) {
        self.refresh_with_current_filter();
    }

    /// Refresh a single row by id (e.g., after copying to update timestamp)
    pub fn refresh_row(&self, id: i32) {
        if let Ok(db) = self.db.lock() {
            if let Ok(Some(item)) = db.get_by_id(id as i64) {
                for i in 0..self.model.row_count() {
                    if let Some(entry) = self.model.row_data(i) {
                        if entry.id == id {
                            self.model.set_row_data(i, item_to_entry(&item));
                            break;
                        }
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

    /// Toggle favorite status for an item and refresh that row
    pub fn toggle_favorite(&mut self, id: i32) {
        let needs_full_refresh = self.filters.is_favorites_active();
        {
            if let Ok(db) = self.db.lock() {
                let _ = db.toggle_favorite(id as i64);
            }
        }
        if needs_full_refresh {
            self.refresh_with_current_filter();
        } else {
            self.refresh_row(id);
        }
    }

    /// Delete an item from database and remove from model
    pub fn delete_item(&mut self, id: i32) {
        if let Ok(db) = self.db.lock() {
            let _ = db.delete_item(id as i64);
        }
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

    /// Get reference to shared buffer for platform layer
    pub fn shared(&self) -> &ClipboardShared {
        &self.shared
    }
}

impl Pollable for ClipboardService {
    fn poll(&mut self) {
        let pending = {
            let mut p = self.shared.pending.lock().expect("clipboard pending lock poisoned");
            p.drain(..).collect::<Vec<_>>()
        };

        if pending.is_empty() {
            return;
        }

        if let Ok(db) = self.db.lock() {
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

                if let Ok(Some(existing)) = db.get_by_hash(item.content_hash) {
                    let existing_idx = (0..self.model.row_count()).position(|i| {
                        self.model.row_data(i).map(|e| e.id as i64 == existing.id).unwrap_or(false)
                    });
                    if let Some(idx) = existing_idx {
                        self.model.remove(idx);
                    }
                    self.model.insert(0, item_to_entry(&existing));
                } else {
                    self.model.insert(0, item_to_entry(&item));
                }
            }
        }

        while self.model.row_count() > MAX_ITEMS {
            self.model.remove(self.model.row_count() - 1);
        }

        if let Some(app) = self.app.upgrade() {
            app.set_item_count(self.model.row_count() as i32);
        }
    }
}

fn item_to_entry(item: &crate::core::types::ClipboardItem) -> ClipboardEntry {
    let thumbnail = if !item.image_path.is_empty() {
        Image::load_from_path(std::path::Path::new(&item.image_path)).unwrap_or_default()
    } else {
        Image::default()
    };
    let (img_w, img_h) = if !item.image_path.is_empty() {
        image::image_dimensions(&item.image_path).unwrap_or((0, 0))
    } else {
        (0, 0)
    };

    // Decode source app icon from base64 and save to disk for Slint Image loading
    let (source_icon_path, source_icon_image) = if !item.source_app_icon.is_empty() {
        let icon_dir = images_dir().join("icons");
        let _ = std::fs::create_dir_all(&icon_dir);
        let icon_path = icon_dir.join(format!("{:016x}.png", item.content_hash));
        let path_str = icon_path.to_string_lossy().to_string();

        // Write decoded PNG only if not cached
        if !icon_path.exists() {
            if let Ok(png_bytes) = base64::engine::general_purpose::STANDARD.decode(&item.source_app_icon) {
                let _ = std::fs::write(&icon_path, png_bytes);
            }
        }

        let img = Image::load_from_path(std::path::Path::new(&icon_path)).unwrap_or_default();
        (path_str, img)
    } else {
        (String::new(), Image::default())
    };

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
        source_app_name: SharedString::from(item.source_app_name.clone()),
        source_app_icon_path: SharedString::from(source_icon_path),
        source_app_icon_image: source_icon_image,
    }
}
