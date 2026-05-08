//! AppController - assembles all services and sets up the application

use crate::core::db::Database;
use crate::core::frontend::{Frontend, PositionMode};
use crate::core::settings::{
    is_system_dark_mode, migrate_database, set_auto_start, spawn_new_process,
    AppSettings,
};
use crate::looper::Looper;
use crate::platform::clipboard::{create_listener, ClipboardShared};
use crate::platform::paste::{paste_after_delay, restore_paste_target};
use crate::platform::hotkey::create_hotkey_listener;
use crate::services::clipboard::ClipboardService;
use crate::services::focus::FocusService;
use crate::services::hotkey::HotkeyService;
use crate::services::tray::TrayService;
use crate::App;
use clipboard_rs::{Clipboard, ClipboardContext};
use slint::{ComponentHandle, SharedString};
use std::sync::{Arc, Mutex};

pub struct AppController {
    looper: Arc<Looper>,
    listener: Option<Box<dyn crate::platform::clipboard::ClipboardListener>>,
    frontend: Arc<Mutex<Frontend>>,
    _settings: AppSettings,
}

impl AppController {
    pub fn new(slint_app: &App) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Migrate legacy files from exe dir to platform data dir
        crate::core::paths::migrate_legacy_files();

        // Load settings
        let settings = AppSettings::load();

        // Initialize shared state
        let db_path = settings.resolve_db_path();
        let db = Arc::new(Mutex::new(Database::open(db_path.to_str().unwrap())?));
        let mut frontend = Frontend::new(slint_app);
        frontend.set_position_mode(PositionMode::from_str(&settings.window_position_mode));
        frontend.set_saved_position(settings.saved_window_x, settings.saved_window_y);
        let frontend = Arc::new(Mutex::new(frontend));
        let app = slint_app.as_weak();

        // Initialize UI with settings
        init_ui_from_settings(slint_app, &settings);

        // Create clipboard service (sets up model in new())
        let clipboard_shared = ClipboardShared::new();
        let mut clipboard_service = ClipboardService::new(
            clipboard_shared,
            db.clone(),
            app.clone(),
        );
        let clipboard_service_for_callbacks = clipboard_service.clone();
        // Load initial data and apply sort setting
        clipboard_service.load_initial();
        clipboard_service.set_sort_and_refresh(settings.sort_by_created);

        let mut listener = create_listener();
        listener.start(clipboard_service.shared())?;

        // Create hotkey service
        let mut hotkey_service = HotkeyService::new(frontend.clone(), app.clone());
        if let Ok(h) = create_hotkey_listener(&settings.hotkey) {
            hotkey_service.set_hotkey(h);
        }

        // Create tray service (use shared frontend)
        let tray_service = TrayService::new(frontend.clone());

        // Create focus service
        let mut focus_service = match FocusService::new(frontend.clone(), app.clone()) {
            Ok(fs) => fs,
            Err(e) => {
                return Err(e.into());
            }
        };
        // Apply initial focus settings
        focus_service.set_auto_hide(settings.auto_hide);

        // Create and start looper
        let mut looper = Looper::new();
        looper.add_service(Box::new(tray_service));
        looper.set_clipboard_service(clipboard_service);
        looper.set_hotkey_service(hotkey_service);
        looper.set_focus_service(focus_service);
        looper.start();

        // ========== Bind Slint callbacks ==========

        let frontend_clone = frontend.clone();
        slint_app.on_move_window(move |dx, dy| {
            if let Ok(fe) = frontend_clone.lock() {
                fe.move_window(dx, dy);
            }
        });

        // Copy item to clipboard
        let db_for_copy = db.clone();
        let cs_for_copy = clipboard_service_for_callbacks.clone();
        slint_app.on_copy_item(move |id| {
            if let Ok(db) = db_for_copy.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    if let Ok(ctx) = ClipboardContext::new() {
                        let _ = Clipboard::set_text(&ctx, item.full_text.clone());
                    }
                }
            }
            cs_for_copy.refresh_row(id);
        });

        // Paste item - copy to clipboard, restore focus, then paste
        // FocusService will auto-hide if not pinned
        let db_for_paste = db.clone();
        slint_app.on_paste_item(move |id| {
            // Copy to clipboard
            if let Ok(db) = db_for_paste.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    if let Ok(ctx) = ClipboardContext::new() {
                        let _ = Clipboard::set_text(&ctx, item.full_text.clone());
                    }
                }
            }
            // Restore focus to previous window (FocusService handles auto-hide)
            restore_paste_target();
            // Simulate Ctrl+V after delay (to paste to previously focused app)
            paste_after_delay();
        });

        let app_for_close = app.clone();
        let frontend_close = frontend.clone();
        let settings_for_close = settings.clone();
        slint_app.on_close_window(move || {
            if let Some(app) = app_for_close.upgrade() {
                app.set_pinned(false);
            }
            if let Ok(mut fe) = frontend_close.lock() {
                fe.hide();
                let mut s = settings_for_close.clone();
                fe.apply_saved_position_to_settings(&mut s);
                s.save();
            }
        });

        // Hotkey callbacks
        let looper = Arc::new(looper);
        let looper_for_set = Arc::clone(&looper);
        let looper_for_recording = Arc::clone(&looper);
        let app_for_set_hotkey = app.clone();
        let app_for_recording = app.clone();

        slint_app.on_set_hotkey(move |s: SharedString| {
            let looper = Arc::clone(&looper_for_set);
            let _ = looper.try_with_hotkey_service(|hk| {
                if let Err(e) = hk.update_hotkey(&s) {
                    if let Some(app) = app_for_set_hotkey.upgrade() {
                        app.set_settings_error(SharedString::from(e));
                    }
                }
            });
        });

        slint_app.on_start_recording_hotkey(move || {
            let looper = Arc::clone(&looper_for_recording);
            let _ = looper.try_with_hotkey_service(|hk| {
                hk.start_recording();
                if let Some(app) = app_for_recording.upgrade() {
                    app.set_recording_hotkey(true);
                }
            });
        });

        // Settings callbacks
        let settings_for_callbacks = settings.clone();
        let app_for_auto_start = app.clone();
        slint_app.on_toggle_auto_start(move || {
            if let Some(app) = app_for_auto_start.upgrade() {
                let new_val = !app.get_auto_start();
                match set_auto_start(new_val) {
                    Ok(()) => {
                        app.set_auto_start(new_val);
                        let mut s = settings_for_callbacks.clone();
                        s.auto_start = new_val;
                        s.save();
                    }
                    Err(e) => {
                        app.set_settings_error(SharedString::from(e));
                    }
                }
            }
        });

        let settings_for_callbacks = settings.clone();
        let app_for_auto_hide = app.clone();
        let looper_for_auto_hide = Arc::clone(&looper);
        slint_app.on_toggle_auto_hide(move || {
            if let Some(app) = app_for_auto_hide.upgrade() {
                let new_val = !app.get_auto_hide();
                app.set_auto_hide(new_val);
                let mut s = settings_for_callbacks.clone();
                s.auto_hide = new_val;
                s.save();
                let _ = looper_for_auto_hide.try_with_focus_service(|fs| {
                    fs.set_auto_hide(new_val);
                });
            }
        });

        let app_for_pinned = app.clone();
        let looper_for_pinned = Arc::clone(&looper);
        slint_app.on_toggle_pinned(move || {
            if let Some(app) = app_for_pinned.upgrade() {
                let new_val = !app.get_pinned();
                app.set_pinned(new_val);
                let _ = looper_for_pinned.try_with_focus_service(|fs| {
                    fs.set_pinned(new_val);
                });
            }
        });

        let settings_for_callbacks = settings.clone();
        let app_for_sort = app.clone();
        let looper_for_sort = Arc::clone(&looper);
        slint_app.on_toggle_sort_by_created(move || {
            if let Some(app) = app_for_sort.upgrade() {
                let new_val = !app.get_sort_by_created();
                app.set_sort_by_created(new_val);
                let mut s = settings_for_callbacks.clone();
                s.sort_by_created = new_val;
                s.save();
                let _ = looper_for_sort.try_with_clipboard_service(|cs| {
                    cs.set_sort_and_refresh(new_val);
                });
            }
        });

        let settings_for_callbacks = settings.clone();
        let app_for_pick_db = app.clone();
        slint_app.on_pick_db_path(move || {
            let result = rfd::FileDialog::new()
                .set_file_name("clippi.db")
                .save_file();

            if let Some(new_path) = result {
                if let Some(app) = app_for_pick_db.upgrade() {
                    let old_path = settings_for_callbacks.resolve_db_path();
                    match migrate_database(&old_path, &new_path) {
                        Ok(()) => {
                            let path_str = new_path.to_string_lossy().to_string();
                            let mut s = settings_for_callbacks.clone();
                            s.db_path = path_str;
                            s.save();
                            spawn_new_process();
                            slint::quit_event_loop().ok();
                        }
                        Err(e) => {
                            app.set_settings_error(SharedString::from(e));
                        }
                    }
                }
            }
        });

        let settings_for_callbacks = settings.clone();
        let app_for_reset_db = app.clone();
        slint_app.on_reset_db_path(move || {
            let old_path = settings_for_callbacks.resolve_db_path();
            let default_db_path = AppSettings::default().resolve_db_path();
            if old_path == default_db_path {
                return;
            }
            match migrate_database(&old_path, &default_db_path) {
                Ok(()) => {
                    let mut s = settings_for_callbacks.clone();
                    s.db_path = String::new();
                    s.save();
                    spawn_new_process();
                    slint::quit_event_loop().ok();
                }
                Err(e) => {
                    if let Some(app) = app_for_reset_db.upgrade() {
                        app.set_settings_error(SharedString::from(e));
                    }
                }
            }
        });

        let settings_for_callbacks = settings.clone();
        let app_for_set_theme = app.clone();
        slint_app.on_set_theme(move |mode: i32| {
            if let Some(app) = app_for_set_theme.upgrade() {
                let is_dark = match mode {
                    1 => true,
                    2 => false,
                    _ => is_system_dark_mode(),
                };
                app.set_theme_mode(mode);
                app.set_dark_mode(is_dark);
                let mut s = settings_for_callbacks.clone();
                s.theme = match mode {
                    1 => "dark".to_string(),
                    2 => "light".to_string(),
                    _ => "system".to_string(),
                };
                s.save();
            }
        });

        let settings_for_callbacks = settings.clone();
        let app_for_pos = app.clone();
        let frontend_for_pos = frontend.clone();
        slint_app.on_set_position_mode(move |mode: i32| {
            let mode_str = match mode {
                1 => "follow",
                2 => "remember",
                _ => "center",
            };
            if let Some(app) = app_for_pos.upgrade() {
                app.set_position_mode(mode);
            }
            if let Ok(mut fe) = frontend_for_pos.lock() {
                fe.set_position_mode(PositionMode::from_int(mode));
            }
            let mut s = settings_for_callbacks.clone();
            s.window_position_mode = mode_str.to_string();
            s.save();
        });

        // Filter callbacks
        let looper_for_filter = Arc::clone(&looper);
        let app_for_filter = app.clone();
        slint_app.on_toggle_filter(move |filter: SharedString| {
            let ft = filter.to_string();
            let _ = looper_for_filter.try_with_clipboard_service(|cs| {
                cs.toggle_filter_and_refresh(&ft);
                if let Some(app) = app_for_filter.upgrade() {
                    app.set_filter_plain_text(cs.is_filter_active("plain_text"));
                    app.set_filter_rich_text(cs.is_filter_active("rich_text"));
                    app.set_filter_image(cs.is_filter_active("image"));
                    app.set_filter_link(cs.is_filter_active("link"));
                }
            });
        });

        // Apply initial window position and suppress auto-hide before first show
        if let Ok(mut fe) = frontend.lock() {
            fe.apply_position();
            fe.set_initial_suppress();
        }

        Ok(Self {
            looper,
            listener: Some(listener),
            _settings: settings,
            frontend,
        })
    }

    pub fn shutdown(mut self) {
        if let Ok(fe) = self.frontend.lock() {
            let mut s = self._settings.clone();
            fe.apply_saved_position_to_settings(&mut s);
            s.save();
        }
        if let Some(mut listener) = self.listener.take() {
            listener.stop();
        }
        if let Ok(mut looper) = Arc::try_unwrap(self.looper) {
            looper.stop();
        }
    }
}

fn init_ui_from_settings(app: &App, settings: &AppSettings) {
    let is_dark = match settings.theme.as_str() {
        "dark" => true,
        "light" => false,
        _ => is_system_dark_mode(),
    };
    let theme_mode = match settings.theme.as_str() {
        "dark" => 1,
        "light" => 2,
        _ => 0,
    };

    app.set_dark_mode(is_dark);
    app.set_theme_mode(theme_mode);
    app.set_auto_start(settings.auto_start);
    app.set_auto_hide(settings.auto_hide);
    app.set_sort_by_created(settings.sort_by_created);
    app.set_db_path(SharedString::from(settings.resolve_db_path().to_string_lossy().to_string()));
    app.set_hotkey_display(SharedString::from(&settings.hotkey));
    app.set_position_mode(PositionMode::from_str(&settings.window_position_mode).to_int());
}
