//! Clipboard service - handles clipboard business logic

use crate::core::db::Database;
use crate::core::frontend::Frontend;
use crate::core::types::format_relative_time;
use crate::looper::Pollable;
use crate::platform::clipboard::ClipboardShared;
use crate::App;
use crate::ClipboardEntry;
use slint::{ModelRc, SharedString, VecModel};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

pub struct ClipboardService {
    shared: ClipboardShared,
    db: Arc<Mutex<Database>>,
    frontend: Arc<Mutex<Frontend>>,
    app: slint::Weak<App>,
}

impl ClipboardService {
    pub fn new(
        shared: ClipboardShared,
        db: Arc<Mutex<Database>>,
        frontend: Arc<Mutex<Frontend>>,
        app: slint::Weak<App>,
    ) -> Self {
        Self {
            shared,
            db,
            frontend,
            app,
        }
    }

    fn refresh_ui(&self) {
        let Some(app) = self.app.upgrade() else { return };
        if let Ok(db) = self.db.lock() {
            let items = db.load_by_updated(100).unwrap_or_default();
            let model: VecModel<ClipboardEntry> = VecModel::from(
                items.iter()
                    .map(|item| ClipboardEntry {
                        id: item.id as i32,
                        preview: SharedString::from(item.text_preview.clone()),
                        content_type: SharedString::from(item.content_type.as_str()),
                        time_label: SharedString::from(format_relative_time(&item.updated_at)),
                    })
                    .collect::<Vec<_>>(),
            );
            app.set_clipboard_items(ModelRc::from(Rc::new(model)));
            app.set_item_count(items.len() as i32);
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

        // Upsert to database
        if let Ok(db) = self.db.lock() {
            for item in &pending {
                let _ = db.upsert(item);
            }
        }

        // Refresh UI if visible
        if let Ok(fe) = self.frontend.lock() {
            if fe.is_visible() {
                drop(fe);
                self.refresh_ui();
            }
        }
    }
}
