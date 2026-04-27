use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, PhysicalPosition, SharedString, VecModel};

use crate::blacklist::{is_blacklisted, is_clippi_foreground};
use crate::clipboard::{self, ClipboardEvent, ClipboardWatcherHandle};
use crate::db::Database;
use crate::focus;
use crate::history::ClipboardHistory;
use crate::hotkey::HotkeyManager;
use crate::settings::{self, AppSettings};
use crate::tray::{TrayAction, TrayManager};
use crate::types::ClipboardItem;
use crate::{App, ClipboardEntry};

pub struct AppController {
    #[allow(dead_code)]
    slint_model: Rc<VecModel<ClipboardEntry>>,
    watcher: ClipboardWatcherHandle,
    #[allow(dead_code)]
    timer: slint::Timer,
    #[allow(dead_code)]
    db: Rc<RefCell<Database>>,
    #[allow(dead_code)]
    tray: Rc<TrayManager>,
    #[allow(dead_code)]
    tray_timer: slint::Timer,
    #[allow(dead_code)]
    hotkey: Rc<RefCell<HotkeyManager>>,
    #[allow(dead_code)]
    focus_watcher: focus::FocusWatcher,
}

impl AppController {
    pub fn new(slint_app: &App, tray: Rc<TrayManager>) -> Self {
        let slint_model: Rc<VecModel<ClipboardEntry>> = Rc::new(VecModel::default());
        slint_app.set_clipboard_items(ModelRc::from(slint_model.clone()));

        // 加载配置，确定数据库路径
        let app_settings = Rc::new(RefCell::new(AppSettings::load()));
        let db_path = app_settings.borrow().resolve_db_path();
        let db = Rc::new(RefCell::new(
            Database::open(&db_path.to_string_lossy()).expect("Failed to open database")
        ));

        let history = Rc::new(RefCell::new(ClipboardHistory::new(100)));
        let seen_hashes = Rc::new(RefCell::new(HashSet::new()));
        let loaded_items = db.borrow().load_recent(100).expect("Failed to load from database");

        for item in &loaded_items {
            seen_hashes.borrow_mut().insert(item.content_hash);
            let entry = item_to_slint(item);
            slint_model.push(entry);
        }
        for item in loaded_items {
            history.borrow_mut().add(item);
        }
        slint_app.set_item_count(slint_model.row_count() as i32);

        // 恢复主题设置
        let theme = app_settings.borrow().theme.clone();
        let is_dark = match theme.as_str() {
            "dark" => true,
            "light" => false,
            _ => settings::is_system_dark_mode(),
        };
        slint_app.set_dark_mode(is_dark);
        slint_app.set_theme_mode(match theme.as_str() {
            "dark" => 1,
            "light" => 2,
            _ => 0,
        });

        // 恢复设置初始值
        slint_app.set_auto_start(settings::is_auto_start_enabled());
        slint_app.set_auto_hide(app_settings.borrow().auto_hide);
        slint_app.set_db_path(SharedString::from(db.borrow().path()));
        slint_app.set_settings_error(SharedString::from(""));

        let (tx, rx) = mpsc::channel();
        let watcher = clipboard::start_watcher(tx).expect("Failed to start clipboard watcher");

        // 启动焦点事件监听
        let (focus_watcher, focus_rx) = focus::start_focus_watcher().expect("Failed to start focus watcher");

        // 从配置加载快捷键设置
        let hotkey_str = app_settings.borrow().hotkey.clone();

        let hotkey = Rc::new(RefCell::new(
            HotkeyManager::new(&hotkey_str).unwrap_or_else(|e| {
                eprintln!("Warning: {e}");
                HotkeyManager::new(crate::hotkey::DEFAULT_HOTKEY)
                    .expect("Failed to initialize fallback hotkey")
            })
        ));

        slint_app.set_hotkey_display(SharedString::from(hotkey.borrow().current_display()));

        // 从配置加载黑名单（逗号分隔的进程名）
        let blacklist: HashSet<String> = {
            let bl = &app_settings.borrow().blacklist;
            bl.split(',').map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty()).collect()
        };

        let timer_model = slint_model.clone();
        let timer_history = history.clone();
        let timer_hashes = seen_hashes.clone();
        let timer_db = db.clone();
        let timer_hotkey = hotkey.clone();
        let timer_blacklist = Rc::new(RefCell::new(blacklist));
        let weak = slint_app.as_weak();

        let timer = slint::Timer::default();
        let timer_hotkey_settings = app_settings.clone();
        let timer_auto_hide = app_settings.clone();
let timer_suppress_hide = Arc::new(AtomicBool::new(false));
        let suppress_for_timer = timer_suppress_hide.clone();
        let startup_guard = Arc::new(AtomicBool::new(true));
        let startup_timer = startup_guard.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            startup_timer.store(false, Ordering::SeqCst);
        });
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(100), move || {
            while let Ok(event) = rx.try_recv() {
                match event {
                    ClipboardEvent::NewContent(item) => {
                        let hash = item.content_hash;
                        if timer_hashes.borrow().contains(&hash) {
                            let _ = timer_db.borrow().upsert(&item);
                            let idx = timer_model.iter().position(|e| {
                                let h = timer_history.borrow();
                                h.items().iter()
                                    .any(|i| i.content_hash == hash && i.id == e.id as i64)
                            });
                            if let Some(i) = idx {
                                let entry = item_to_slint(&item);
                                timer_model.remove(i);
                                timer_model.insert(0, entry);
                            }
                            timer_history.borrow_mut().add(item);
                            continue;
                        }

                        timer_hashes.borrow_mut().insert(hash);
                        let _ = timer_db.borrow().upsert(&item);
                        let entry = item_to_slint(&item);
                        timer_history.borrow_mut().add(item);

                        timer_model.insert(0, entry);
                    }
                }
            }

            // 录制模式：直接轮询 GetAsyncKeyState 检测按下的快捷键
            {
                let captured = {
                    timer_hotkey.borrow().poll_recording_pressed()
                };
                if let Some(s) = captured {
                    if !s.is_empty() {
                        // 先更新
                        let update_ok = timer_hotkey.borrow_mut().update_hotkey(&s).is_ok();
                        if update_ok {
                            let display = timer_hotkey.borrow().current_display();
                            timer_hotkey_settings.borrow_mut().hotkey = s.to_string();
                            timer_hotkey_settings.borrow().save();
                            if let Some(app) = weak.upgrade() {
                                app.set_hotkey_display(SharedString::from(display));
                                app.set_settings_error(SharedString::from(""));
                                app.set_recording_hotkey(false);
                            }
                        }
                    }
                    timer_hotkey.borrow_mut().finish_recording();
                }
            }

            // 热键事件：按下时显示窗口
            if timer_hotkey.borrow().poll_pressed() {
                if let Some(app) = weak.upgrade() {
                    app.window().show().ok();
                }
            }

            // 消费焦点事件：黑名单应用获得焦点时注销，离开时重新注册
            if let Some(event) = focus::poll_focus_events(&focus_rx) {
                match event {
                    focus::FocusEvent::ForegroundChanged(Some(name)) => {
                        let bl = timer_blacklist.borrow();
                        if is_blacklisted(&name, &bl) {
                            let _ = timer_hotkey.borrow_mut().unregister();
                        } else {
                            let _ = timer_hotkey.borrow_mut().register();
                        }
                    }
                    focus::FocusEvent::ForegroundChanged(None) => {
                        let _ = timer_hotkey.borrow_mut().register();
                    }
                }
            }

            if let Some(app) = weak.upgrade() {
                app.set_item_count(timer_model.row_count() as i32);

                // 失焦自动隐藏（仅剪贴板列表视图、窗口可见、功能开启时、且不在启动宽限期内）
                if timer_auto_hide.borrow().auto_hide {
                    if app.window().is_visible() {
                        if app.get_current_view() == "clipboard" {
                            if !suppress_for_timer.load(Ordering::SeqCst) {
                                if !startup_guard.load(Ordering::SeqCst) {
                                    if !is_clippi_foreground() {
                                        app.window().hide().ok();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        // copy-item: 复制到剪贴板
        let copy_history = history.clone();
        slint_app.on_copy_item(move |id| {
            if let Some(text) = copy_history.borrow().items().iter()
                .find(|i| i.id == id as i64)
                .map(|i| i.full_text.clone())
            {
                if let Ok(ctx) = clipboard_rs::ClipboardContext::new() {
                    let _ = clipboard_rs::Clipboard::set_text(&ctx, text);
                }
            }
        });

        // paste-item: 快速粘贴（复制+隐藏窗口+模拟Ctrl+V）
        let paste_history = history.clone();
        let paste_weak = slint_app.as_weak();
        slint_app.on_paste_item(move |id| {
            if let Some(text) = paste_history.borrow().items().iter()
                .find(|i| i.id == id as i64)
                .map(|i| i.full_text.clone())
            {
                if let Ok(ctx) = clipboard_rs::ClipboardContext::new() {
                    let _ = clipboard_rs::Clipboard::set_text(&ctx, text);
                }
                // 隐藏窗口
                if let Some(app) = paste_weak.upgrade() {
                    app.window().hide().ok();
                }
                // 短暂延迟后模拟 Ctrl+V（等窗口隐藏完成）
                std::thread::sleep(std::time::Duration::from_millis(50));
                crate::paste::simulate_ctrl_v();
            }
        });

        // clear-all
        let clear_model = slint_model.clone();
        let clear_history = history.clone();
        let clear_hashes = seen_hashes.clone();
        let clear_db = db.clone();
        slint_app.on_clear_all(move || {
            clear_model.clear();
            clear_history.borrow_mut().clear();
            clear_hashes.borrow_mut().clear();
            let _ = clear_db.borrow().clear();
        });

        // move-window
        let move_weak = slint_app.as_weak();
        slint_app.on_move_window(move |dx, dy| {
            if let Some(app) = move_weak.upgrade() {
                let window = app.window();
                let pos = window.position();
                let scale = window.scale_factor();
                let new_x = pos.x + (dx * scale) as i32;
                let new_y = pos.y + (dy * scale) as i32;
                window.set_position(PhysicalPosition::new(new_x, new_y));
            }
        });

        // resize-window
        let resize_weak = slint_app.as_weak();
        slint_app.on_resize_window(move |dw, dh| {
            if let Some(app) = resize_weak.upgrade() {
                let window = app.window();
                let scale = window.scale_factor();
                let size = window.size();
                let min_w = 320i32;
                let min_h = 480i32;
                let new_w = (size.width as i32 + (dw * scale) as i32).max(min_w);
                let new_h = (size.height as i32 + (dh * scale) as i32).max(min_h);
                window.set_size(slint::PhysicalSize::new(new_w as u32, new_h as u32));
            }
        });

        // close-window → 隐藏窗口并重置到剪贴板视图
        let close_weak = slint_app.as_weak();
        slint_app.on_close_window(move || {
            if let Some(app) = close_weak.upgrade() {
                app.set_current_view(SharedString::from("clipboard"));
                app.window().hide().ok();
            }
        });

        // toggle-auto-start → 切换开机自启
        let autostart_weak = slint_app.as_weak();
        let autostart_settings = app_settings.clone();
        slint_app.on_toggle_auto_start(move || {
            if let Some(app) = autostart_weak.upgrade() {
                let current = app.get_auto_start();
                let new_val = !current;
                match settings::set_auto_start(new_val) {
                    Ok(()) => {
                        app.set_auto_start(new_val);
                        autostart_settings.borrow_mut().auto_start = new_val;
                        autostart_settings.borrow().save();
                    }
                    Err(e) => {
                        app.set_settings_error(SharedString::from(e));
                    }
                }
            }
});

        // toggle-auto-hide → 切换失焦自动隐藏
        let autohide_weak = slint_app.as_weak();
        let autohide_settings = app_settings.clone();
        slint_app.on_toggle_auto_hide(move || {
            if let Some(app) = autohide_weak.upgrade() {
                let current = app.get_auto_hide();
                let new_val = !current;
                app.set_auto_hide(new_val);
                autohide_settings.borrow_mut().auto_hide = new_val;
                autohide_settings.borrow().save();
            }
        });

// pick-db-path → 选择新数据库路径并迁移（对话框期间抑制自动隐藏）
        let pick_weak = slint_app.as_weak();
        let pick_db = db.clone();
        let pick_settings = app_settings.clone();
        let pick_suppress = timer_suppress_hide.clone();
        slint_app.on_pick_db_path(move || {
            pick_suppress.store(true, Ordering::SeqCst);
            let old_path = std::path::PathBuf::from(pick_db.borrow().path());

            if let Some(new_path) = rfd::FileDialog::new()
                .set_file_name("clippi.db")
                .save_file()
            {
                pick_suppress.store(false, Ordering::SeqCst);
                if let Some(app) = pick_weak.upgrade() {
                    match settings::migrate_database(&old_path, &new_path) {
                        Ok(()) => {
                            let path_str = new_path.to_string_lossy().to_string();
                            pick_settings.borrow_mut().db_path = path_str;
                            pick_settings.borrow().save();
                            settings::spawn_new_process();
                            slint::quit_event_loop().ok();
                        }
                        Err(e) => {
                            app.set_settings_error(SharedString::from(e));
                        }
                    }
                }
            }
        });

        // reset-db-path → 重置数据库路径为默认
        let reset_weak = slint_app.as_weak();
        let reset_db = db.clone();
        let reset_settings = app_settings.clone();
        slint_app.on_reset_db_path(move || {
            let old_path = std::path::PathBuf::from(reset_db.borrow().path());
            let default_path = std::path::PathBuf::from("clippi.db");
            if old_path == default_path {
                return;
            }
            match settings::migrate_database(&old_path, &default_path) {
                Ok(()) => {
                    reset_settings.borrow_mut().db_path = String::new();
                    reset_settings.borrow().save();
                    settings::spawn_new_process();
                    slint::quit_event_loop().ok();
                }
                Err(e) => {
                    if let Some(app) = reset_weak.upgrade() {
                        app.set_settings_error(SharedString::from(e));
                    }
                }
            }
        });

        // set-hotkey → 更新快捷键
        let hotkey_inner = hotkey.clone();
        let hotkey_weak = slint_app.as_weak();
        let hotkey_settings = app_settings.clone();
        slint_app.on_set_hotkey(move |hotkey_str: SharedString| {
            let s = hotkey_str.as_str();
            match hotkey_inner.borrow_mut().update_hotkey(s) {
                Ok(()) => {
                    let display = hotkey_inner.borrow().current_display();
                    hotkey_settings.borrow_mut().hotkey = s.to_string();
                    hotkey_settings.borrow().save();
                    if let Some(app) = hotkey_weak.upgrade() {
                        app.set_hotkey_display(SharedString::from(display));
                        app.set_settings_error(SharedString::from(""));
                    }
                }
                Err(e) => {
                    if let Some(app) = hotkey_weak.upgrade() {
                        app.set_settings_error(SharedString::from(e));
                    }
                }
            }
        });

        // start-recording-hotkey → 开始录制快捷键
        let start_rec_weak = slint_app.as_weak();
        let start_rec_hotkey = hotkey.clone();
        slint_app.on_start_recording_hotkey(move || {
            if let Some(app) = start_rec_weak.upgrade() {
                app.window().show().ok();
                app.set_recording_hotkey(true);
                let _ = start_rec_hotkey.borrow_mut().start_recording();
            }
        });

        // set-theme → 切换主题模式
        let theme_settings = app_settings.clone();
        let theme_weak = slint_app.as_weak();
        slint_app.on_set_theme(move |mode: i32| {
            if let Some(app) = theme_weak.upgrade() {
                app.set_theme_mode(mode);
                let theme_str = match mode {
                    1 => "dark",
                    2 => "light",
                    _ => "system",
                };
                let is_dark = match mode {
                    1 => true,
                    2 => false,
                    _ => settings::is_system_dark_mode(),
                };
                app.set_dark_mode(is_dark);
                theme_settings.borrow_mut().theme = theme_str.to_string();
                theme_settings.borrow().save();
            }
        });

        // 托盘事件轮询
        let tray_inner = tray.clone();
        let tray_weak = slint_app.as_weak();
        let tray_timer = slint::Timer::default();
        tray_timer.start(slint::TimerMode::Repeated, Duration::from_millis(100), move || {
            while let Some(action) = tray_inner.poll_events() {
                match action {
                    TrayAction::Show => {
                        if let Some(app) = tray_weak.upgrade() {
                            app.window().show().ok();
                        }
                    }
                    TrayAction::Quit => {
                        slint::quit_event_loop().ok();
                    }
                }
            }
        });

        Self {
            slint_model,
            watcher,
            timer,
            db,
            tray,
            tray_timer,
            hotkey,
            focus_watcher,
        }
    }

    pub fn shutdown(mut self) {
        let _ = self.hotkey.borrow_mut().unregister();
        self.watcher.stop();
        self.focus_watcher.stop();
    }
}

fn item_to_slint(item: &ClipboardItem) -> ClipboardEntry {
    ClipboardEntry {
        id: item.id as i32,
        preview: SharedString::from(item.text_preview.clone()),
        content_type: SharedString::from(item.content_type.as_str()),
        time_label: SharedString::from(format_relative_time(&item.captured_at)),
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

