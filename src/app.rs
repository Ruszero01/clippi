use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, PhysicalPosition, SharedString, VecModel};

use crate::clipboard::{self, ClipboardEvent, ClipboardWatcherHandle};
use crate::db::Database;
use crate::history::ClipboardHistory;
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
}

impl AppController {
    pub fn new(slint_app: &App, tray: Rc<TrayManager>) -> Self {
        let slint_model: Rc<VecModel<ClipboardEntry>> = Rc::new(VecModel::default());
        slint_app.set_clipboard_items(ModelRc::from(slint_model.clone()));

        let db = Rc::new(RefCell::new(Database::open().expect("Failed to open database")));

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

        let (tx, rx) = mpsc::channel();
        let watcher = clipboard::start_watcher(tx).expect("Failed to start clipboard watcher");

        let timer_model = slint_model.clone();
        let timer_history = history.clone();
        let timer_hashes = seen_hashes.clone();
        let timer_db = db.clone();
        let weak = slint_app.as_weak();

        let timer = slint::Timer::default();
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

            if let Some(app) = weak.upgrade() {
                app.set_item_count(timer_model.row_count() as i32);
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

        // close-window → 隐藏窗口（不退出）
        let close_weak = slint_app.as_weak();
        slint_app.on_close_window(move || {
            if let Some(app) = close_weak.upgrade() {
                app.window().hide().ok();
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
        }
    }

    pub fn shutdown(mut self) {
        self.watcher.stop();
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
