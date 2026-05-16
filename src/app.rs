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

/// Pre-cloned shared state for callback closures.
/// All fields are cheap to clone (Arc or Weak).
#[derive(Clone)]
struct CallbackCtx {
    looper: Arc<Looper>,
    app: slint::Weak<App>,
    frontend: Arc<Mutex<Frontend>>,
    db: Arc<Mutex<Database>>,
    settings: Arc<Mutex<AppSettings>>,
    copy_as_plain_text: Arc<AtomicBool>,
    batch_pasting: Arc<AtomicBool>,
    clear_selection: Arc<AtomicBool>,
}

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
        let blacklist = settings.hotkey_blacklist.clone();
        let sort_by_created_setting = settings.sort_by_created;
        let copy_as_plain_text_setting = settings.copy_as_plain_text;
        let max_items_setting = settings.max_items;
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
        clipboard_service.set_max_items(max_items_setting);

        let mut listener = create_listener();
        listener.start(clipboard_service.shared())?;

        // Create shared foreground app name for blacklist coordination
        let foreground_app_name = crate::services::focus::shared_foreground_app_name();

        // Create hotkey service
        let mut hotkey_service = HotkeyService::new(
            frontend.clone(),
            app.clone(),
            foreground_app_name.clone(),
        );
        if let Ok(h) = create_hotkey_listener(&hotkey_str) {
            hotkey_service.set_hotkey(h);
        }
        hotkey_service.load_blacklist(&blacklist);
        // Bind blacklist model to Slint
        slint_app.set_hotkey_blacklist(hotkey_service.blacklist_model());

        // Create tray service (use shared frontend)
        let tray_service = TrayService::new(frontend.clone());

        // Create focus service
        let mut focus_service = match FocusService::new(
            frontend.clone(),
            app.clone(),
            foreground_app_name,
        ) {
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

        // Wrap looper in Arc for callback sharing
        #[allow(clippy::arc_with_non_send_sync)]
        let looper = Arc::new(looper);

        // Build callback context (all fields are cheap Arc/Weak clones)
        let ctx = CallbackCtx {
            looper: looper.clone(),
            app: app.clone(),
            frontend: frontend.clone(),
            db: db.clone(),
            settings: shared_settings.clone(),
            copy_as_plain_text: copy_as_plain_text_flag,
            batch_pasting: batch_pasting_flag,
            clear_selection: clear_selection_flag,
        };

        // Bind all Slint callbacks by category
        Self::bind_window_callbacks(slint_app, &ctx, &clipboard_service_for_callbacks);
        Self::bind_hotkey_callbacks(slint_app, &ctx);
        Self::bind_settings_callbacks(slint_app, &ctx);
        Self::bind_sync_callbacks(slint_app, &ctx);
        Self::bind_note_and_edit_callbacks(slint_app, &ctx);
        Self::bind_filter_callbacks(slint_app, &ctx);
        Self::bind_context_menu_callbacks(slint_app, &ctx);
        Self::bind_selection_callbacks(slint_app, &ctx);
        Self::bind_batch_callbacks(slint_app, &ctx);
        Self::bind_tag_callbacks(slint_app, &ctx);
        Self::bind_color_paste_callbacks(slint_app, &ctx);

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

    // ── Window callbacks: move, resize, copy, paste, close ──

    fn bind_window_callbacks(
        slint_app: &App,
        ctx: &CallbackCtx,
        cs: &ClipboardService,
    ) {
        let ctx_move = ctx.clone();
        slint_app.on_move_window(move |dx, dy| {
            if let Ok(fe) = ctx_move.frontend.lock() {
                fe.move_window(dx, dy);
            }
        });

        let ctx_resize = ctx.clone();
        slint_app.on_resize_window(move |dx, dy| {
            if let Some(app) = ctx_resize.app.upgrade() {
                let window = app.window();
                let scale = window.scale_factor();
                let s = window.size();
                let w = s.width as f32 / scale;
                let h = s.height as f32 / scale;
                let new_w = (w + dx).max(320.0);
                let new_h = (h + dy).max(480.0);
                window.set_size(LogicalSize::new(new_w, new_h));
                if let Ok(mut fe) = ctx_resize.frontend.lock() {
                    fe.set_saved_size(new_w, new_h);
                }
            }
        });

        let ctx_copy = ctx.clone();
        let mut cs_for_copy = cs.clone();
        slint_app.on_copy_item(move |id| {
            if let Ok(db) = ctx_copy.db.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    write_item_to_clipboard(&item, ctx_copy.copy_as_plain_text.load(Ordering::Relaxed));
                }
            }
            cs_for_copy.refresh_row(id);
        });

        let ctx_paste = ctx.clone();
        slint_app.on_paste_item(move |id| {
            let mut expected = String::new();
            let mut is_file = false;
            if let Ok(db) = ctx_paste.db.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    let plain = ctx_paste.copy_as_plain_text.load(Ordering::Relaxed);
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

        let ctx_close = ctx.clone();
        slint_app.on_close_window(move || {
            if let Some(app) = ctx_close.app.upgrade() {
                app.set_pinned(false);
            }
            if let Ok(mut fe) = ctx_close.frontend.lock() {
                fe.hide();
                let mut s = ctx_close.settings.lock().expect("settings lock poisoned");
                fe.apply_saved_position_to_settings(&mut s);
                s.save();
            }
        });
    }

    // ── Hotkey callbacks: set hotkey, start recording ──

    fn bind_hotkey_callbacks(slint_app: &App, ctx: &CallbackCtx) {
        let ctx_set = ctx.clone();
        slint_app.on_set_hotkey(move |s: SharedString| {
            let _ = ctx_set.looper.try_with_hotkey_service(|hk| {
                if let Err(e) = hk.update_hotkey(&s) {
                    if let Some(app) = ctx_set.app.upgrade() {
                        app.set_settings_error(SharedString::from(e));
                    }
                } else {
                    if let Ok(mut settings) = ctx_set.settings.lock() {
                        settings.hotkey = s.to_string();
                        settings.save();
                    }
                }
            });
        });

        let ctx_rec = ctx.clone();
        slint_app.on_start_recording_hotkey(move || {
            let _ = ctx_rec.looper.try_with_hotkey_service(|hk| {
                hk.start_recording();
                if let Some(app) = ctx_rec.app.upgrade() {
                    app.set_recording_hotkey(true);
                }
            });
        });

        // ── Blacklist callbacks ──

        // Show confirmation panel for the current foreground app
        let ctx_bl_show = ctx.clone();
        slint_app.on_show_hotkey_blacklist_panel(move || {
            if let Some(app) = ctx_bl_show.app.upgrade() {
                let fg_name = app.get_foreground_app_name();
                if fg_name.is_empty() {
                    return;
                }
                app.set_blacklist_pending_app(fg_name);
                app.set_hotkey_blacklist_panel_visible(true);
            }
        });

        // Hide confirmation panel
        let ctx_bl_hide = ctx.clone();
        slint_app.on_hide_hotkey_blacklist_panel(move || {
            if let Some(app) = ctx_bl_hide.app.upgrade() {
                app.set_hotkey_blacklist_panel_visible(false);
                app.set_blacklist_pending_app(SharedString::default());
            }
        });

        // Confirm add to blacklist
        let ctx_bl_confirm = ctx.clone();
        slint_app.on_confirm_add_blacklist(move || {
            if let Some(app) = ctx_bl_confirm.app.upgrade() {
                let pending = app.get_blacklist_pending_app();
                if pending.is_empty() {
                    return;
                }
                let _ = ctx_bl_confirm.looper.try_with_hotkey_service(|hk| {
                    if hk.add_to_blacklist(&pending) {
                        // Persist to settings
                        let blacklist = hk.blacklist_apps();
                        if let Ok(mut s) = ctx_bl_confirm.settings.lock() {
                            s.hotkey_blacklist = blacklist;
                            s.save();
                        }
                    }
                });
                app.set_hotkey_blacklist_panel_visible(false);
                app.set_blacklist_pending_app(SharedString::default());
            }
        });

        // Remove from blacklist
        let ctx_bl_remove = ctx.clone();
        slint_app.on_remove_hotkey_blacklist(move |app_name: SharedString| {
            let _ = ctx_bl_remove.looper.try_with_hotkey_service(|hk| {
                hk.remove_from_blacklist(&app_name);
                let blacklist = hk.blacklist_apps();
                if let Ok(mut s) = ctx_bl_remove.settings.lock() {
                    s.hotkey_blacklist = blacklist;
                    s.save();
                }
            });
        });
    }

    // ── Settings callbacks: all settings toggle/set operations ──

    fn bind_settings_callbacks(slint_app: &App, ctx: &CallbackCtx) {
        let c = ctx.clone();
        slint_app.on_toggle_auto_start(move || {
            if let Some(app) = c.app.upgrade() {
                let new_val = !app.get_auto_start();
                match set_auto_start(new_val) {
                    Ok(()) => {
                        app.set_auto_start(new_val);
                        let mut s = c.settings.lock().expect("settings lock poisoned");
                        s.auto_start = new_val;
                        s.save();
                    }
                    Err(e) => {
                        app.set_settings_error(SharedString::from(e));
                    }
                }
            }
        });

        let c = ctx.clone();
        slint_app.on_toggle_auto_hide(move || {
            if let Some(app) = c.app.upgrade() {
                let new_val = !app.get_auto_hide();
                app.set_auto_hide(new_val);
                let mut s = c.settings.lock().expect("settings lock poisoned");
                s.auto_hide = new_val;
                s.save();
                let _ = c.looper.try_with_focus_service(|fs| {
                    fs.set_auto_hide(new_val);
                });
            }
        });

        let c = ctx.clone();
        slint_app.on_toggle_silent_start(move || {
            if let Some(app) = c.app.upgrade() {
                let new_val = !app.get_silent_start();
                app.set_silent_start(new_val);
                let mut s = c.settings.lock().expect("settings lock poisoned");
                s.silent_start = new_val;
                s.save();
            }
        });

        let c = ctx.clone();
        slint_app.on_toggle_pinned(move || {
            if let Some(app) = c.app.upgrade() {
                let new_val = !app.get_pinned();
                app.set_pinned(new_val);
                let _ = c.looper.try_with_focus_service(|fs| {
                    fs.set_pinned(new_val);
                });
            }
        });

        let c = ctx.clone();
        slint_app.on_toggle_sort_by_created(move || {
            if let Some(app) = c.app.upgrade() {
                let new_val = !app.get_sort_by_created();
                app.set_sort_by_created(new_val);
                let mut s = c.settings.lock().expect("settings lock poisoned");
                s.sort_by_created = new_val;
                s.save();
                let _ = c.looper.try_with_clipboard_service(|cs| {
                    cs.set_sort_and_refresh(new_val);
                });
            }
        });

        let c = ctx.clone();
        slint_app.on_pick_db_path(move || {
            let result = rfd::FileDialog::new()
                .set_file_name("clippi.db")
                .save_file();

            if let Some(new_path) = result {
                if let Some(app) = c.app.upgrade() {
                    let old_path = c.settings.lock().expect("settings lock poisoned").resolve_db_path();
                    match migrate_database(&old_path, &new_path) {
                        Ok(()) => {
                            let path_str = new_path.to_string_lossy().to_string();
                            let mut s = c.settings.lock().expect("settings lock poisoned");
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

        let c = ctx.clone();
        slint_app.on_reset_db_path(move || {
            let old_path = c.settings.lock().expect("settings lock poisoned").resolve_db_path();
            let default_db_path = AppSettings::default().resolve_db_path();
            if old_path == default_db_path {
                return;
            }
            match migrate_database(&old_path, &default_db_path) {
                Ok(()) => {
                    let mut s = c.settings.lock().expect("settings lock poisoned");
                    s.db_path = String::new();
                    s.save();
                    spawn_new_process();
                    slint::quit_event_loop().ok();
                }
                Err(e) => {
                    if let Some(app) = c.app.upgrade() {
                        app.set_settings_error(SharedString::from(e));
                    }
                }
            }
        });

        let c = ctx.clone();
        slint_app.on_set_theme(move |mode: i32| {
            if let Some(app) = c.app.upgrade() {
                let is_dark = match mode {
                    1 => true,
                    2 => false,
                    _ => is_system_dark_mode(),
                };
                app.set_theme_mode(mode);
                app.set_dark_mode(is_dark);
                let mut s = c.settings.lock().expect("settings lock poisoned");
                s.theme = match mode {
                    1 => "dark".to_string(),
                    2 => "light".to_string(),
                    _ => "system".to_string(),
                };
                s.save();
            }
        });

        let c = ctx.clone();
        slint_app.on_set_position_mode(move |mode: i32| {
            let mode_str = match mode {
                1 => "follow",
                2 => "remember",
                _ => "center",
            };
            if let Some(app) = c.app.upgrade() {
                app.set_position_mode(mode);
            }
            if let Ok(mut fe) = c.frontend.lock() {
                fe.set_position_mode(PositionMode::from_int(mode));
            }
            let mut s = c.settings.lock().expect("settings lock poisoned");
            s.window_position_mode = mode_str.to_string();
            s.save();
        });

        let c = ctx.clone();
        slint_app.on_set_card_height_mode(move |mode: SharedString| {
            if let Some(app) = c.app.upgrade() {
                app.set_card_height_mode(mode.clone());
            }
            let mut s = c.settings.lock().expect("settings lock poisoned");
            s.card_height_mode = mode.to_string();
            s.save();
        });

        let c = ctx.clone();
        slint_app.on_toggle_show_source_app(move || {
            if let Some(app) = c.app.upgrade() {
                let new_val = !app.get_show_source_app();
                app.set_show_source_app(new_val);
                let mut s = c.settings.lock().expect("settings lock poisoned");
                s.show_source_app = new_val;
                s.save();
            }
        });

        let c = ctx.clone();
        slint_app.on_toggle_auto_scroll_to_top(move || {
            if let Some(app) = c.app.upgrade() {
                let new_val = !app.get_auto_scroll_to_top();
                app.set_auto_scroll_to_top(new_val);
                let mut s = c.settings.lock().expect("settings lock poisoned");
                s.auto_scroll_to_top = new_val;
                s.save();
            }
        });

        let c = ctx.clone();
        slint_app.on_toggle_copy_as_plain_text(move || {
            if let Some(app) = c.app.upgrade() {
                let new_val = !app.get_copy_as_plain_text();
                app.set_copy_as_plain_text(new_val);
                let mut s = c.settings.lock().expect("settings lock poisoned");
                s.copy_as_plain_text = new_val;
                s.save();
                c.copy_as_plain_text.store(new_val, Ordering::Relaxed);
                let _ = c.looper.try_with_clipboard_service(|cs| {
                    cs.set_copy_as_plain_text(new_val);
                });
            }
        });

        let c = ctx.clone();
        slint_app.on_toggle_show_original_on_hover(move || {
            if let Some(app) = c.app.upgrade() {
                let new_val = !app.get_show_original_on_hover();
                app.set_show_original_on_hover(new_val);
                let mut s = c.settings.lock().expect("settings lock poisoned");
                s.show_original_on_hover = new_val;
                s.save();
            }
        });

        let c = ctx.clone();
        slint_app.on_set_max_items(move |v: i32| {
            let v_u32 = if v < 0 { 0 } else { v as u32 };
            if let Some(app) = c.app.upgrade() {
                app.set_max_items(v);
            }
            let mut s = c.settings.lock().expect("settings lock poisoned");
            s.max_items = v_u32;
            s.save();
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.set_max_items(v_u32);
            });
        });
    }

    // ── Sync callbacks: auto-sync toggle, interval, manual sync, backend CRUD ──

    fn bind_sync_callbacks(slint_app: &App, ctx: &CallbackCtx) {
        let c = ctx.clone();
        slint_app.on_toggle_sync_auto_enabled(move || {
            if let Some(app) = c.app.upgrade() {
                let new_val = !app.get_sync_auto_enabled();
                app.set_sync_auto_enabled(new_val);
                let mut s = c.settings.lock().expect("settings lock poisoned");
                s.sync_auto_enabled = new_val;
                s.save();
            }
        });

        let c = ctx.clone();
        slint_app.on_set_sync_interval(move |secs: i32| {
            if let Some(app) = c.app.upgrade() {
                app.set_sync_interval_secs(secs);
                let mut s = c.settings.lock().expect("settings lock poisoned");
                s.sync_interval_secs = secs as u64;
                s.save();
            }
        });

        let c = ctx.clone();
        slint_app.on_sync_now(move || {
            let _ = c.looper.try_with_sync_manager(|sm| {
                sm.trigger_sync_now();
            });
        });

        let c = ctx.clone();
        slint_app.on_add_sync_backend(move |name: SharedString, path: SharedString| {
            let _ = c.looper.try_with_sync_manager(|sm| {
                sm.add_local_folder_backend(name.to_string(), path.to_string());
            });
        });

        let c = ctx.clone();
        slint_app.on_save_sync_backend(move |id: SharedString, name: SharedString, path: SharedString| {
            let _ = c.looper.try_with_sync_manager(|sm| {
                sm.edit_backend(&id, &name, &path);
            });
        });

        let c = ctx.clone();
        slint_app.on_remove_sync_backend(move |id: SharedString| {
            let _ = c.looper.try_with_sync_manager(|sm| {
                sm.remove_backend(&id);
            });
        });

        let c = ctx.clone();
        slint_app.on_toggle_sync_backend(move |id: SharedString| {
            let _ = c.looper.try_with_sync_manager(|sm| {
                sm.toggle_backend(&id);
            });
        });

        let c = ctx.clone();
        slint_app.on_edit_sync_backend(move |id: SharedString| {
            let _ = c.looper.try_with_sync_manager(|sm| {
                if let Some((name, folder)) = sm.get_backend_info(&id) {
                    if let Some(app) = c.app.upgrade() {
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

        let c = ctx.clone();
        slint_app.on_show_add_backend_panel(move || {
            if let Some(app) = c.app.upgrade() {
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

        let c = ctx.clone();
        slint_app.on_hide_add_backend_panel(move || {
            if let Some(app) = c.app.upgrade() {
                app.set_add_backend_panel_visible(false);
                app.set_add_backend_edit_mode(false);
            }
        });

        let c = ctx.clone();
        slint_app.on_pick_backend_folder(move || {
            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                if let Some(app) = c.app.upgrade() {
                    app.set_add_backend_edit_folder(SharedString::from(
                        folder.to_string_lossy().as_ref(),
                    ));
                }
            }
        });
    }

    // ── Note & edit callbacks: update note, edit item, save content ──

    fn bind_note_and_edit_callbacks(slint_app: &App, ctx: &CallbackCtx) {
        let c = ctx.clone();
        slint_app.on_update_note(move |id, text: SharedString| {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.update_note(id, &text);
            });
        });

        let c = ctx.clone();
        slint_app.on_edit_item(move |id| {
            if let Ok(db) = c.db.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    if let Some(app) = c.app.upgrade() {
                        app.set_editing_item_id(id);
                        app.set_editing_item_type(SharedString::from(item.content_type.as_str()));
                        app.set_editing_content(SharedString::from(item.full_text.clone()));
                        app.set_current_view(SharedString::from("edit"));
                    }
                }
            }
        });

        let c = ctx.clone();
        slint_app.on_save_content(move |id, text: SharedString| {
            let content_type = if is_url(&text) {
                "link"
            } else if is_path(&text) {
                "path"
            } else {
                "plain_text"
            };
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.update_content(id, &text, content_type);
            });
            if let Some(app) = c.app.upgrade() {
                app.set_current_view(SharedString::from("clipboard"));
            }
        });
    }

    // ── Filter callbacks: type filters, favorites, search ──

    fn bind_filter_callbacks(slint_app: &App, ctx: &CallbackCtx) {
        let c = ctx.clone();
        slint_app.on_toggle_filter(move |filter: SharedString| {
            let ft = filter.to_string();
            let _ = c.looper.try_with_clipboard_service(|cs| {
                if ft == "file" {
                    cs.toggle_file_filter_and_refresh();
                } else if ft == "link" {
                    cs.toggle_link_filter_and_refresh();
                } else {
                    cs.toggle_filter_and_refresh(&ft);
                }
                if let Some(app) = c.app.upgrade() {
                    app.set_filter_plain_text(cs.is_filter_active("plain_text"));
                    app.set_filter_rich_text(cs.is_filter_active("rich_text"));
                    app.set_filter_image(cs.is_filter_active("image"));
                    app.set_filter_link(cs.is_filter_active("link") || cs.is_filter_active("path"));
                    app.set_filter_color(cs.is_filter_active("color"));
                    app.set_filter_file(cs.is_filter_active("file") || cs.is_filter_active("image"));
                }
            });
        });

        let c = ctx.clone();
        slint_app.on_clear_filters(move || {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.clear_filters();
                if let Some(app) = c.app.upgrade() {
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

        let c = ctx.clone();
        slint_app.on_toggle_favorites_filter(move || {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.toggle_favorites_filter_and_refresh();
                if let Some(app) = c.app.upgrade() {
                    app.set_filter_favorites(cs.is_favorites_filter_active());
                }
            });
        });

        let c = ctx.clone();
        slint_app.on_toggle_favorite(move |id| {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.toggle_favorite(id);
            });
        });

        let c = ctx.clone();
        slint_app.on_delete_item(move |id| {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.delete_item(id);
            });
        });

        let c = ctx.clone();
        slint_app.on_search_keyword(move |keyword: SharedString| {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                if keyword.is_empty() {
                    cs.clear_keyword();
                } else {
                    cs.set_keyword(keyword.as_str());
                }
            });
        });
    }

    // ── Context menu callbacks ──

    fn bind_context_menu_callbacks(slint_app: &App, ctx: &CallbackCtx) {
        let c = ctx.clone();
        slint_app.on_show_context_menu(move |id| {
            if let Some(app) = c.app.upgrade() {
                let (is_color, is_hex, is_image, is_file, is_favorite) =
                    if let Ok(db) = c.db.lock() {
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

        let c = ctx.clone();
        slint_app.on_hide_context_menu(move || {
            if let Some(app) = c.app.upgrade() {
                app.set_context_menu_visible(false);
            }
        });
    }

    // ── Selection callbacks ──

    fn bind_selection_callbacks(slint_app: &App, ctx: &CallbackCtx) {
        let c = ctx.clone();
        slint_app.on_select_single(move |id| {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.select_single(id);
            });
        });

        let c = ctx.clone();
        slint_app.on_toggle_selection(move |id| {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.toggle_selection(id);
            });
        });

        let c = ctx.clone();
        slint_app.on_range_select(move |id| {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.range_select(id);
            });
        });

        let c = ctx.clone();
        slint_app.on_clear_selection(move || {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.clear_selection();
            });
        });
    }

    // ── Batch operation callbacks ──

    fn bind_batch_callbacks(slint_app: &App, ctx: &CallbackCtx) {
        let c = ctx.clone();
        slint_app.on_batch_paste(move || {
            let items = c.looper.try_with_clipboard_service(|cs| {
                cs.get_selected_items()
            }).unwrap_or_default();

            if items.is_empty() {
                return;
            }

            let plain_flag = c.copy_as_plain_text.load(Ordering::Relaxed);
            let owned_items: Vec<_> = items.into_iter().collect();
            let bp = c.batch_pasting.clone();
            let clear_sel = c.clear_selection.clone();

            std::thread::spawn(move || {
                bp.store(true, Ordering::SeqCst);
                batch_paste_sequential(&owned_items, plain_flag);
                bp.store(false, Ordering::SeqCst);
                clear_sel.store(true, Ordering::SeqCst);
            });
        });

        let c = ctx.clone();
        slint_app.on_batch_favorite(move || {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.batch_toggle_favorite();
            });
        });

        let c = ctx.clone();
        slint_app.on_batch_delete(move || {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.batch_delete();
            });
        });
    }

    // ── Tag callbacks: filter panel, picker, CRUD, batch operations ──

    fn bind_tag_callbacks(slint_app: &App, ctx: &CallbackCtx) {
        let c = ctx.clone();
        slint_app.on_show_tag_filter_panel(move || {
            if let Some(app) = c.app.upgrade() {
                if app.get_tag_filter_visible() {
                    app.set_tag_filter_visible(false);
                    return;
                }
                let _ = c.looper.try_with_clipboard_service(|cs| {
                    cs.load_all_tags_for_filter();
                });
                app.set_tag_picker_visible(false);
                app.set_tag_filter_y(106.0);
                app.set_tag_filter_visible(true);
            }
        });

        let c = ctx.clone();
        slint_app.on_hide_tag_filter_panel(move || {
            if let Some(app) = c.app.upgrade() {
                app.set_tag_filter_visible(false);
            }
        });

        let c = ctx.clone();
        slint_app.on_toggle_tag_filter(move |tag_id: i32| {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.toggle_tag_filter_and_refresh(tag_id as i64);
                cs.load_all_tags_for_filter();
                if let Some(app) = c.app.upgrade() {
                    app.set_has_tag_filter(cs.has_tag_filters());
                }
            });
        });

        let c = ctx.clone();
        slint_app.on_create_tag(move |name: SharedString| {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.create_tag(name.as_str());
                cs.load_all_tags_for_filter();
            });
        });

        let c = ctx.clone();
        slint_app.on_update_tag(move |tag_id: i32, name: SharedString, color: slint::Color| {
            let hex = format!("#{:02X}{:02X}{:02X}", color.red(), color.green(), color.blue());
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.update_tag(tag_id as i64, name.as_str(), &hex);
                cs.load_all_tags_for_filter();
                cs.refresh_with_current_filter();
            });
        });

        let c = ctx.clone();
        slint_app.on_delete_tag(move |tag_id: i32| {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.delete_tag(tag_id as i64);
                cs.load_all_tags_for_filter();
                cs.refresh_with_current_filter();
                if let Some(app) = c.app.upgrade() {
                    app.set_has_tag_filter(cs.has_tag_filters());
                }
            });
        });

        let c = ctx.clone();
        slint_app.on_show_tag_picker(move |item_id: i32| {
            let is_batch = c.app.upgrade().is_some_and(|app| app.get_context_menu_is_batch());
            let _ = c.looper.try_with_clipboard_service(|cs| {
                if is_batch {
                    cs.load_all_tags_for_batch_picker();
                } else {
                    cs.load_all_tags_for_picker(item_id);
                }
            });
            if let Some(app) = c.app.upgrade() {
                app.set_tag_filter_visible(false);
                let (px, py) = (app.get_context_menu_x(), app.get_context_menu_y());
                app.set_tag_picker_x(px);
                app.set_tag_picker_y(py);
                app.set_tag_picker_item_id(item_id);
                app.set_tag_picker_is_batch(is_batch);
                app.set_tag_picker_visible(true);
            }
        });

        let c = ctx.clone();
        slint_app.on_hide_tag_picker(move || {
            if let Some(app) = c.app.upgrade() {
                app.set_tag_picker_visible(false);
            }
        });

        let c = ctx.clone();
        slint_app.on_create_and_add_tag(move |item_id: i32, name: SharedString| {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.create_and_add_tag(item_id, name.as_str());
                cs.load_all_tags_for_picker(item_id);
            });
        });

        let c = ctx.clone();
        slint_app.on_toggle_item_tag(move |item_id: i32, tag_id: i32| {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.toggle_item_tag(item_id, tag_id as i64);
                cs.load_all_tags_for_picker(item_id);
            });
        });

        let c = ctx.clone();
        slint_app.on_batch_add_tag(move |tag_id: i32| {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.batch_add_tag(tag_id as i64);
                if let Some(app) = c.app.upgrade() {
                    app.set_tag_picker_visible(false);
                    app.set_selected_count(0);
                }
            });
        });

        let c = ctx.clone();
        slint_app.on_batch_remove_tag(move |tag_id: i32| {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.batch_remove_tag(tag_id as i64);
                if let Some(app) = c.app.upgrade() {
                    app.set_tag_picker_visible(false);
                    app.set_selected_count(0);
                }
            });
        });

        let c = ctx.clone();
        slint_app.on_clear_all_tags(move || {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                if let Some(app) = c.app.upgrade() {
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
    }

    // ── Color paste callbacks: paste as RGB / paste as HEX ──

    fn bind_color_paste_callbacks(slint_app: &App, ctx: &CallbackCtx) {
        let c = ctx.clone();
        slint_app.on_paste_as_rgb(move |id| {
            if let Ok(db) = c.db.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    if let Some(color) = detect_color(&item.full_text) {
                        let rgb_text = color.to_rgb();
                        if let Ok(clip_ctx) = ClipboardContext::new() {
                            let _ = Clipboard::set_text(&clip_ctx, rgb_text);
                        }
                    }
                }
            }
            restore_paste_target();
            paste_after_delay();
            if let Some(app) = c.app.upgrade() {
                app.set_context_menu_visible(false);
            }
        });

        let c = ctx.clone();
        slint_app.on_paste_as_hex(move |id| {
            if let Ok(db) = c.db.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    if let Some(color) = detect_color(&item.full_text) {
                        let hex_text = color.to_css_hex();
                        if let Ok(clip_ctx) = ClipboardContext::new() {
                            let _ = Clipboard::set_text(&clip_ctx, hex_text);
                        }
                    }
                }
            }
            restore_paste_target();
            paste_after_delay();
            if let Some(app) = c.app.upgrade() {
                app.set_context_menu_visible(false);
            }
        });
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
    app.set_max_items(settings.max_items as i32);
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
