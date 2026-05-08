//! Clipboard service - handles clipboard business logic

use crate::core::db::Database;
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
    active_filters: Vec<String>,
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
            active_filters: Vec::new(),
        };
        if let Some(app) = service.app.upgrade() {
            app.set_clipboard_items(ModelRc::from(model));
            app.set_item_count(0);
        }
        service
    }

    /// Toggle a content type filter and reload from database
    pub fn toggle_filter_and_refresh(&mut self, filter_type: &str) {
        if let Some(pos) = self.active_filters.iter().position(|f| f == filter_type) {
            self.active_filters.remove(pos);
        } else {
            self.active_filters.push(filter_type.to_string());
        }
        self.refresh_with_current_filter();
    }

    fn refresh_with_current_filter(&mut self) {
        self.model.clear();
        let order_by = if self.sort_by_created { "created_at" } else { "updated_at" };
        let items = if self.active_filters.is_empty() {
            if self.sort_by_created {
                self.db.lock().unwrap().load_by_created(MAX_ITEMS).unwrap_or_default()
            } else {
                self.db.lock().unwrap().load_by_updated(MAX_ITEMS).unwrap_or_default()
            }
        } else {
            let types: Vec<&str> = self.active_filters.iter().map(|s| s.as_str()).collect();
            self.db.lock().unwrap()
                .load_by_types(&types, MAX_ITEMS, order_by)
                .unwrap_or_default()
        };
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
        let items = if self.sort_by_created {
            self.db.lock().unwrap().load_by_created(MAX_ITEMS).unwrap_or_default()
        } else {
            self.db.lock().unwrap().load_by_updated(MAX_ITEMS).unwrap_or_default()
        };
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
        self.active_filters.iter().any(|f| f == filter_type)
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
                let matches_filter = if self.active_filters.is_empty() {
                    true
                } else {
                    self.active_filters.iter().any(|f| item.content_type.as_str() == f.as_str())
                };
                if !matches_filter {
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
        preview: SharedString::from(item.text_preview.clone()),
        content_type: SharedString::from(item.content_type.as_str()),
        time_label: SharedString::from(format_relative_time(&item.updated_at)),
        image_path: SharedString::from(item.image_path.clone()),
        thumbnail,
    }
}