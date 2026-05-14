//! Frontend management - window visibility and UI operations

use crate::platform::monitor;
use crate::App;
use slint::{ComponentHandle, LogicalSize, PhysicalPosition};

/// Default window size (width, height) in logical pixels.
pub const DEFAULT_WINDOW_WIDTH: f32 = 320.0;
pub const DEFAULT_WINDOW_HEIGHT: f32 = 480.0;

#[derive(Clone, Copy, PartialEq)]
pub enum PositionMode {
    Center,
    FollowMouse,
    Remember,
}

impl PositionMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "follow" => PositionMode::FollowMouse,
            "remember" => PositionMode::Remember,
            _ => PositionMode::Center,
        }
    }

    pub fn to_int(self) -> i32 {
        match self {
            PositionMode::Center => 0,
            PositionMode::FollowMouse => 1,
            PositionMode::Remember => 2,
        }
    }

    pub fn from_int(v: i32) -> Self {
        match v {
            1 => PositionMode::FollowMouse,
            2 => PositionMode::Remember,
            _ => PositionMode::Center,
        }
    }
}

pub struct Frontend {
    app: slint::Weak<App>,
    visible: bool,
    suppress_until: Option<std::time::Instant>,
    position_mode: PositionMode,
    saved_window_x: i32,
    saved_window_y: i32,
    saved_window_width: f32,
    saved_window_height: f32,
}

impl Frontend {
    pub fn new(app: &App) -> Self {
        Self {
            app: app.as_weak(),
            visible: true,
            suppress_until: None,
            position_mode: PositionMode::Center,
            saved_window_x: -1,
            saved_window_y: -1,
            saved_window_width: 0.0,
            saved_window_height: 0.0,
        }
    }

    pub fn set_position_mode(&mut self, mode: PositionMode) {
        self.position_mode = mode;
    }

    pub fn set_saved_position(&mut self, x: i32, y: i32) {
        self.saved_window_x = x;
        self.saved_window_y = y;
    }

    pub fn set_saved_size(&mut self, w: f32, h: f32) {
        self.saved_window_width = w;
        self.saved_window_height = h;
    }

    pub fn saved_position(&self) -> (i32, i32) {
        (self.saved_window_x, self.saved_window_y)
    }

    pub fn apply_saved_position_to_settings(&self, settings: &mut crate::core::settings::AppSettings) {
        if self.saved_window_x >= 0 && self.saved_window_y >= 0 {
            settings.saved_window_x = self.saved_window_x;
            settings.saved_window_y = self.saved_window_y;
        }
        if self.saved_window_width > 0.0 && self.saved_window_height > 0.0 {
            settings.saved_window_width = self.saved_window_width;
            settings.saved_window_height = self.saved_window_height;
        }
    }

    pub fn apply_position(&self) {
        if let Some(app) = self.app.upgrade() {
            let window = app.window();
            let size = window.size();
            let win_w = size.width as i32;
            let win_h = size.height as i32;

            if let Some(pos) = self.calculate_position(win_w, win_h) {
                window.set_position(pos);
            }
        }
    }

    fn calculate_position(&self, win_w: i32, win_h: i32) -> Option<PhysicalPosition> {
        match self.position_mode {
            PositionMode::Center => self.calc_center(win_w, win_h),
            PositionMode::FollowMouse => self.calc_follow_mouse(win_w, win_h),
            PositionMode::Remember => self.calc_remember(win_w, win_h),
        }
    }

    fn calc_center(&self, win_w: i32, win_h: i32) -> Option<PhysicalPosition> {
        let (cx, cy) = monitor::get_cursor_pos()?;
        let area = monitor::get_monitor_work_area(cx, cy)?;
        let x = area.x + (area.width - win_w) / 2;
        let y = area.y + (area.height - win_h) / 2;
        Some(PhysicalPosition::new(x, y))
    }

    fn calc_follow_mouse(&self, win_w: i32, win_h: i32) -> Option<PhysicalPosition> {
        let (cx, cy) = monitor::get_cursor_pos()?;
        let area = monitor::get_monitor_work_area(cx, cy)?;
        let (x, y) = clamp_to_work_area(cx, cy, win_w, win_h, &area);
        Some(PhysicalPosition::new(x, y))
    }

    fn calc_remember(&self, win_w: i32, win_h: i32) -> Option<PhysicalPosition> {
        let (sx, sy) = self.saved_position();
        if sx < 0 || sy < 0 {
            return self.calc_center(win_w, win_h);
        }
        if !monitor::is_point_on_monitor(sx, sy) {
            return self.calc_center(win_w, win_h);
        }
        if let Some(area) = monitor::get_monitor_work_area(sx, sy) {
            let (x, y) = clamp_to_work_area(sx, sy, win_w, win_h, &area);
            Some(PhysicalPosition::new(x, y))
        } else {
            self.calc_center(win_w, win_h)
        }
    }

    #[cfg(target_os = "windows")]
    pub fn show_and_focus(&mut self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow};

        self.suppress_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
        self.visible = true;
        if let Some(app) = self.app.upgrade() {
            app.set_pinned(false);
            // Reset double-click state so stale state doesn't eat the first click
            app.set_click_pending(false);
            app.set_last_clicked_id(-1);
            let window = app.window();
            window.show().ok();
            let w = if self.saved_window_width > 0.0 { self.saved_window_width } else { DEFAULT_WINDOW_WIDTH };
            let h = if self.saved_window_height > 0.0 { self.saved_window_height } else { DEFAULT_WINDOW_HEIGHT };
            window.set_size(LogicalSize::new(w, h));
            self.apply_position();
            let t = app.get_scroll_trigger();
            app.set_scroll_trigger(t + 1);
        }

        let title: Vec<u16> = "Clippi\0".encode_utf16().collect();
        let hwnd = unsafe { FindWindowW(std::ptr::null_mut(), title.as_ptr()) };
        if !hwnd.is_null() {
            unsafe { SetForegroundWindow(hwnd) };
        }
    }

    #[cfg(target_os = "macos")]
    #[allow(deprecated)] // TODO: migrate to NSApp.activate once objc2 binding is available
    pub fn show_and_focus(&mut self) {
        self.suppress_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
        self.visible = true;
        if let Some(app) = self.app.upgrade() {
            app.set_pinned(false);
            app.set_click_pending(false);
            app.set_last_clicked_id(-1);
            let window = app.window();
            window.show().ok();
            let w = if self.saved_window_width > 0.0 { self.saved_window_width } else { DEFAULT_WINDOW_WIDTH };
            let h = if self.saved_window_height > 0.0 { self.saved_window_height } else { DEFAULT_WINDOW_HEIGHT };
            window.set_size(LogicalSize::new(w, h));
            self.apply_position();
            let t = app.get_scroll_trigger();
            app.set_scroll_trigger(t + 1);
            let mtm = unsafe { objc2::MainThreadMarker::new_unchecked() };
            let ns_app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            ns_app.activateIgnoringOtherApps(true);
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    pub fn show_and_focus(&mut self) {
        self.suppress_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
        self.visible = true;
        if let Some(app) = self.app.upgrade() {
            app.set_pinned(false);
            app.set_click_pending(false);
            app.set_last_clicked_id(-1);
            let t = app.get_scroll_trigger();
            app.set_scroll_trigger(t + 1);
        }
    }

    pub fn hide(&mut self) {
        if let Some(app) = self.app.upgrade() {
            let window = app.window();
            if self.position_mode == PositionMode::Remember {
                let pos = window.position();
                self.saved_window_x = pos.x;
                self.saved_window_y = pos.y;
            }
            // Save current window size so user-resized dimensions persist
            let size = window.size();
            let scale = window.scale_factor();
            self.saved_window_width = size.width as f32 / scale;
            self.saved_window_height = size.height as f32 / scale;
        }
        self.visible = false;
        if let Some(app) = self.app.upgrade() {
            app.window().hide().ok();
        }
    }

    pub fn show_settings(&mut self) {
        self.suppress_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
        if let Some(app) = self.app.upgrade() {
            app.set_current_view(slint::SharedString::from("settings"));
            let window = app.window();
            window.show().ok();
            let w = if self.saved_window_width > 0.0 { self.saved_window_width } else { DEFAULT_WINDOW_WIDTH };
            let h = if self.saved_window_height > 0.0 { self.saved_window_height } else { DEFAULT_WINDOW_HEIGHT };
            window.set_size(LogicalSize::new(w, h));
            self.apply_position();
            self.visible = true;
            let t = app.get_scroll_trigger();
            app.set_scroll_trigger(t + 1);
        }
    }

    pub fn is_suppressed(&self) -> bool {
        if let Some(until) = self.suppress_until {
            if std::time::Instant::now() < until {
                return true;
            }
        }
        false
    }

    /// Set suppress for initial window show to prevent immediate auto-hide
    pub fn set_initial_suppress(&mut self) {
        self.suppress_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
    }

    pub fn move_window(&self, dx: f32, dy: f32) {
        if let Some(app) = self.app.upgrade() {
            let window = app.window();
            let pos = window.position();
            let scale = window.scale_factor();
            window.set_position(PhysicalPosition::new(
                pos.x + (dx * scale) as i32,
                pos.y + (dy * scale) as i32,
            ));
        }
    }
}

fn clamp_to_work_area(x: i32, y: i32, w: i32, h: i32, area: &monitor::MonitorRect) -> (i32, i32) {
    let max_x = (area.x + area.width - w).max(area.x);
    let max_y = (area.y + area.height - h).max(area.y);
    let x = x.max(area.x).min(max_x);
    let y = y.max(area.y).min(max_y);
    (x, y)
}
