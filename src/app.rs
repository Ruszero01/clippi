//! AppController - assembles all services and sets up the application

use crate::core::color::{detect_color, is_hex_format};
use crate::core::db::Database;
use crate::core::frontend::{Frontend, PositionMode};
use crate::core::i18n;
use crate::core::settings::{
    is_system_dark_mode, migrate_database, set_auto_start, spawn_new_process, AppSettings,
};
use crate::core::types::{ContentType, RichData};
use crate::looper::Looper;
use crate::platform::clipboard::{create_listener, ClipboardShared};
use crate::platform::hotkey::create_hotkey_listener;
#[cfg(target_os = "macos")]
use crate::platform::monitor::get_cursor_pos;
use crate::platform::paste::{paste_after_delay, paste_sync, restore_paste_target};
use crate::services::clipboard::ClipboardService;
use crate::services::focus::FocusService;
use crate::services::hotkey::HotkeyService;
use crate::services::sync::SyncManager;
use crate::services::tray::TrayService;
use base64::Engine;
use crate::App;
use clipboard_rs::{Clipboard, ClipboardContent, ClipboardContext};
use slint::{ComponentHandle, LogicalSize, SharedString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use percent_encoding::percent_decode_str;

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
    shared: ClipboardShared,
}

pub struct AppController {
    looper: Arc<Looper>,
    listener: Option<Box<dyn crate::platform::clipboard::ClipboardListener>>,
    frontend: Arc<Mutex<Frontend>>,
    shared_settings: Arc<Mutex<AppSettings>>,
    db: Arc<Mutex<Database>>,
    restart_requested: Arc<AtomicBool>,
}

impl AppController {
    pub fn new(slint_app: &App) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Migrate legacy files from exe dir to platform data dir
        crate::core::paths::migrate_legacy_files();

        // Load settings
        let settings = AppSettings::load();

        // Initialize images cache directory (follows db_path if set)
        crate::core::paths::init_images_dir(&settings.db_path);

        // Initialize shared state
        let db_path = settings.resolve_db_path();
        let db = Arc::new(Mutex::new(Database::open(db_path.to_str().unwrap())?));

        // Extract values before wrapping settings in Arc
        let auto_hide_setting = settings.auto_hide;
        let hotkey_str = settings.hotkey.clone();
        let blacklist = settings.hotkey_blacklist.clone();
        let sort_by_created_setting = settings.sort_by_created;
        let copy_as_plain_text_setting = settings.copy_as_plain_text;
        let ocr_enabled_setting = settings.ocr_enabled;
        let qr_enabled_setting = settings.qr_enabled;
        let max_items_setting = settings.max_items;
        let pinned_tag_ids_setting = settings.pinned_tag_ids.clone();
        let copy_as_plain_text_flag = Arc::new(AtomicBool::new(copy_as_plain_text_setting));
        let window_position_mode = settings.window_position_mode.clone();
        let saved_window_x = settings.saved_window_x;
        let saved_window_y = settings.saved_window_y;
        let saved_window_width = settings.saved_window_width;
        let saved_window_height = settings.saved_window_height;

        let shared_settings = Arc::new(Mutex::new(settings));

        let needs_reload_flag = Arc::new(AtomicBool::new(false));
        let needs_release_flag = Arc::new(AtomicBool::new(false));
        let mut frontend = Frontend::new(
            slint_app,
            needs_reload_flag.clone(),
            needs_release_flag.clone(),
            shared_settings.clone(),
        );
        frontend.set_position_mode(PositionMode::from_str(&window_position_mode));
        frontend.set_saved_position(saved_window_x, saved_window_y);
        frontend.set_saved_size(saved_window_width, saved_window_height);
        let frontend = Arc::new(Mutex::new(frontend));
        let app = slint_app.as_weak();

        // Initialize UI with settings
        {
            let s = shared_settings.lock().expect("settings lock poisoned");
            init_ui_from_settings(slint_app, &s);
        }

        // Create clipboard service (sets up model in new())
        let clipboard_shared = ClipboardShared::new();
        let callback_shared = clipboard_shared.clone();
        let batch_pasting_flag = clipboard_shared.batch_pasting.clone();
        let clear_selection_flag = clipboard_shared.clear_selection_requested.clone();
        let sync_dirty_flag = Arc::new(AtomicBool::new(false));
        let needs_model_refresh_flag = Arc::new(AtomicBool::new(false));
        let mut clipboard_service = ClipboardService::new(
            clipboard_shared,
            db.clone(),
            app.clone(),
            sync_dirty_flag.clone(),
            needs_model_refresh_flag.clone(),
            needs_release_flag.clone(),
            needs_reload_flag.clone(),
        );
        // Load initial data and apply settings
        clipboard_service.set_sort_and_refresh(sort_by_created_setting);
        clipboard_service.set_copy_as_plain_text(copy_as_plain_text_setting);
        clipboard_service.set_ocr_enabled(ocr_enabled_setting);
        clipboard_service.set_qr_enabled(qr_enabled_setting);
        clipboard_service.set_max_items(max_items_setting);
        clipboard_service.set_pinned_tag_ids(pinned_tag_ids_setting.clone());
        clipboard_service.refresh_sidebar_tags(&pinned_tag_ids_setting);

        let mut listener = create_listener();
        listener.start(clipboard_service.shared())?;

        // Create shared foreground app name for blacklist coordination
        let foreground_app_name = crate::services::focus::shared_foreground_app_name();

        // Create hotkey service
        let mut hotkey_service =
            HotkeyService::new(frontend.clone(), app.clone(), foreground_app_name.clone(), shared_settings.clone());
        if let Ok(h) = create_hotkey_listener(&hotkey_str) {
            hotkey_service.set_hotkey(h);
        }
        hotkey_service.load_blacklist(&blacklist);
        // Bind blacklist model to Slint
        slint_app.set_hotkey_blacklist(hotkey_service.blacklist_model());

        // Create tray service (use shared frontend)
        let restart_requested = Arc::new(AtomicBool::new(false));
        let tray_service = TrayService::new(frontend.clone(), restart_requested.clone());

        // Create focus service
        let mut focus_service =
            match FocusService::new(frontend.clone(), app.clone(), foreground_app_name) {
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
            needs_model_refresh_flag,
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
            shared: callback_shared,
        };

        // Bind all Slint callbacks by category
        Self::bind_window_callbacks(slint_app, &ctx);
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

        // Trigger an initial pull on startup to fetch latest remote data.
        // The semantic hash gate ensures no unnecessary pushes.
        let _ = ctx.looper.try_with_sync_manager(|sm| {
            sm.trigger_pull_all();
        });

        // Apply initial window position and suppress auto-hide before first show
        if let Ok(mut fe) = frontend.lock() {
            fe.apply_position();
            fe.set_initial_suppress();
        }

        // Spawn background update check (fire-and-forget, non-critical)
        {
            use crate::services::update::UpdateChecker;
            let checker =
                UpdateChecker::new(env!("CARGO_PKG_VERSION"), "Ruszero01", "clippi");
            std::thread::spawn(move || {
                if let Some(info) = checker.check() {
                    log::info!(
                        "Update available: {} -> {} ({})",
                        checker.current_version(),
                        info.latest_version,
                        info.html_url,
                    );
                }
            });
        }

        // Trim working set after startup — initialization (Slint renderer,
        // font loading, DB setup) allocates temporary memory that the
        // allocator won't return to the OS without a hint.
        crate::platform::util::trim_process_working_set();

        // Run cache cleanup in background — removes orphaned images and
        // expired icon caches without blocking startup.
        {
            let db = db.clone();
            std::thread::spawn(move || {
                crate::core::cache_cleanup::cleanup_unused_cache(&db.lock().expect("db lock"));
            });
        }

        Ok(Self {
            looper,
            listener: Some(listener),
            shared_settings,
            frontend,
            db,
            restart_requested,
        })
    }

    /// Prepare for restart: flush WAL, save settings, release hotkey.
    /// Call before spawning a new process.
    pub fn prepare_restart(&self) {
        let _ = self.db.lock().expect("db lock").checkpoint();
        self.shared_settings.lock().expect("settings lock").save();
        let _ = self
            .looper
            .try_with_hotkey_service(|hk| hk.unregister_hotkey());
    }

    pub fn restart_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.restart_requested)
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
        self.looper.stop();
    }

    // ── Window callbacks: move, resize, copy, paste, close ──

    fn bind_window_callbacks(slint_app: &App, ctx: &CallbackCtx) {
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
                // Clamp delta to prevent runaway window expansion
                let dx = dx.clamp(-200.0, 600.0);
                let dy = dy.clamp(-200.0, 600.0);
                let new_w = (w + dx).clamp(360.0, 1200.0);
                let new_h = (h + dy).clamp(480.0, 1200.0);
                window.set_size(LogicalSize::new(new_w, new_h));
                if let Ok(mut fe) = ctx_resize.frontend.lock() {
                    fe.set_saved_size(new_w, new_h);
                }
            }
        });

        let ctx_copy = ctx.clone();
        let looper_for_copy = ctx.looper.clone();
        slint_app.on_copy_item(move |id| {
            if let Ok(db) = ctx_copy.db.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    write_item_to_clipboard(
                        &item,
                        ctx_copy.copy_as_plain_text.load(Ordering::Relaxed),
                        &ctx_copy.shared,
                    );
                }
            }
            let _ = looper_for_copy.try_with_clipboard_service(|cs| {
                cs.refresh_row(id);
            });
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
                    write_item_to_clipboard(&item, plain, &ctx_paste.shared);
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

        let ctx_open = ctx.clone();
        slint_app.on_open_original_image(move |id| {
            if let Ok(db) = ctx_open.db.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    if !item.image_path.is_empty() {
                        #[cfg(target_os = "windows")]
                        {
                            use windows_sys::Win32::UI::Shell::ShellExecuteW;
                            use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;
                            let path_utf16: Vec<u16> = item
                                .image_path
                                .encode_utf16()
                                .chain(std::iter::once(0))
                                .collect();
                            let operation: Vec<u16> = "open\0".encode_utf16().collect();
                            unsafe {
                                ShellExecuteW(
                                    std::ptr::null_mut(),
                                    operation.as_ptr(),
                                    path_utf16.as_ptr(),
                                    std::ptr::null(),
                                    std::ptr::null(),
                                    SW_SHOW,
                                );
                            }
                        }
                        #[cfg(target_os = "macos")]
                        {
                            let _ = std::process::Command::new("open")
                                .arg(&item.image_path)
                                .spawn();
                        }
                        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                        {
                            let _ = std::process::Command::new("xdg-open")
                                .arg(&item.image_path)
                                .spawn();
                        }
                    }
                }
            }
        });

        let ctx_qr = ctx.clone();
        slint_app.on_qr_action(move |id| {
            if let Ok(db) = ctx_qr.db.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    let rd = RichData::from_json(&item.rich_data);
                    if let Some(qr_text) = rd.qr_text {
                        if qr_text.starts_with("http://") || qr_text.starts_with("https://") {
                            crate::services::update::open_releases_page(&qr_text);
                        } else {
                            if let Ok(clip) = ClipboardContext::new() {
                                let _ = clip.set_text(qr_text);
                            }
                            if let Some(app) = ctx_qr.app.upgrade() {
                                app.set_toast_visible(false);
                                app.set_toast_message(SharedString::from(i18n::tr(
                                    "识别结果已写入剪贴板",
                                    "QR code content copied to clipboard",
                                )));
                                app.set_toast_visible(true);
                            }
                        }
                    }
                }
            }
        });

        let ctx_paste_ocr = ctx.clone();
        slint_app.on_paste_ocr(move |id| {
            // Load item and check for cached OCR text (release DB lock before running OCR)
            let ocr_load = {
                if let Ok(db) = ctx_paste_ocr.db.lock() {
                    match db.get_by_id(id as i64) {
                        Ok(Some(item)) => {
                            let rd = crate::core::types::RichData::from_json(&item.rich_data);
                            match rd.ocr_text.filter(|t| !t.trim().is_empty()) {
                                Some(cached) => Some((cached, true, String::new(), String::new())),
                                None if item.image_path.is_empty() => None,
                                None => Some((String::new(), false, item.image_path.clone(), item.rich_data.clone())),
                            }
                        }
                        Ok(None) => None,
                        Err(e) => {
                            log::error!("paste_ocr: db error: {:?}", e);
                            None
                        }
                    }
                } else {
                    None
                }
            };

            let ocr_text = match ocr_load {
                Some((cached, true, _, _)) => Some(cached),
                Some((_, false, ref img_path, ref existing_rich)) => {
                    let engine = crate::core::ocr::create_ocr_engine();
                    match engine.recognize(std::path::Path::new(img_path)) {
                        Ok(text) if !text.trim().is_empty() => {
                            if let Ok(db) = ctx_paste_ocr.db.lock() {
                                let mut new_rd = crate::core::types::RichData::from_json(existing_rich);
                                new_rd.ocr_text = Some(text.clone());
                                let _ = db.update_rich_data(id as i64, &new_rd.to_json());
                            }
                            Some(text)
                        }
                        Ok(_) => None,
                        Err(e) => {
                            log::error!("OCR error for item {}: {}", id, e);
                            None
                        }
                    }
                }
                None => None,
            };

            if let Some(text) = ocr_text {
                if let Ok(ctx) = clipboard_rs::ClipboardContext::new() {
                    ctx_paste_ocr
                        .shared
                        .skip_next
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    let _ = ctx.set_text(text);
                }
                if ctx_paste_ocr.copy_as_plain_text.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }
                restore_paste_target();
                paste_after_delay();
            }
        });

        let ctx_qr_detect = ctx.clone();
        slint_app.on_qr_detect(move |id| {
            let qr_text = if let Ok(db) = ctx_qr_detect.db.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    let rd = RichData::from_json(&item.rich_data);
                    if let Some(cached) = rd.qr_text {
                        Some(cached)
                    } else if !item.image_path.is_empty() {
                        match crate::core::qr::detect_qr(std::path::Path::new(&item.image_path))
                        {
                            Ok(Some(text)) => {
                                let mut rd2 = RichData::from_json(&item.rich_data);
                                rd2.qr_text = Some(text.clone());
                                let _ = db.update_rich_data(id as i64, &rd2.to_json());
                                Some(text)
                            }
                            _ => None,
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(text) = qr_text {
                if text.starts_with("http://") || text.starts_with("https://") {
                    crate::services::update::open_releases_page(&text);
                } else {
                    if let Ok(clip) = ClipboardContext::new() {
                        let _ = clip.set_text(text);
                    }
                    if let Some(app) = ctx_qr_detect.app.upgrade() {
                        app.set_toast_visible(false);
                        app.set_toast_message(SharedString::from(i18n::tr(
                            "识别结果已写入剪贴板",
                            "QR code content copied to clipboard",
                        )));
                        app.set_toast_visible(true);
                    }
                }
            } else if let Some(app) = ctx_qr_detect.app.upgrade() {
                app.set_toast_visible(false);
                app.set_toast_message(SharedString::from(i18n::tr(
                    "未识别到二维码",
                    "No QR code detected",
                )));
                app.set_toast_visible(true);
            }
        });

        let ctx_open_loc = ctx.clone();
        slint_app.on_open_item_location(move |id| {
            use crate::core::types::{ContentType, FileData};
            if let Ok(db) = ctx_open_loc.db.lock() {
                if let Ok(Some(item)) = db.get_by_id(id as i64) {
                    match item.content_type {
                        ContentType::Link | ContentType::Path => {
                            let target = item.full_text.clone();
                            #[cfg(target_os = "windows")]
                            {
                                let target_utf16: Vec<u16> =
                                    target.encode_utf16().chain(std::iter::once(0)).collect();
                                unsafe {
                                    use windows_sys::Win32::UI::Shell::ShellExecuteW;
                                    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;
                                    ShellExecuteW(
                                        std::ptr::null_mut(),
                                        "open\0".encode_utf16().collect::<Vec<u16>>().as_ptr(),
                                        target_utf16.as_ptr(),
                                        std::ptr::null(),
                                        std::ptr::null(),
                                        SW_SHOW,
                                    );
                                }
                            }
                            #[cfg(target_os = "macos")]
                            {
                                let _ = std::process::Command::new("open").arg(&target).spawn();
                            }
                            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                            {
                                let _ = std::process::Command::new("xdg-open").arg(&target).spawn();
                            }
                        }
                        ContentType::File => {
                            let file_data = FileData::from_json(&item.file_data);
                            if let Some(first) = file_data.files.first() {
                                #[cfg(target_os = "windows")]
                                {
                                    let arg = format!("/select,\"{}\"", first.path);
                                    let arg_utf16: Vec<u16> =
                                        arg.encode_utf16().chain(std::iter::once(0)).collect();
                                    unsafe {
                                        use windows_sys::Win32::UI::Shell::ShellExecuteW;
                                        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;
                                        ShellExecuteW(
                                            std::ptr::null_mut(),
                                            "open\0".encode_utf16().collect::<Vec<u16>>().as_ptr(),
                                            "explorer\0"
                                                .encode_utf16()
                                                .collect::<Vec<u16>>()
                                                .as_ptr(),
                                            arg_utf16.as_ptr(),
                                            std::ptr::null(),
                                            SW_SHOW,
                                        );
                                    }
                                }
                                #[cfg(target_os = "macos")]
                                {
                                    let parent = std::path::Path::new(&first.path).parent();
                                    if let Some(p) = parent {
                                        let _ = std::process::Command::new("open").arg(p).spawn();
                                    }
                                }
                                #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                                {
                                    let parent = std::path::Path::new(&first.path).parent();
                                    if let Some(p) = parent {
                                        let _ =
                                            std::process::Command::new("xdg-open").arg(p).spawn();
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        });

        // 此处不处理关闭逻辑，关闭逻辑统一在 Frontend::hide() / dismiss_ui() 中处理。
        let ctx_close = ctx.clone();
        slint_app.on_close_window(move || {
            if let Ok(mut fe) = ctx_close.frontend.lock() {
                fe.hide();
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
                    let old_path = c
                        .settings
                        .lock()
                        .expect("settings lock poisoned")
                        .resolve_db_path();
                    if let Err(e) = c.db.lock().expect("db lock").checkpoint() {
                        app.set_settings_error(SharedString::from(format!(
                            "{}: {e}",
                            i18n::tr("准备迁移失败", "Migration preparation failed")
                        )));
                        return;
                    }
                    match migrate_database(&old_path, &new_path) {
                        Ok(()) => {
                            let path_str = new_path.to_string_lossy().to_string();
                            let mut s = c.settings.lock().expect("settings lock poisoned");
                            s.db_path = path_str;
                            s.save();
                            drop(s); // release lock before do_restart tries to acquire it
                            do_restart(&c);
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
            let old_path = c
                .settings
                .lock()
                .expect("settings lock poisoned")
                .resolve_db_path();
            let default_db_path = AppSettings::default().resolve_db_path();
            if old_path == default_db_path {
                return;
            }
            if let Err(e) = c.db.lock().expect("db lock").checkpoint() {
                if let Some(app) = c.app.upgrade() {
                    app.set_settings_error(SharedString::from(format!(
                        "{}: {e}",
                        i18n::tr("准备迁移失败", "Migration preparation failed")
                    )));
                }
                return;
            }
            match migrate_database(&old_path, &default_db_path) {
                Ok(()) => {
                    let mut s = c.settings.lock().expect("settings lock poisoned");
                    s.db_path = String::new();
                    s.save();
                    drop(s); // release lock before do_restart tries to acquire it
                    do_restart(&c);
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
        slint_app.on_set_language(move |idx: i32| {
            let lang = match idx {
                1 => "en",
                _ => "zh_CN",
            };
            let current = c
                .settings
                .lock()
                .expect("settings lock poisoned")
                .language
                .clone();
            if lang == current {
                return;
            }
            // Save new preference, then ask for restart.
            {
                let mut s = c.settings.lock().expect("settings lock poisoned");
                s.language = lang.to_string();
                s.save();
            }
            let confirmed = rfd::MessageDialog::new()
                .set_title(crate::core::i18n::tr("重启应用", "Restart App"))
                .set_description(crate::core::i18n::tr(
                    "切换语言后需要重启应用才能完全生效，是否立即重启？",
                    "A restart is required to apply the language change. Restart now?",
                ))
                .set_buttons(rfd::MessageButtons::OkCancel)
                .show()
                == rfd::MessageDialogResult::Ok;
            if confirmed {
                do_restart(&c);
            } else {
                // User cancelled — revert settings and UI.
                let old_idx: i32 = if current == "en" { 1 } else { 0 };
                {
                    let mut s = c.settings.lock().expect("settings lock poisoned");
                    s.language = current;
                    s.save();
                }
                if let Some(app) = c.app.upgrade() {
                    app.set_language_index(old_idx);
                }
            }
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
        slint_app.on_toggle_ocr_enabled(move || {
            if let Some(app) = c.app.upgrade() {
                let new_val = !app.get_ocr_enabled();
                app.set_ocr_enabled(new_val);
                let mut s = c.settings.lock().expect("settings lock poisoned");
                s.ocr_enabled = new_val;
                s.save();
                let _ = c.looper.try_with_clipboard_service(|cs| {
                    cs.set_ocr_enabled(new_val);
                });
            }
        });

        let c = ctx.clone();
        slint_app.on_toggle_qr_enabled(move || {
            if let Some(app) = c.app.upgrade() {
                let new_val = !app.get_qr_enabled();
                app.set_qr_enabled(new_val);
                let mut s = c.settings.lock().expect("settings lock poisoned");
                s.qr_enabled = new_val;
                s.save();
                let _ = c.looper.try_with_clipboard_service(|cs| {
                    cs.set_qr_enabled(new_val);
                });
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
                // Require at least one backend before enabling auto-sync
                if new_val {
                    let s = c.settings.lock().expect("settings lock poisoned");
                    if s.sync_backends.is_empty() {
                        app.set_toast_visible(false);
                        app.set_toast_message(SharedString::from(i18n::tr(
                            "请先添加同步服务",
                            "Please add a sync service first",
                        )));
                        app.set_toast_visible(true);
                        return;
                    }
                    drop(s);
                }
                app.set_sync_auto_enabled(new_val);
                let mut s = c.settings.lock().expect("settings lock poisoned");
                s.sync_auto_enabled = new_val;
                s.save();
                drop(s);

                // When enabling auto-sync, trigger an immediate pull for all backends
                if new_val {
                    let _ = c.looper.try_with_sync_manager(|sm| {
                        sm.trigger_pull_all();
                    });
                }
            }
        });

        let c = ctx.clone();
        slint_app.on_toggle_sync_favorites_only(move || {
            if let Some(app) = c.app.upgrade() {
                let new_val = !app.get_sync_favorites_only();
                app.set_sync_favorites_only(new_val);
                let mut s = c.settings.lock().expect("settings lock poisoned");
                s.sync_favorites_only = new_val;
                s.save();
            }
        });

        let c = ctx.clone();
        slint_app.on_set_backend_sync_interval(move |id: SharedString, secs: i32| {
            // Update per-backend interval in settings
            {
                let mut s = c.settings.lock().expect("settings lock poisoned");
                for cfg in &mut s.sync_backends {
                    if cfg.id == id.as_str() {
                        cfg.sync_interval_secs = Some(secs as u64);
                        s.save();
                        break;
                    }
                }
            }
            // Reload backends to pick up new interval
            let _ = c.looper.try_with_sync_manager(|sm| {
                sm.reload_backends();
            });
            // UI model will be refreshed on next poll cycle
        });

        let c = ctx.clone();
        slint_app.on_sync_backend_now(move |id: SharedString| {
            let _ = c.looper.try_with_sync_manager(|sm| {
                sm.trigger_backend_sync(&id);
            });
        });

        let c = ctx.clone();
        slint_app.on_add_sync_backend(move |name: SharedString, path: SharedString| {
            let _ = c.looper.try_with_sync_manager(|sm| {
                sm.add_local_folder_backend(name.to_string(), path.to_string());
            });
        });

        let c = ctx.clone();
        slint_app.on_add_webdav_backend(
            move |name: SharedString,
                  url: SharedString,
                  username: SharedString,
                  password: SharedString| {
                let _ = c.looper.try_with_sync_manager(|sm| {
                    sm.add_webdav_backend(
                        name.to_string(),
                        url.to_string(),
                        username.to_string(),
                        password.to_string(),
                    );
                });
            },
        );

        let c = ctx.clone();
        slint_app.on_save_sync_backend(
            move |id: SharedString, name: SharedString, path: SharedString| {
                let _ = c.looper.try_with_sync_manager(|sm| {
                    sm.edit_backend(&id, &name, &path);
                });
            },
        );

        let c = ctx.clone();
        slint_app.on_save_webdav_backend(
            move |id: SharedString,
                  name: SharedString,
                  url: SharedString,
                  username: SharedString,
                  password: SharedString| {
                let _ = c.looper.try_with_sync_manager(|sm| {
                    sm.edit_webdav_backend(
                        &id,
                        &name,
                        &url,
                        &username,
                        &password,
                    );
                });
            },
        );

        let app_weak = slint_app.as_weak();
        slint_app.on_test_webdav_connection(
            move |url: SharedString, username: SharedString, password: SharedString| {
                let app_weak = app_weak.clone();
                std::thread::spawn(move || {
                    let ok = test_webdav_conn(&url, &username, &password);
                    let _ = app_weak.upgrade_in_event_loop(move |app| {
                        if ok {
                            app.set_add_backend_test_ok(true);
                            app.set_add_backend_test_error("".into());
                        } else {
                            app.set_add_backend_test_ok(false);
                            app.set_add_backend_test_error(
                                i18n::tr("连接失败，请检查地址与认证信息。", "Connection failed. Check URL and credentials.").into()
                            );
                        }
                    });
                });
            },
        );

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
                if let Some(info) = sm.get_backend_info(&id) {
                    if let Some(app) = c.app.upgrade() {
                        app.set_add_backend_edit_id(id.clone());
                        app.set_add_backend_edit_name(SharedString::from(&info.name));
                        app.set_add_backend_edit_folder(SharedString::from(&info.folder));
                        app.set_add_backend_backend_type(SharedString::from(&info.backend_type));
                        app.set_add_backend_edit_webdav_url(SharedString::from(&info.webdav_url));
                        app.set_add_backend_edit_webdav_username(SharedString::from(&info.webdav_username));
                        app.set_add_backend_edit_webdav_password(SharedString::from(&info.webdav_password));
                        app.set_add_backend_edit_mode(true);
                        app.set_add_backend_panel_visible(true);
                    }
                }
            });
        });

        let c = ctx.clone();
        slint_app.on_show_add_backend_panel(move || {
            if let Some(app) = c.app.upgrade() {
                app.set_add_backend_edit_mode(false);
                app.set_add_backend_edit_name(SharedString::default());
                app.set_add_backend_edit_folder(SharedString::default());
                app.set_add_backend_backend_type(SharedString::default());
                app.set_add_backend_edit_webdav_url(SharedString::default());
                app.set_add_backend_edit_webdav_username(SharedString::default());
                app.set_add_backend_edit_webdav_password(SharedString::default());
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
                        let edit_type = if item.meta_type == "email" || item.meta_type == "phone" {
                            item.meta_type.as_str()
                        } else {
                            item.content_type.as_str()
                        };
                        app.set_editing_item_type(SharedString::from(edit_type));
                        app.set_editing_content(SharedString::from(item.full_text.clone()));
                        app.set_current_view(SharedString::from("edit"));
                    }
                }
            }
        });

        let c = ctx.clone();
        slint_app.on_save_content(move |id, text: SharedString, sel_type: SharedString| {
            let (content_type, meta_type) = match sel_type.as_str() {
                "email" => ("plain_text", "email"),
                "phone" => ("plain_text", "phone"),
                other => (other, ""),
            };
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.update_content(id, &text, content_type, meta_type);
            });
            if let Some(app) = c.app.upgrade() {
                app.set_current_view(SharedString::from("clipboard"));
            }
        });

        let c = ctx.clone();
        slint_app.on_url_decode(move |text: SharedString| {
            let decoded = percent_decode_str(&text).decode_utf8().unwrap_or(std::borrow::Cow::Borrowed(&text));
            if let Some(app) = c.app.upgrade() {
                app.set_editing_content(SharedString::from(decoded.as_ref()));
            }
        });

        let c = ctx.clone();
        slint_app.on_json_format(move |text: SharedString| {
            let formatted = match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) => serde_json::to_string_pretty(&v).unwrap_or(text.to_string()),
                Err(_) => text.to_string(),
            };
            if let Some(app) = c.app.upgrade() {
                app.set_editing_content(SharedString::from(formatted));
            }
        });

        let c = ctx.clone();
        slint_app.on_trim_text(move |text: SharedString| {
            let trimmed = trim_text(&text);
            if let Some(app) = c.app.upgrade() {
                app.set_editing_content(SharedString::from(trimmed));
            }
        });

        let c = ctx.clone();
        slint_app.on_base64_decode(move |text: SharedString| {
            let decoded = decode_base64(&text);
            if let Some(app) = c.app.upgrade() {
                app.set_editing_content(SharedString::from(decoded));
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
                    app.set_filter_file(
                        cs.is_filter_active("file") || cs.is_filter_active("image"),
                    );
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
                let (is_color, is_hex, is_image, is_file, is_favorite) = if let Ok(db) = c.db.lock()
                {
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

                #[cfg(target_os = "windows")]
                {
                    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
                    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetCursorPos};
                    let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
                    unsafe { GetCursorPos(&mut pt); }
                    let scale = app.window().scale_factor();
                    let title: Vec<u16> = "Clippi\0".encode_utf16().collect();
                    let hwnd = unsafe { FindWindowW(std::ptr::null_mut(), title.as_ptr()) };
                    if !hwnd.is_null() {
                        let mut client_origin = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
                        unsafe { ClientToScreen(hwnd, &mut client_origin); }
                        // ContextMenu renders with an offset equal to the main
                        // content panel's x position (36px in app.slint).
                        const PANEL_OFFSET: f32 = 36.0;
                        app.set_context_menu_x((pt.x - client_origin.x) as f32 / scale - PANEL_OFFSET);
                        app.set_context_menu_y((pt.y - client_origin.y) as f32 / scale);
                    }
                }
                #[cfg(target_os = "macos")]
                {
                    let (cursor_x, cursor_y) = get_cursor_pos().unwrap_or((0, 0));
                    let pos = app.window().position();
                    let scale = app.window().scale_factor();
                    let (cursor_x, cursor_y) = {
                        (
                            (cursor_x as f32 * scale) as i32,
                            (cursor_y as f32 * scale) as i32,
                        )
                    };
                    // ContextMenu renders with an offset equal to the main
                    // content panel's x position (36px in app.slint).
                    const PANEL_OFFSET: f32 = 36.0;
                    app.set_context_menu_x((cursor_x as f32 - pos.x as f32) / scale - PANEL_OFFSET);
                    app.set_context_menu_y((cursor_y as f32 - pos.y as f32) / scale);
                }

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

        let c = ctx.clone();
        slint_app.on_dismiss_ui(move || {
            if let Ok(fe) = c.frontend.lock() {
                fe.dismiss_ui_to_clipboard();
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
            let items = c
                .looper
                .try_with_clipboard_service(|cs| cs.get_selected_items())
                .unwrap_or_default();

            if items.is_empty() {
                return;
            }

            let plain_flag = c.copy_as_plain_text.load(Ordering::Relaxed);
            let owned_items: Vec<_> = items.into_iter().collect();
            let bp = c.batch_pasting.clone();
            let clear_sel = c.clear_selection.clone();
            let shared = c.shared.clone();

            std::thread::spawn(move || {
                bp.store(true, Ordering::SeqCst);
                batch_paste_sequential(&owned_items, plain_flag, &shared);
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
                let pinned = c
                    .settings
                    .lock()
                    .map(|s| s.pinned_tag_ids.clone())
                    .unwrap_or_default();
                cs.refresh_sidebar_tags(&pinned);
            });
        });

        let c = ctx.clone();
        slint_app.on_toggle_tag_mode(move || {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.toggle_tag_mode_and_refresh();
                if let Some(app) = c.app.upgrade() {
                    app.set_tag_match_all(cs.tag_match_all());
                }
            });
        });

        let c = ctx.clone();
        slint_app.on_clear_tag_filters(move || {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.clear_tag_filters_and_refresh();
                if let Some(app) = c.app.upgrade() {
                    app.set_has_tag_filter(false);
                    app.set_tag_match_all(false);
                }
                let pinned = c
                    .settings
                    .lock()
                    .map(|s| s.pinned_tag_ids.clone())
                    .unwrap_or_default();
                cs.refresh_sidebar_tags(&pinned);
            });
        });

        let c = ctx.clone();
        slint_app.on_create_tag(move |name: SharedString| {
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.create_tag(name.as_str());
                cs.load_all_tags_for_filter();
                let pinned = c
                    .settings
                    .lock()
                    .map(|s| s.pinned_tag_ids.clone())
                    .unwrap_or_default();
                cs.refresh_sidebar_tags(&pinned);
            });
        });

        let c = ctx.clone();
        slint_app.on_update_tag(
            move |tag_id: i32, name: SharedString, color: slint::Color| {
                let hex = format!(
                    "#{:02X}{:02X}{:02X}",
                    color.red(),
                    color.green(),
                    color.blue()
                );
                let _ = c.looper.try_with_clipboard_service(|cs| {
                    cs.update_tag(tag_id as i64, name.as_str(), &hex);
                    cs.load_all_tags_for_filter();
                    cs.refresh_with_current_filter();
                    if let Some(app) = c.app.upgrade() {
                        if app.get_tag_picker_visible() {
                            cs.load_all_tags_for_picker(app.get_tag_picker_item_id());
                        }
                    }
                });
            },
        );

        let c = ctx.clone();
        slint_app.on_delete_tag(move |tag_id: i32| {
            let tag_id = tag_id as i64;
            // Clean up pinned tag on delete
            if let Ok(mut settings) = c.settings.lock() {
                if let Some(pos) = settings.pinned_tag_ids.iter().position(|&id| id == tag_id) {
                    settings.pinned_tag_ids.remove(pos);
                    settings.save();
                }
            }
            let _ = c.looper.try_with_clipboard_service(|cs| {
                cs.delete_tag(tag_id);
                cs.load_all_tags_for_filter();
                cs.refresh_with_current_filter();
                if let Some(app) = c.app.upgrade() {
                    app.set_has_tag_filter(cs.has_tag_filters());
                    if app.get_tag_picker_visible() {
                        cs.load_all_tags_for_picker(app.get_tag_picker_item_id());
                    }
                }
                let pinned = c
                    .settings
                    .lock()
                    .map(|s| s.pinned_tag_ids.clone())
                    .unwrap_or_default();
                cs.refresh_sidebar_tags(&pinned);
            });
        });

        let c = ctx.clone();
        slint_app.on_show_tag_picker(move |item_id: i32| {
            let is_batch = c
                .app
                .upgrade()
                .is_some_and(|app| app.get_context_menu_is_batch());
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

        let c = ctx.clone();
        slint_app.on_toggle_pin_tag(move |tag_id: i32| {
            let tag_id = tag_id as i64;
            if let Ok(mut settings) = c.settings.lock() {
                if let Some(pos) = settings.pinned_tag_ids.iter().position(|&id| id == tag_id) {
                    settings.pinned_tag_ids.remove(pos);
                } else {
                    settings.pinned_tag_ids.push(tag_id);
                }
                let pinned = settings.pinned_tag_ids.clone();
                drop(settings);
                let _ = c.looper.try_with_clipboard_service(|cs| {
                    cs.refresh_sidebar_tags(&pinned);
                });
                if let Ok(settings) = c.settings.lock() {
                    settings.save();
                }
            }
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
    app.set_db_path(SharedString::from(
        settings.resolve_db_path().to_string_lossy().to_string(),
    ));
    app.set_hotkey_display(SharedString::from(&settings.hotkey));
    app.set_position_mode(PositionMode::from_str(&settings.window_position_mode).to_int());
    app.set_card_height_mode(SharedString::from(&settings.card_height_mode));
    app.set_language_index(if settings.language == "en" { 1 } else { 0 });
    app.set_silent_start(settings.silent_start);
    app.set_show_source_app(settings.show_source_app);
    app.set_auto_scroll_to_top(settings.auto_scroll_to_top);
    app.set_copy_as_plain_text(settings.copy_as_plain_text);
    app.set_show_original_on_hover(settings.show_original_on_hover);
    app.set_ocr_enabled(settings.ocr_enabled);
    app.set_qr_enabled(settings.qr_enabled);
    app.set_max_items(settings.max_items as i32);
    app.set_sync_auto_enabled(settings.sync_auto_enabled);
    app.set_sync_favorites_only(settings.sync_favorites_only);
    // per-backend interval managed via SyncBackendInfo model
}

/// Write a clipboard item's content to the system clipboard.
/// When copy_as_plain_text is true, only plain text is written; otherwise
/// HTML and RTF formats are also restored from rich_data.
/// For images, the PNG file is loaded and written as an image format.
/// For files, file paths are written via ClipboardContent::Files (CF_HDROP).
fn write_item_to_clipboard(
    item: &crate::core::types::ClipboardItem,
    copy_as_plain_text: bool,
    _shared: &ClipboardShared,
) {
    crate::services::clipboard_ops::write_item_to_clipboard(item, copy_as_plain_text);
}

/// Sequential batch paste with clipboard verification and newline separators.
/// For each non-first item, a literal `\n` is pasted first to move the cursor to a
/// new line, then the actual item is written and pasted. This works for all content
/// types (text, rich text, images) because the newline is a separate paste operation.
fn batch_paste_sequential(
    items: &[crate::core::types::ClipboardItem],
    plain_flag: bool,
    shared: &ClipboardShared,
) {
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

        // Record expected image size before writing, for verification
        let expected_img_size = if item.content_type == ContentType::Image {
            std::fs::metadata(&item.image_path).map(|m| m.len()).ok()
        } else {
            None
        };

        let expected = item.full_text.clone();
        write_item_to_clipboard(item, plain_flag, shared);

        // Verify clipboard content before pasting.
        // Image items: verify PNG byte length (no text on clipboard).
        // File items: skip — no reliable text-based check.
        if item.content_type == ContentType::Image {
            if let Some(size) = expected_img_size {
                if !verify_clipboard_image(size, 300) {
                    log::warn!(
                        "batch_paste: image clipboard verification failed for item {}",
                        item.id
                    );
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        } else if item.content_type != ContentType::File {
            if !verify_clipboard_content(&expected, 300) {
                log::warn!(
                    "batch_paste: clipboard verification timed out for item {}",
                    item.id
                );
            }
        } else {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        restore_paste_target();
        paste_sync();

        // Wait for target app to fully retrieve clipboard data before we
        // write the next item (which calls EmptyClipboard and frees current data).
        // Images scale with file size — large PNGs can be 15-25 MB and the target
        // app needs time to copy that out of the clipboard.
        if i < n - 1 {
            let delay = if item.content_type == ContentType::Image {
                let file_size = std::fs::metadata(&item.image_path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                // ~1ms per 10 KB, floor 200ms, ceiling 3000ms
                let size_delay = (file_size / 10_000) as u64;
                size_delay.clamp(200, 3000)
            } else {
                100
            };
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
    }
}

/// Poll-read clipboard text until it matches expected or timeout expires.
fn verify_clipboard_content(expected: &str, timeout_ms: u64) -> bool {
    crate::services::clipboard_ops::verify_clipboard_content(expected, timeout_ms)
}

/// Common restart logic: save settings, release hotkey, spawn new process, quit.
/// The caller should have already checkpointed WAL and saved any pending settings.
fn do_restart(ctx: &CallbackCtx) {
    ctx.settings.lock().expect("settings lock").save();
    let _ = ctx
        .looper
        .try_with_hotkey_service(|hk| hk.unregister_hotkey());
    spawn_new_process();
    slint::quit_event_loop().ok();
}

/// Poll-read clipboard PNG buffer until its length matches expected_size or timeout expires.
fn verify_clipboard_image(expected_size: u64, timeout_ms: u64) -> bool {
    crate::services::clipboard_ops::verify_clipboard_image(expected_size, timeout_ms)
}

/// Test a WebDAV connection with a quick HEAD request.
fn test_webdav_conn(url: &str, username: &str, password: &str) -> bool {
    let raw = format!("{username}:{password}");
    let auth = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(&raw));
    let file_url = format!("{}/clippi_sync.json", url.trim_end_matches('/'));
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout_read(std::time::Duration::from_secs(5))
        .build();
    for test_url in [file_url.as_str(), url.trim_end_matches('/')] {
        match agent
            .head(test_url)
            .set("Authorization", &auth)
            .call()
        {
            Ok(resp) => {
                let status = resp.status();
                if (200..400).contains(&status) {
                    return true;
                }
                if status == 401 || status == 403 {
                    return false;
                }
            }
            Err(ureq::Error::Status(404, _)) => continue,
            Err(_) => return false,
        }
    }
    false
}

/// Trim text: normalize Unicode whitespace, remove blank lines, collapse
/// extra whitespace, keep lines close. Handles special chars from design
/// tools (non-breaking spaces, line/paragraph separators, etc.).
fn trim_text(text: &str) -> String {
    // Normalize line endings: CRLF → LF, Unicode separators → LF
    let text = text
        .replace("\r\n", "\n")
        .replace(['\r', '\u{2028}', '\u{2029}'], "\n"); // LINE/PARAGRAPH SEPARATORS

    let mut result = String::with_capacity(text.len());

    for line in text.lines() {
        let trimmed = line.trim(); // str::trim uses char::is_whitespace (Unicode-aware)
        if trimmed.is_empty() {
            continue;
        }
        let mut prev_ws = false;
        for ch in trimmed.chars() {
            if ch.is_whitespace() {
                if !prev_ws {
                    result.push(' ');
                    prev_ws = true;
                }
            } else {
                result.push(ch);
                prev_ws = false;
            }
        }
        result.push('\n');
    }

    if result.ends_with('\n') {
        result.pop();
    }

    result
}

/// Decode Base64 text, with optional prefix ("data:..." or bare).
fn decode_base64(text: &str) -> String {
    // Strip data URL prefix if present, e.g. "data:image/png;base64,"
    let encoded = if let Some(pos) = text.find(";base64,") {
        &text[pos + 8..]
    } else {
        text
    };
    // Try standard decoding first, then URL-safe
    use base64::Engine;
    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) {
        return String::from_utf8_lossy(&bytes).into_owned();
    }
    match base64::engine::general_purpose::URL_SAFE.decode(encoded) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => text.to_string(),
    }
}

