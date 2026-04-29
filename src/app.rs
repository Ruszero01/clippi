//! AppController - ties together all components

use crate::core::db::Database;
use crate::core::frontend::Frontend;
use crate::core::types::ClipboardItem;
use crate::platform::clipboard::create_listener;
use crate::platform::hotkey::create_hotkey_listener;
use crate::{App, ClipboardEntry};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

pub struct AppController {
    db: Arc<Mutex<Database>>,
    frontend: Arc<Mutex<Frontend>>,
    app: slint::Weak<App>,
    listener: Option<Box<dyn crate::platform::clipboard::ClipboardListener>>,
    hotkey: Arc<Mutex<Option<Box<dyn crate::platform::hotkey::HotkeyListener>>>>,
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
            if let Ok(db) = db_clone.lock() {
                let _ = db.upsert(&item);
            }

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

        // Hotkey pressed callback
        let fe_for_hk = frontend.clone();
        let app_for_hk = app.clone();
        let db_for_hk = db.clone();
        let on_pressed = Arc::new(Mutex::new(Box::new(move || {
            let fe = fe_for_hk.clone();
            let app = app_for_hk.clone();
            let db = db_for_hk.clone();
            slint::invoke_from_event_loop(move || {
                if let Ok(mut fe) = fe.lock() {
                    fe.show();
                }
                if let Some(app) = app.upgrade() {
                    if let Ok(db) = db.lock() {
                        refresh_ui(&app, &db);
                    }
                }
            }).ok();
        }) as Box<dyn Fn() + Send>));

        // Create hotkey Arc early so recording callback can capture it
        let hotkey: Arc<Mutex<Option<Box<dyn crate::platform::hotkey::HotkeyListener>>>> =
            Arc::new(Mutex::new(None));

        // Recording callback - invoked from hotkey thread when a key is detected
        let app_for_recording = app.clone();
        let hotkey_for_apply = hotkey.clone();
        let on_recording = Arc::new(Mutex::new(Box::new(move |result: Option<String>| {
            let app = app_for_recording.clone();
            let hotkey = hotkey_for_apply.clone();
            slint::invoke_from_event_loop(move || {
                if let Some(app) = app.upgrade() {
                    app.set_recording_hotkey(false);
                    if let Some(new_hotkey) = result {
                        app.set_hotkey_display(slint::SharedString::from(&new_hotkey));
                        // Apply the new hotkey
                        if let Ok(mut hk) = hotkey.lock() {
                            if let Some(ref mut h) = *hk {
                                h.finish_recording();
                                if let Err(e) = h.update_hotkey(&new_hotkey) {
                                    app.set_settings_error(slint::SharedString::from(e));
                                }
                            }
                        }
                    }
                }
            }).ok();
        }) as Box<dyn Fn(Option<String>) + Send>));

        // Create and store the actual hotkey listener
        match create_hotkey_listener("Alt+V", on_pressed, Some(on_recording)) {
            Ok(h) => *hotkey.lock().unwrap() = Some(h),
            Err(e) => eprintln!("Failed to create hotkey listener: {}", e),
        }

        // Set initial hotkey display
        slint_app.set_hotkey_display(slint::SharedString::from("Alt+V"));

        // on_set_hotkey callback
        let hotkey_for_callback = hotkey.clone();
        let frontend_for_callback = frontend.clone();
        let app_for_error = app.clone();
        slint_app.on_set_hotkey(move |s: slint::SharedString| {
            if let Ok(mut fe) = frontend_for_callback.lock() {
                fe.show();
            }

            let err_msg = if let Ok(mut hk) = hotkey_for_callback.lock() {
                if let Some(ref mut h) = *hk {
                    match h.update_hotkey(&s) {
                        Ok(()) => None,
                        Err(e) => Some(e),
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let app = app_for_error.clone();
            slint::invoke_from_event_loop(move || {
                if let Some(app) = app.upgrade() {
                    app.set_settings_error(slint::SharedString::from(err_msg.unwrap_or_default()));
                }
            }).ok();
        });

        // on_start_recording_hotkey callback
        let hotkey_for_recording = hotkey.clone();
        slint_app.on_start_recording_hotkey(move || {
            if let Ok(mut hk) = hotkey_for_recording.lock() {
                if let Some(ref mut h) = *hk {
                    h.start_recording();
                }
            }
        });

        Ok(Self {
            db,
            frontend,
            app,
            listener: Some(listener),
            hotkey,
        })
    }

    pub fn shutdown(mut self) {
        if let Some(mut listener) = self.listener {
            listener.stop();
        }
        if let Ok(mut hk) = self.hotkey.lock() {
            if let Some(ref mut h) = *hk {
                h.stop();
            }
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
