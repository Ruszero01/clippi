//! Clipboard service - handles clipboard business logic

use crate::core::db::Database;
use crate::core::filters::ClipboardFilters;
use crate::core::types::format_relative_time;
use crate::looper::Pollable;
use crate::platform::clipboard::ClipboardShared;
use crate::App;
use crate::ClipboardEntry;
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

    /// Check if keyword search is active
    pub fn has_keyword(&self) -> bool {
        self.filters.has_keyword()
    }

    fn refresh_with_current_filter(&mut self) {
        self.model.clear();
        let order_by = if self.sort_by_created { "created_at" } else { "updated_at" };
        let items = self.db.lock().unwrap()
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

    /// Load initial items from database
    pub fn load_initial(&self) {
        let items = self.db.lock().unwrap()
            .load_filtered(&self.filters, MAX_ITEMS,
                if self.sort_by_created { "created_at" } else { "updated_at" })
            .unwrap_or_default();
        for item in items {
            self.model.push(item_to_entry(&item));
        }
        if let Some(app) = self.app.upgrade() {
            app.set_item_count(self.model.row_count() as i32);
        }
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

    /// Check if any filters are active
    pub fn has_active_filters(&self) -> bool {
        !self.filters.is_empty()
    }

    /// Get reference to shared buffer for platform layer
    pub fn shared(&self) -> &ClipboardShared {
        &self.shared
    }
}

impl Pollable for ClipboardService {
    fn poll(&mut self) {
        let pending = {
            let mut p = self.shared.pending.lock().unwrap();
            p.drain(..).collect::<Vec<_>>()
        };

        if pending.is_empty() {
            return;
        }

        if let Ok(db) = self.db.lock() {
            for item in &pending {
                if let Err(e) = db.upsert(item) {
                    eprintln!("[ERROR] ClipboardService: upsert error: {:?}", e);
                    continue;
                }

                // Check if item matches current filters
                if !self.filters.matches_item(item) {
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
                    self.model.insert(0, item_to_entry(item));
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
    ClipboardEntry {
        id: item.id as i32,
        preview: SharedString::from(item.full_text.clone()),
        content_type: SharedString::from(item.content_type.as_str()),
        time_label: SharedString::from(format_relative_time(&item.updated_at)),
        image_path: SharedString::from(item.image_path.clone()),
        thumbnail,
    }
}
