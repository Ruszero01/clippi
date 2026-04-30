//! Clipboard service - handles clipboard business logic

use crate::core::db::Database;
use crate::core::types::format_relative_time;
use crate::looper::Pollable;
use crate::platform::clipboard::ClipboardShared;
use crate::App;
use crate::ClipboardEntry;
use slint::{Model, ModelRc, SharedString, VecModel};
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
        };
        if let Some(app) = service.app.upgrade() {
            app.set_clipboard_items(ModelRc::from(model));
            app.set_item_count(0);
        }
        service
    }

    /// Set sort mode and refresh (full reload)
    pub fn set_sort_and_refresh(&mut self, sort_by_created: bool) {
        self.sort_by_created = sort_by_created;
        self.model.clear();
        let items = if sort_by_created {
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

    /// Get reference to shared buffer for platform layer
    pub fn shared(&self) -> &ClipboardShared {
        &self.shared
    }
}

impl Pollable for ClipboardService {
    fn poll(&mut self) {
        // Take all pending items
        let pending = {
            let mut p = self.shared.pending.lock().unwrap();
            p.drain(..).collect::<Vec<_>>()
        };

        if pending.is_empty() {
            return;
        }

        // Upsert to database and add to model
        if let Ok(db) = self.db.lock() {
            for item in &pending {
                if let Err(e) = db.upsert(item) {
                    eprintln!("[ERROR] ClipboardService: upsert error: {:?}", e);
                } else {
                    // Find existing item by content_hash to get actual DB id
                    if let Ok(Some(existing)) = db.get_by_hash(item.content_hash) {
                        // Remove existing item from model (by id)
                        let existing_idx = (0..self.model.row_count()).position(|i| {
                            self.model.row_data(i).map(|e| e.id as i64 == existing.id).unwrap_or(false)
                        });
                        if let Some(idx) = existing_idx {
                            self.model.remove(idx);
                        }

                        // Insert with correct DB id at beginning
                        self.model.insert(0, item_to_entry(&existing));
                    } else {
                        // New item - insert with id from item (should match DB after upsert)
                        self.model.insert(0, item_to_entry(item));
                    }
                }
            }
        }

        // Trim to MAX_ITEMS items
        while self.model.row_count() > MAX_ITEMS {
            self.model.remove(self.model.row_count() - 1);
        }

        if let Some(app) = self.app.upgrade() {
            app.set_item_count(self.model.row_count() as i32);
        }
    }
}

fn item_to_entry(item: &crate::core::types::ClipboardItem) -> ClipboardEntry {
    ClipboardEntry {
        id: item.id as i32,
        preview: SharedString::from(item.text_preview.clone()),
        content_type: SharedString::from(item.content_type.as_str()),
        time_label: SharedString::from(format_relative_time(&item.updated_at)),
    }
}