//! AppController - assembles all services and sets up the application

use crate::core::color::{detect_color, is_hex_format};
use crate::core::db::Database;
use crate::core::frontend::{Frontend, PositionMode};
use crate::core::settings::{
    is_system_dark_mode, migrate_database, set_auto_start, spawn_new_process,
    AppSettings,
};
use crate::core::types::{is_url, is_path, ContentType, RichData};
use crate::looper::Looper;
use crate::platform::clipboard::{create_listener, ClipboardShared};
use crate::platform::monitor::get_cursor_pos;
use crate::platform::paste::{paste_after_delay, paste_sync, restore_paste_target};
use crate::platform::hotkey::create_hotkey_listener;
use crate::services::clipboard::ClipboardService;
use crate::services::focus::FocusService;
use crate::services::hotkey::HotkeyService;
use crate::services::sync::SyncManager;
use crate::services::tray::TrayService;
use crate::App;
use clipboard_rs::{Clipboard, ClipboardContext, ClipboardContent, common::RustImageData};
use clipboard_rs::common::RustImage;
use slint::{ComponentHandle, LogicalSize, SharedString};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct AppController {
    looper: Arc<Looper>,
    listener: Option<Box<dyn crate::platform::clipboard::ClipboardListener>>,
    frontend: Arc<Mutex<Frontend>>,
    shared_settings: Arc<Mutex<AppSettings>>,
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
        frontend.set_saved_size(settings.saved_window_width, settings.saved_window_height);
        let frontend = Arc::new(Mutex::new(frontend));
        let app = slint_app.as_weak();

        // Initialize UI with settings
        init_ui_from_settings(slint_app, &settings);

        // Wrap in Arc<Mutex<>> so callbacks and shutdown share live settings
        let auto_hide_setting = settings.auto_hide;
        let hotkey_str = settings.hotkey.clone();
        let sort_by_created_setting = settings.sort_by_created;
        let copy_as_plain_text_setting = settings.copy_as_plain_text;
        let copy_as_plain_text_flag = Arc::new(AtomicBool::new(copy_as_plain_text_setting));
        let shared_settings = Arc::new(Mutex::new(settings));

        // Create clipboard service (sets up model in new())
        let clipboard_shared = ClipboardShared::new();
        let batch_pasting_flag = clipboard_shared.batch_pasting.clone();
        let clear_selection_flag = clipboard_shared.clear_selection_requested.clone();
        let sync_dirty_flag = Arc::new(AtomicBool::new(false));
        let mut clipboard_service = ClipboardService::new(
            clipboard_shared,
            db.clone(),
            app.clone(),
            sync_dirty_flag.clone(),
        );
        let clipboard_service_for_callbacks = clipboard_service.clone();
        // Load initial data and apply settings
        clipboard_service.load_initial();
        clipboard_service.set_sort_and_refresh(sort_by_created_setting);
        clipboard_service.set_copy_as_plain_text(copy_as_plain_text_setting);

        let mut listener = create_listener();
        listener.start(clipboard_service.shared())?;

        // Create hotkey service
        let mut hotkey_service = HotkeyService::new(frontend.clone(), app.clone());
        if let Ok(h) = create_hotkey_listener(&hotkey_str) {
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
        focus_service.set_auto_hide(auto_hide_setting);

        // Create sync manager (uses the same dirty flag as ClipboardService)
        let sync_manager = SyncManager::new(
            db.clone(),
            shared_settings.clone(),
            app.clone(),
            sync_dirty_flag,
        );

        // Detect preset paths for AddBackendPanel
        let presets = crate::services::backends::local_folder::detect_presets();
        for (name, path) in &presets {
            match *name {
                "OneDrive" => slint_app.set_preset_onedrive_path(SharedString::from(path)),
                "iCloud" => slint_app.set_preset_icloud_path(SharedString::from(path)),
                _ => {}
            }
        }

        // Create and start looper
        let mut looper = Looper::new();
        looper.add_service(Box::new(tray_service));
        looper.set_clipboard_service(clipboard_service);
        looper.set_hotkey_service(hotkey_service);
        looper.set_focus_service(focus_service);
        looper.set_sync_manager(sync_manager);
        looper.start();

        // ========== Bind Slint callbacks ==========

        let frontend_clone = frontend.clone();
        slint_app.on_move_window(move |dx, dy| {
            if let Ok(fe) = frontend_clone.lock() {
                fe.move_window(dx, dy);
            }
        });

        // Window resize — delta from resize handles in app.slint
        let app_for_resize = app.clone();
        let frontend_for_resize = frontend.clone();
        slint_app.on_resize_window(move |dx, dy| {
            if let Some(app) = app_for_resize.upgrade() {
                let window = app.window();
                let scale = window.scale_factor();
                let s = window.size();
                let w = s.width as f32 / scale;
                let h = s.height as f32 / scale;
                let new_w = (w + dx).max(320.0);
                let new_h = (h + dy).max(480.0);
                window.set_size(LogicalSize::new(new_w, new_h));
                if let Ok(mut fe) = frontend_for_resize.lock() {
                    fe.set_saved_size(new_w, new_h);
                }
            }
        });

        // Copy item to clipboard (with format restoration when rich_data is present)
        let db_for_copy = db.clone();
        let cs_for_copy = clipboard_service_for_callbacks.clone();
        let plain_flag_for_copy = Arc::clone(&copy_as_plain_text_flag);
        slint_app.on_copy_item(move |id| {
            if let Ok(db) = db_for_copy.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    write_item_to_clipboard(&item, plain_flag_for_copy.load(Ordering::Relaxed));
                }
            }
            cs_for_copy.refresh_row(id);
        });

        // Paste item - copy to clipboard, restore focus, then paste
        // FocusService will auto-hide if not pinned
        let db_for_paste = db.clone();
        let plain_flag_for_paste = Arc::clone(&copy_as_plain_text_flag);
        slint_app.on_paste_item(move |id| {
            let mut expected = String::new();
            let mut is_file = false;
            if let Ok(db) = db_for_paste.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    let plain = plain_flag_for_paste.load(Ordering::Relaxed);
                    expected = item.full_text.clone();
                    is_file = item.content_type == ContentType::File;
                    write_item_to_clipboard(&item, plain);
                }
            }
            if !expected.is_empty() && !is_file {
                verify_clipboard_content(&expected, 200);
            }
            if is_file {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            restore_paste_target();
            paste_after_delay();
        });

        let app_for_close = app.clone();
        let frontend_close = frontend.clone();
        let settings_for_close = Arc::clone(&shared_settings);
        slint_app.on_close_window(move || {
            if let Some(app) = app_for_close.upgrade() {
                app.set_pinned(false);
            }
            if let Ok(mut fe) = frontend_close.lock() {
                fe.hide();
                let mut s = settings_for_close.lock().expect("settings lock poisoned");
                fe.apply_saved_position_to_settings(&mut s);
                s.save();
            }
        });

        // Hotkey callbacks
        #[allow(clippy::arc_with_non_send_sync)]
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
        let settings_for_callbacks = Arc::clone(&shared_settings);
        let app_for_auto_start = app.clone();
        slint_app.on_toggle_auto_start(move || {
            if let Some(app) = app_for_auto_start.upgrade() {
                let new_val = !app.get_auto_start();
                match set_auto_start(new_val) {
                    Ok(()) => {
                        app.set_auto_start(new_val);
                        let mut s = settings_for_callbacks.lock().expect("settings lock poisoned");
                        s.auto_start = new_val;
                        s.save();
                    }
                    Err(e) => {
                        app.set_settings_error(SharedString::from(e));
                    }
                }
            }
        });

        let settings_for_callbacks = Arc::clone(&shared_settings);
        let app_for_auto_hide = app.clone();
        let looper_for_auto_hide = Arc::clone(&looper);
        slint_app.on_toggle_auto_hide(move || {
            if let Some(app) = app_for_auto_hide.upgrade() {
                let new_val = !app.get_auto_hide();
                app.set_auto_hide(new_val);
                let mut s = settings_for_callbacks.lock().expect("settings lock poisoned");
                s.auto_hide = new_val;
                s.save();
                let _ = looper_for_auto_hide.try_with_focus_service(|fs| {
                    fs.set_auto_hide(new_val);
                });
            }
        });

        let settings_for_callbacks = Arc::clone(&shared_settings);
        let app_for_silent_start = app.clone();
        slint_app.on_toggle_silent_start(move || {
            if let Some(app) = app_for_silent_start.upgrade() {
                let new_val = !app.get_silent_start();
                app.set_silent_start(new_val);
                let mut s = settings_for_callbacks.lock().expect("settings lock poisoned");
                s.silent_start = new_val;
                s.save();
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

        let settings_for_callbacks = Arc::clone(&shared_settings);
        let app_for_sort = app.clone();
        let looper_for_sort = Arc::clone(&looper);
        slint_app.on_toggle_sort_by_created(move || {
            if let Some(app) = app_for_sort.upgrade() {
                let new_val = !app.get_sort_by_created();
                app.set_sort_by_created(new_val);
                let mut s = settings_for_callbacks.lock().expect("settings lock poisoned");
                s.sort_by_created = new_val;
                s.save();
                let _ = looper_for_sort.try_with_clipboard_service(|cs| {
                    cs.set_sort_and_refresh(new_val);
                });
            }
        });

        let settings_for_callbacks = Arc::clone(&shared_settings);
        let app_for_pick_db = app.clone();
        slint_app.on_pick_db_path(move || {
            let result = rfd::FileDialog::new()
                .set_file_name("clippi.db")
                .save_file();

            if let Some(new_path) = result {
                if let Some(app) = app_for_pick_db.upgrade() {
                    let old_path = settings_for_callbacks.lock().expect("settings lock poisoned").resolve_db_path();
                    match migrate_database(&old_path, &new_path) {
                        Ok(()) => {
                            let path_str = new_path.to_string_lossy().to_string();
                            let mut s = settings_for_callbacks.lock().expect("settings lock poisoned");
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

        let settings_for_callbacks = Arc::clone(&shared_settings);
        let app_for_reset_db = app.clone();
        slint_app.on_reset_db_path(move || {
            let old_path = settings_for_callbacks.lock().expect("settings lock poisoned").resolve_db_path();
            let default_db_path = AppSettings::default().resolve_db_path();
            if old_path == default_db_path {
                return;
            }
            match migrate_database(&old_path, &default_db_path) {
                Ok(()) => {
                    let mut s = settings_for_callbacks.lock().expect("settings lock poisoned");
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

        let settings_for_callbacks = Arc::clone(&shared_settings);
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
                let mut s = settings_for_callbacks.lock().expect("settings lock poisoned");
                s.theme = match mode {
                    1 => "dark".to_string(),
                    2 => "light".to_string(),
                    _ => "system".to_string(),
                };
                s.save();
            }
        });

        let settings_for_callbacks = Arc::clone(&shared_settings);
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
            let mut s = settings_for_callbacks.lock().expect("settings lock poisoned");
            s.window_position_mode = mode_str.to_string();
            s.save();
        });

        let settings_for_callbacks = Arc::clone(&shared_settings);
        let app_for_chm = app.clone();
        slint_app.on_set_card_height_mode(move |mode: SharedString| {
            if let Some(app) = app_for_chm.upgrade() {
                app.set_card_height_mode(mode.clone());
            }
            let mut s = settings_for_callbacks.lock().expect("settings lock poisoned");
            s.card_height_mode = mode.to_string();
            s.save();
        });

        // Show source app callback
        let settings_for_callbacks = Arc::clone(&shared_settings);
        let app_for_source = app.clone();
        slint_app.on_toggle_show_source_app(move || {
            if let Some(app) = app_for_source.upgrade() {
                let new_val = !app.get_show_source_app();
                app.set_show_source_app(new_val);
                let mut s = settings_for_callbacks.lock().expect("settings lock poisoned");
                s.show_source_app = new_val;
                s.save();
            }
        });

        // Auto scroll to top callback
        let settings_for_callbacks = Arc::clone(&shared_settings);
        let app_for_scroll = app.clone();
        slint_app.on_toggle_auto_scroll_to_top(move || {
            if let Some(app) = app_for_scroll.upgrade() {
                let new_val = !app.get_auto_scroll_to_top();
                app.set_auto_scroll_to_top(new_val);
                let mut s = settings_for_callbacks.lock().expect("settings lock poisoned");
                s.auto_scroll_to_top = new_val;
                s.save();
            }
        });

        // Copy as plain text callback
        let settings_for_callbacks = Arc::clone(&shared_settings);
        let app_for_copy_plain = app.clone();
        let looper_for_copy_plain = Arc::clone(&looper);
        let plain_flag_for_toggle = Arc::clone(&copy_as_plain_text_flag);
        slint_app.on_toggle_copy_as_plain_text(move || {
            if let Some(app) = app_for_copy_plain.upgrade() {
                let new_val = !app.get_copy_as_plain_text();
                app.set_copy_as_plain_text(new_val);
                let mut s = settings_for_callbacks.lock().expect("settings lock poisoned");
                s.copy_as_plain_text = new_val;
                s.save();
                plain_flag_for_toggle.store(new_val, Ordering::Relaxed);
                let _ = looper_for_copy_plain.try_with_clipboard_service(|cs| {
                    cs.set_copy_as_plain_text(new_val);
                });
            }
        });

        // Show original on hover callback
        let settings_for_callbacks = Arc::clone(&shared_settings);
        let app_for_hover = app.clone();
        slint_app.on_toggle_show_original_on_hover(move || {
            if let Some(app) = app_for_hover.upgrade() {
                let new_val = !app.get_show_original_on_hover();
                app.set_show_original_on_hover(new_val);
                let mut s = settings_for_callbacks.lock().expect("settings lock poisoned");
                s.show_original_on_hover = new_val;
                s.save();
            }
        });

        // ── Sync callbacks ──

        // Toggle auto-sync
        let settings_for_sync_auto = Arc::clone(&shared_settings);
        let app_for_sync_auto = app.clone();
        slint_app.on_toggle_sync_auto_enabled(move || {
            if let Some(app) = app_for_sync_auto.upgrade() {
                let new_val = !app.get_sync_auto_enabled();
                app.set_sync_auto_enabled(new_val);
                let mut s = settings_for_sync_auto.lock().expect("settings lock poisoned");
                s.sync_auto_enabled = new_val;
                s.save();
            }
        });

        // Set sync interval (global)
        let settings_for_sync_interval = Arc::clone(&shared_settings);
        let app_for_sync_interval = app.clone();
        slint_app.on_set_sync_interval(move |secs: i32| {
            if let Some(app) = app_for_sync_interval.upgrade() {
                app.set_sync_interval_secs(secs);
                let mut s = settings_for_sync_interval.lock().expect("settings lock poisoned");
                s.sync_interval_secs = secs as u64;
                s.save();
            }
        });

        // Manual sync now
        let looper_for_sync_now = Arc::clone(&looper);
        slint_app.on_sync_now(move || {
            let _ = looper_for_sync_now.try_with_sync_manager(|sm| {
                sm.trigger_sync_now();
            });
        });

        // Show add-backend floating panel
        let app_for_show_panel = app.clone();
        slint_app.on_show_add_backend_panel(move || {
            if let Some(app) = app_for_show_panel.upgrade() {
                if let Some(pos) = get_cursor_pos() {
                    app.set_add_backend_panel_x(pos.0 as f32);
                    app.set_add_backend_panel_y(pos.1 as f32);
                }
                app.set_add_backend_edit_mode(false);
                app.set_add_backend_edit_name(SharedString::default());
                app.set_add_backend_edit_folder(SharedString::default());
                app.set_add_backend_panel_visible(true);
            }
        });

        // Hide add-backend floating panel
        let app_for_hide_panel = app.clone();
        slint_app.on_hide_add_backend_panel(move || {
            if let Some(app) = app_for_hide_panel.upgrade() {
                app.set_add_backend_panel_visible(false);
                app.set_add_backend_edit_mode(false);
            }
        });

        // Add backend from floating panel
        let looper_for_add_backend = Arc::clone(&looper);
        slint_app.on_add_sync_backend(move |name: SharedString, path: SharedString| {
            let _ = looper_for_add_backend.try_with_sync_manager(|sm| {
                sm.add_local_folder_backend(name.to_string(), path.to_string());
            });
        });

        // Pick folder for backend path
        let app_for_pick_folder = app.clone();
        slint_app.on_pick_backend_folder(move || {
            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                if let Some(app) = app_for_pick_folder.upgrade() {
                    app.set_add_backend_edit_folder(SharedString::from(
                        folder.to_string_lossy().as_ref(),
                    ));
                }
            }
        });

        // Save backend from floating panel (edit mode)
        let looper_for_save_backend = Arc::clone(&looper);
        slint_app.on_save_sync_backend(move |id: SharedString, name: SharedString, path: SharedString| {
            let _ = looper_for_save_backend.try_with_sync_manager(|sm| {
                sm.edit_backend(&id, &name, &path);
            });
        });

        // Remove backend
        let looper_for_remove_backend = Arc::clone(&looper);
        slint_app.on_remove_sync_backend(move |id: SharedString| {
            let _ = looper_for_remove_backend.try_with_sync_manager(|sm| {
                sm.remove_backend(&id);
            });
        });

        // Toggle backend enabled/disabled
        let looper_for_toggle_backend = Arc::clone(&looper);
        slint_app.on_toggle_sync_backend(move |id: SharedString| {
            let _ = looper_for_toggle_backend.try_with_sync_manager(|sm| {
                sm.toggle_backend(&id);
            });
        });

        // Edit backend — show panel pre-filled with current config
        let looper_for_edit_backend = Arc::clone(&looper);
        let app_for_edit = app.clone();
        slint_app.on_edit_sync_backend(move |id: SharedString| {
            let _ = looper_for_edit_backend.try_with_sync_manager(|sm| {
                if let Some((name, folder)) = sm.get_backend_info(&id) {
                    if let Some(app) = app_for_edit.upgrade() {
                        if let Some(pos) = get_cursor_pos() {
                            app.set_add_backend_panel_x(pos.0 as f32);
                            app.set_add_backend_panel_y(pos.1 as f32);
                        }
                        app.set_add_backend_edit_id(id.clone());
                        app.set_add_backend_edit_name(SharedString::from(&name));
                        app.set_add_backend_edit_folder(SharedString::from(&folder));
                        app.set_add_backend_edit_mode(true);
                        app.set_add_backend_panel_visible(true);
                    }
                }
            });
        });

        // Update note callback
        let looper_for_note = Arc::clone(&looper);
        slint_app.on_update_note(move |id, text: SharedString| {
            let _ = looper_for_note.try_with_clipboard_service(|cs| {
                cs.update_note(id, &text);
            });
        });

        // Edit item callback — load content and switch to edit view
        let db_for_edit = db.clone();
        let app_for_edit = app.clone();
        slint_app.on_edit_item(move |id| {
            if let Ok(db) = db_for_edit.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    if let Some(app) = app_for_edit.upgrade() {
                        app.set_editing_item_id(id);
                        app.set_editing_item_type(SharedString::from(item.content_type.as_str()));
                        app.set_editing_content(SharedString::from(item.full_text.clone()));
                        app.set_current_view(SharedString::from("edit"));
                    }
                }
            }
        });

        // Save content callback — update item and switch back to clipboard view
        let looper_for_save = Arc::clone(&looper);
        let app_for_save = app.clone();
        slint_app.on_save_content(move |id, text: SharedString| {
            let content_type = if is_url(&text) {
                "link"
            } else if is_path(&text) {
                "path"
            } else {
                "plain_text"
            };
            let _ = looper_for_save.try_with_clipboard_service(|cs| {
                cs.update_content(id, &text, content_type);
            });
            if let Some(app) = app_for_save.upgrade() {
                app.set_current_view(SharedString::from("clipboard"));
            }
        });

        // Filter callbacks
        let looper_for_filter = Arc::clone(&looper);
        let app_for_filter = app.clone();
        slint_app.on_toggle_filter(move |filter: SharedString| {
            let ft = filter.to_string();
            let _ = looper_for_filter.try_with_clipboard_service(|cs| {
                if ft == "file" {
                    cs.toggle_file_filter_and_refresh();
                } else if ft == "link" {
                    cs.toggle_link_filter_and_refresh();
                } else {
                    cs.toggle_filter_and_refresh(&ft);
                }
                if let Some(app) = app_for_filter.upgrade() {
                    app.set_filter_plain_text(cs.is_filter_active("plain_text"));
                    app.set_filter_rich_text(cs.is_filter_active("rich_text"));
                    app.set_filter_image(cs.is_filter_active("image"));
                    app.set_filter_link(cs.is_filter_active("link") || cs.is_filter_active("path"));
                    app.set_filter_color(cs.is_filter_active("color"));
                    app.set_filter_file(cs.is_filter_active("file") || cs.is_filter_active("image"));
                }
            });
        });

        let looper_for_clear = Arc::clone(&looper);
        let app_for_clear = app.clone();
        slint_app.on_clear_filters(move || {
            let _ = looper_for_clear.try_with_clipboard_service(|cs| {
                cs.clear_filters();
                if let Some(app) = app_for_clear.upgrade() {
                    app.set_filter_plain_text(false);
                    app.set_filter_rich_text(false);
                    app.set_filter_image(false);
                    app.set_filter_link(false);
                    app.set_filter_color(false);
                    app.set_filter_file(false);
                    app.set_filter_favorites(false);
                    app.set_has_tag_filter(false);
                }
            });
        });

        // Favorites filter callback
        let looper_for_fav_filter = Arc::clone(&looper);
        let app_for_fav_filter = app.clone();
        slint_app.on_toggle_favorites_filter(move || {
            let _ = looper_for_fav_filter.try_with_clipboard_service(|cs| {
                cs.toggle_favorites_filter_and_refresh();
                if let Some(app) = app_for_fav_filter.upgrade() {
                    app.set_filter_favorites(cs.is_favorites_filter_active());
                }
            });
        });

        // Toggle favorite on a single item
        let looper_for_fav = Arc::clone(&looper);
        slint_app.on_toggle_favorite(move |id| {
            let _ = looper_for_fav.try_with_clipboard_service(|cs| {
                cs.toggle_favorite(id);
            });
        });

        // Delete item
        let looper_for_del = Arc::clone(&looper);
        slint_app.on_delete_item(move |id| {
            let _ = looper_for_del.try_with_clipboard_service(|cs| {
                cs.delete_item(id);
            });
        });

        // Search callback
        let looper_for_search = Arc::clone(&looper);
        slint_app.on_search_keyword(move |keyword: SharedString| {
            let _ = looper_for_search.try_with_clipboard_service(|cs| {
                if keyword.is_empty() {
                    cs.clear_keyword();
                } else {
                    cs.set_keyword(keyword.as_str());
                }
            });
        });

        // ── Context menu callbacks ──

        // Show context menu: read item from DB, determine type info,
        // get cursor position, and set Slint properties for the overlay.
        let db_for_ctx = db.clone();
        let app_for_ctx = app.clone();
        slint_app.on_show_context_menu(move |id| {
            if let Some(app) = app_for_ctx.upgrade() {
                let (is_color, is_hex, is_image, is_file, is_favorite) =
                    if let Ok(db) = db_for_ctx.lock() {
                        if let Ok(Some(item)) = db.get_by_id(id as i64) {
                            let color = item.content_type == ContentType::Color;
                            let hex = color && is_hex_format(&item.full_text);
                            let img = item.content_type == ContentType::Image;
                            let file = item.content_type == ContentType::File;
                            let fav = item.is_favorite;
                            (color, hex, img, file, fav)
                        } else {
                            (false, false, false, false, false)
                        }
                    } else {
                        (false, false, false, false, false)
                    };

                // Get cursor position in screen coordinates, convert to window-relative
                let (cursor_x, cursor_y) = get_cursor_pos().unwrap_or((0, 0));
                let pos = app.window().position();
                let scale = app.window().scale_factor();
                let client_x = (cursor_x as f32 - pos.x as f32) / scale;
                let client_y = (cursor_y as f32 - pos.y as f32) / scale;
                app.set_context_menu_x(client_x);
                app.set_context_menu_y(client_y);

                app.set_context_menu_item_id(id);
                app.set_context_menu_is_color(is_color);
                app.set_context_menu_is_hex(is_hex);
                app.set_context_menu_is_image(is_image);
                app.set_context_menu_is_file(is_file);
                app.set_context_menu_is_favorite(is_favorite);
                app.set_context_menu_visible(true);
            }
        });

        let app_for_hide_ctx = app.clone();
        slint_app.on_hide_context_menu(move || {
            if let Some(app) = app_for_hide_ctx.upgrade() {
                app.set_context_menu_visible(false);
            }
        });

        // ── Selection callbacks ──

        let looper_for_select = Arc::clone(&looper);
        slint_app.on_select_single(move |id| {
            let _ = looper_for_select.try_with_clipboard_service(|cs| {
                cs.select_single(id);
            });
        });

        let looper_for_toggle = Arc::clone(&looper);
        slint_app.on_toggle_selection(move |id| {
            let _ = looper_for_toggle.try_with_clipboard_service(|cs| {
                cs.toggle_selection(id);
            });
        });

        let looper_for_range = Arc::clone(&looper);
        slint_app.on_range_select(move |id| {
            let _ = looper_for_range.try_with_clipboard_service(|cs| {
                cs.range_select(id);
            });
        });

        let looper_for_clear_sel = Arc::clone(&looper);
        slint_app.on_clear_selection(move || {
            let _ = looper_for_clear_sel.try_with_clipboard_service(|cs| {
                cs.clear_selection();
            });
        });

        // ── Batch operation callbacks ──

        // Batch paste: paste selected items sequentially in selection order
        let looper_for_batch_paste = Arc::clone(&looper);
        let plain_flag_for_batch = Arc::clone(&copy_as_plain_text_flag);
        let bp_flag = batch_pasting_flag.clone();
        let clear_sel_flag = clear_selection_flag.clone();
        slint_app.on_batch_paste(move || {
            let items = looper_for_batch_paste.try_with_clipboard_service(|cs| {
                cs.get_selected_items()
            }).unwrap_or_default();

            if items.is_empty() {
                return;
            }

            let plain_flag = plain_flag_for_batch.load(Ordering::Relaxed);
            let owned_items: Vec<_> = items.into_iter().collect();
            let bp = bp_flag.clone();
            let clear_sel = clear_sel_flag.clone();

            std::thread::spawn(move || {
                bp.store(true, Ordering::SeqCst);
                batch_paste_sequential(&owned_items, plain_flag);
                bp.store(false, Ordering::SeqCst);

                // Signal looper to clear selection on next poll (main thread)
                clear_sel.store(true, Ordering::SeqCst);
            });
        });

        // Batch toggle favorite
        let looper_for_batch_fav = Arc::clone(&looper);
        slint_app.on_batch_favorite(move || {
            let _ = looper_for_batch_fav.try_with_clipboard_service(|cs| {
                cs.batch_toggle_favorite();
            });
        });

        // Batch delete
        let looper_for_batch_del = Arc::clone(&looper);
        slint_app.on_batch_delete(move || {
            let _ = looper_for_batch_del.try_with_clipboard_service(|cs| {
                cs.batch_delete();
            });
        });

        // ── Tag callbacks ──

        // Show tag filter panel
        let looper_for_tag_filter_panel = Arc::clone(&looper);
        let app_for_tag_filter_panel = app.clone();
        slint_app.on_show_tag_filter_panel(move || {
            if let Some(app) = app_for_tag_filter_panel.upgrade() {
                // Toggle: close if already visible, open if hidden
                if app.get_tag_filter_visible() {
                    app.set_tag_filter_visible(false);
                    return;
                }
                let _ = looper_for_tag_filter_panel.try_with_clipboard_service(|cs| {
                    cs.load_all_tags_for_filter();
                });
                app.set_tag_picker_visible(false);
                // Fixed position: centered horizontally (via Slint), aligned with card list top
                app.set_tag_filter_y(106.0);
                app.set_tag_filter_visible(true);
            }
        });

        // Hide tag filter panel
        let app_for_hide_tag_filter = app.clone();
        slint_app.on_hide_tag_filter_panel(move || {
            if let Some(app) = app_for_hide_tag_filter.upgrade() {
                app.set_tag_filter_visible(false);
            }
        });

        // Toggle tag filter
        let looper_for_toggle_tag = Arc::clone(&looper);
        let app_for_toggle_tag = app.clone();
        slint_app.on_toggle_tag_filter(move |tag_id: i32| {
            let _ = looper_for_toggle_tag.try_with_clipboard_service(|cs| {
                cs.toggle_tag_filter_and_refresh(tag_id as i64);
                cs.load_all_tags_for_filter();
                if let Some(app) = app_for_toggle_tag.upgrade() {
                    app.set_has_tag_filter(cs.has_tag_filters());
                }
            });
        });

        // Create tag from filter panel
        let looper_for_create_tag = Arc::clone(&looper);
        slint_app.on_create_tag(move |name: SharedString| {
            let _ = looper_for_create_tag.try_with_clipboard_service(|cs| {
                cs.create_tag(name.as_str());
                cs.load_all_tags_for_filter();
            });
        });

        // Update tag from filter panel
        let looper_for_update_tag = Arc::clone(&looper);
        slint_app.on_update_tag(move |tag_id: i32, name: SharedString, color: slint::Color| {
            let hex = format!("#{:02X}{:02X}{:02X}", color.red(), color.green(), color.blue());
            let _ = looper_for_update_tag.try_with_clipboard_service(|cs| {
                cs.update_tag(tag_id as i64, name.as_str(), &hex);
                cs.load_all_tags_for_filter();
                cs.refresh_with_current_filter();
            });
        });

        // Delete tag from filter panel
        let looper_for_delete_tag = Arc::clone(&looper);
        let app_for_delete_tag = app.clone();
        slint_app.on_delete_tag(move |tag_id: i32| {
            let _ = looper_for_delete_tag.try_with_clipboard_service(|cs| {
                cs.delete_tag(tag_id as i64);
                cs.load_all_tags_for_filter();
                cs.refresh_with_current_filter();
                if let Some(app) = app_for_delete_tag.upgrade() {
                    app.set_has_tag_filter(cs.has_tag_filters());
                }
            });
        });

        // Show tag picker (from context menu)
        let looper_for_show_picker = Arc::clone(&looper);
        let app_for_show_picker = app.clone();
        slint_app.on_show_tag_picker(move |item_id: i32| {
            let is_batch = {
                if let Some(app) = app_for_show_picker.upgrade() {
                    app.get_context_menu_is_batch()
                } else {
                    false
                }
            };
            let _ = looper_for_show_picker.try_with_clipboard_service(|cs| {
                if is_batch {
                    cs.load_all_tags_for_batch_picker();
                } else {
                    cs.load_all_tags_for_picker(item_id);
                }
            });
            if let Some(app) = app_for_show_picker.upgrade() {
                app.set_tag_filter_visible(false);
                let (px, py) = (app.get_context_menu_x(), app.get_context_menu_y());
                app.set_tag_picker_x(px);
                app.set_tag_picker_y(py);
                app.set_tag_picker_item_id(item_id);
                app.set_tag_picker_is_batch(is_batch);
                app.set_tag_picker_visible(true);
            }
        });

        // Hide tag picker
        let app_for_hide_picker = app.clone();
        slint_app.on_hide_tag_picker(move || {
            if let Some(app) = app_for_hide_picker.upgrade() {
                app.set_tag_picker_visible(false);
            }
        });

        // Create tag and add to item (from picker)
        let looper_for_create_add = Arc::clone(&looper);
        slint_app.on_create_and_add_tag(move |item_id: i32, name: SharedString| {
            let _ = looper_for_create_add.try_with_clipboard_service(|cs| {
                cs.create_and_add_tag(item_id, name.as_str());
                cs.load_all_tags_for_picker(item_id);
            });
        });

        // Toggle tag on item
        let looper_for_toggle_item_tag = Arc::clone(&looper);
        slint_app.on_toggle_item_tag(move |item_id: i32, tag_id: i32| {
            let _ = looper_for_toggle_item_tag.try_with_clipboard_service(|cs| {
                cs.toggle_item_tag(item_id, tag_id as i64);
                cs.load_all_tags_for_picker(item_id);
            });
        });

        // Batch add tag
        let looper_for_batch_add_tag = Arc::clone(&looper);
        let app_for_batch_add_tag = app.clone();
        slint_app.on_batch_add_tag(move |tag_id: i32| {
            let _ = looper_for_batch_add_tag.try_with_clipboard_service(|cs| {
                cs.batch_add_tag(tag_id as i64);
                if let Some(app) = app_for_batch_add_tag.upgrade() {
                    app.set_tag_picker_visible(false);
                    app.set_selected_count(0);
                }
            });
        });

        // Batch remove tag
        let looper_for_batch_rem_tag = Arc::clone(&looper);
        let app_for_batch_rem_tag = app.clone();
        slint_app.on_batch_remove_tag(move |tag_id: i32| {
            let _ = looper_for_batch_rem_tag.try_with_clipboard_service(|cs| {
                cs.batch_remove_tag(tag_id as i64);
                if let Some(app) = app_for_batch_rem_tag.upgrade() {
                    app.set_tag_picker_visible(false);
                    app.set_selected_count(0);
                }
            });
        });

        // Clear all tags from current item / selected items
        let looper_for_clear_tags = Arc::clone(&looper);
        let app_for_clear_tags = app.clone();
        slint_app.on_clear_all_tags(move || {
            let _ = looper_for_clear_tags.try_with_clipboard_service(|cs| {
                if let Some(app) = app_for_clear_tags.upgrade() {
                    if app.get_tag_picker_is_batch() {
                        cs.clear_selected_tags();
                        app.set_tag_picker_visible(false);
                        app.set_selected_count(0);
                    } else {
                        cs.clear_item_tags(app.get_tag_picker_item_id());
                        cs.load_all_tags_for_picker(app.get_tag_picker_item_id());
                    }
                }
            });
        });

        // Paste as RGB: convert HEX color to rgb(r,g,b) format and paste
        let db_for_rgb = db.clone();
        let app_for_rgb = app.clone();
        slint_app.on_paste_as_rgb(move |id| {
            if let Ok(db) = db_for_rgb.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    if let Some(color) = detect_color(&item.full_text) {
                        let rgb_text = color.to_rgb();
                        if let Ok(ctx) = ClipboardContext::new() {
                            let _ = Clipboard::set_text(&ctx, rgb_text);
                        }
                    }
                }
            }
            restore_paste_target();
            paste_after_delay();
            if let Some(app) = app_for_rgb.upgrade() {
                app.set_context_menu_visible(false);
            }
        });

        // Paste as HEX: convert RGB color to #RRGGBB format and paste
        let db_for_hex = db.clone();
        let app_for_hex = app.clone();
        slint_app.on_paste_as_hex(move |id| {
            if let Ok(db) = db_for_hex.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    if let Some(color) = detect_color(&item.full_text) {
                        let hex_text = color.to_css_hex();
                        if let Ok(ctx) = ClipboardContext::new() {
                            let _ = Clipboard::set_text(&ctx, hex_text);
                        }
                    }
                }
            }
            restore_paste_target();
            paste_after_delay();
            if let Some(app) = app_for_hex.upgrade() {
                app.set_context_menu_visible(false);
            }
        });

        // Apply initial window position and suppress auto-hide before first show
        if let Ok(mut fe) = frontend.lock() {
            fe.apply_position();
            fe.set_initial_suppress();
        }

        Ok(Self {
            looper,
            listener: Some(listener),
            shared_settings,
            frontend,
        })
    }

    pub fn shutdown(mut self) {
        if let Ok(fe) = self.frontend.lock() {
            let mut s = self.shared_settings.lock().expect("settings lock poisoned");
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
    app.set_card_height_mode(SharedString::from(&settings.card_height_mode));
    app.set_silent_start(settings.silent_start);
    app.set_show_source_app(settings.show_source_app);
    app.set_auto_scroll_to_top(settings.auto_scroll_to_top);
    app.set_copy_as_plain_text(settings.copy_as_plain_text);
    app.set_show_original_on_hover(settings.show_original_on_hover);
    app.set_sync_auto_enabled(settings.sync_auto_enabled);
    app.set_sync_interval_secs(settings.sync_interval_secs as i32);
}

/// Write a clipboard item's content to the system clipboard.
/// When copy_as_plain_text is true, only plain text is written; otherwise
/// HTML and RTF formats are also restored from rich_data.
/// For images, the PNG file is loaded and written as an image format.
/// For files, file paths are written via ClipboardContent::Files (CF_HDROP).
fn write_item_to_clipboard(item: &crate::core::types::ClipboardItem, copy_as_plain_text: bool) {
    if let Ok(ctx) = ClipboardContext::new() {
        if item.content_type == ContentType::Image && !item.image_path.is_empty() {
            // Write image + file path text
            if let Ok(img_data) = RustImageData::from_path(&item.image_path) {
                let mut contents = vec![
                    ClipboardContent::Text(item.full_text.clone()),
                    ClipboardContent::Image(img_data),
                ];
                // Also add Files for drag-and-drop compatibility
                contents.push(ClipboardContent::Files(vec![item.image_path.clone()]));
                let _ = Clipboard::set(&ctx, contents);
            }
        } else if item.content_type == ContentType::File && !item.file_data.is_empty() {
            // Write file paths to clipboard (CF_HDROP on Windows)
            let file_data = crate::core::types::FileData::from_json(&item.file_data);
            let paths: Vec<String> = file_data.files.iter().map(|f| f.path.clone()).collect();
            let contents = vec![ClipboardContent::Files(paths)];
            let _ = Clipboard::set(&ctx, contents);
        } else if copy_as_plain_text {
            let _ = Clipboard::set_text(&ctx, item.full_text.clone());
        } else {
            let mut contents = vec![ClipboardContent::Text(item.full_text.clone())];
            let rich = RichData::from_json(&item.rich_data);
            if let Some(html) = rich.html {
                contents.push(ClipboardContent::Html(html));
            }
            if let Some(rtf) = rich.rtf {
                contents.push(ClipboardContent::Rtf(rtf));
            }
            let _ = Clipboard::set(&ctx, contents);
        }
    }
}

/// Sequential batch paste with clipboard verification and newline separators.
/// For each non-first item, a literal `\n` is pasted first to move the cursor to a
/// new line, then the actual item is written and pasted. This works for all content
/// types (text, rich text, images) because the newline is a separate paste operation.
fn batch_paste_sequential(items: &[crate::core::types::ClipboardItem], plain_flag: bool) {
    let n = items.len();
    for (i, item) in items.iter().enumerate() {
        // For non-first items, paste a newline to move to the next line.
        // Uses paste_sync (synchronous) to avoid race: the async paste_after_delay
        // spawns a thread with 50ms+ delay, meaning the main thread could overwrite
        // the clipboard before the paste actually fires.
        if i > 0 {
            if let Ok(ctx) = ClipboardContext::new() {
                let _ = Clipboard::set_text(&ctx, "\n".to_string());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            restore_paste_target();
            paste_sync();
            std::thread::sleep(std::time::Duration::from_millis(60));
        }

        let expected = item.full_text.clone();
        write_item_to_clipboard(item, plain_flag);

        // Verify clipboard content before pasting (up to 300ms timeout)
        // Skip text verification for File items (file paths, not text-based)
        if item.content_type != ContentType::File {
            if !verify_clipboard_content(&expected, 300) {
                eprintln!("[WARN] batch_paste: clipboard verification timed out for item {}", item.id);
            }
        } else {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        restore_paste_target();
        paste_after_delay();

        // Wait for target app to process the paste before writing next item
        if i < n - 1 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

/// Poll-read clipboard text until it matches expected or timeout expires.
fn verify_clipboard_content(expected: &str, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        if let Ok(ctx) = ClipboardContext::new() {
            if let Ok(text) = ctx.get_text() {
                if text == expected {
                    return true;
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
