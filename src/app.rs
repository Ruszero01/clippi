//! AppController - ties together all components

use crate::core::db::Database;
use crate::core::frontend::Frontend;
use crate::core::types::ClipboardItem;
use crate::platform::clipboard::create_listener;
use crate::{App, ClipboardEntry};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

pub struct AppController {
    db: Arc<Mutex<Database>>,
    frontend: Arc<Mutex<Frontend>>,
    app: slint::Weak<App>,
    listener: Option<Box<dyn crate::platform::clipboard::ClipboardListener>>,
}

impl AppController {
    pub fn new(slint_app: &App) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let model: Rc<VecModel<ClipboardEntry>> = Rc::new(VecModel::default());
        slint_app.set_clipboard_items(ModelRc::from(model.clone()));
        let app = slint_app.as_weak();

        let db = Arc::new(Mutex::new(Database::open("clippi.db")?));
        let frontend = Arc::new(Mutex::new(Frontend::new(slint_app)));

        // Load existing items from DB
        {
            let items = db.lock().unwrap().load_by_updated(100).unwrap_or_default();
            for item in &items {
                let entry = item_to_slint(item);
                model.push(entry);
            }
            slint_app.set_item_count(model.row_count() as i32);
        }

        let mut listener = create_listener();
        let db_clone = db.clone();
        let frontend_clone = frontend.clone();
        let app_clone = app.clone();

        listener.start(Box::new(move |item: ClipboardItem| {
            // Update database
            if let Ok(db) = db_clone.lock() {
                let _ = db.upsert(&item);
            }

            // Refresh UI if visible
            if let Ok(fe) = frontend_clone.lock() {
                if fe.is_visible() {
                    let db_for_ui = db_clone.clone();
                    let app_for_ui = app_clone.clone();
                    slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_for_ui.upgrade() {
                            if let Ok(db) = db_for_ui.lock() {
                                refresh_ui(&app, &db);
                            }
                        }
                    }).ok();
                }
            }
        }))?;

        // Bind Slint callbacks
        let fe_move = frontend.clone();
        slint_app.on_move_window(move |dx, dy| {
            if let Ok(fe) = fe_move.lock() {
                fe.move_window(dx, dy);
            }
        });

        let fe_close = frontend.clone();
        slint_app.on_close_window(move || {
            if let Ok(mut fe) = fe_close.lock() {
                fe.hide();
            }
        });

        Ok(Self {
            db,
            frontend,
            app,
            listener: Some(listener),
        })
    }

    pub fn shutdown(mut self) {
        if let Some(ref mut listener) = self.listener {
            listener.stop();
        }
    }
}

fn refresh_ui(app: &App, db: &Database) {
    let items = db.load_by_updated(100).unwrap_or_default();
    let model: VecModel<ClipboardEntry> = VecModel::from(
        items.iter().map(item_to_slint).collect::<Vec<_>>()
    );
    app.set_clipboard_items(ModelRc::from(Rc::new(model)));
    app.set_item_count(items.len() as i32);
}

fn item_to_slint(item: &ClipboardItem) -> ClipboardEntry {
    ClipboardEntry {
        id: item.id as i32,
        preview: SharedString::from(item.text_preview.clone()),
        content_type: SharedString::from(item.content_type.as_str()),
        time_label: SharedString::from(format_relative_time(&item.updated_at)),
    }
}

fn format_relative_time(captured_at: &chrono::DateTime<chrono::Utc>) -> String {
    let elapsed = chrono::Utc::now().signed_duration_since(*captured_at);
    let secs = elapsed.num_seconds();
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}
