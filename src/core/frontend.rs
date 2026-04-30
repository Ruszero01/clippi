//! Frontend management - window visibility and UI operations (pure, no platform code)

use crate::App;
use slint::{ComponentHandle, PhysicalPosition};

pub struct Frontend {
    app: slint::Weak<App>,
    visible: bool,
    suppress_until: Option<std::time::Instant>,
}

impl Frontend {
    pub fn new(app: &App) -> Self {
        Self {
            app: app.as_weak(),
            visible: true,
            suppress_until: None,
        }
    }

    fn show(&mut self) {
        self.visible = true;
        if let Some(app) = self.app.upgrade() {
            app.window().show().ok();
        }
    }

    /// Show window with 200ms suppress period (to prevent immediate auto-hide)
    #[cfg(target_os = "windows")]
    pub fn show_and_focus(&mut self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow};

        self.suppress_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
        if let Some(app) = self.app.upgrade() {
            app.window().show().ok();
        }

        // Bring to foreground
        let title: Vec<u16> = "Clippi\0".encode_utf16().collect();
        let hwnd = unsafe { FindWindowW(std::ptr::null_mut(), title.as_ptr()) };
        if !hwnd.is_null() {
            unsafe { SetForegroundWindow(hwnd) };
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub fn show_and_focus(&mut self) {
        self.suppress_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
        self.show();
    }

    pub fn hide(&mut self) {
        self.visible = false;
        if let Some(app) = self.app.upgrade() {
            app.window().hide().ok();
        }
    }

    pub fn show_settings(&mut self) {
        self.suppress_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
        if let Some(app) = self.app.upgrade() {
            app.set_current_view(slint::SharedString::from("settings"));
            app.window().show().ok();
            self.visible = true;
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Check if auto-hide is currently suppressed
    pub fn is_suppressed(&self) -> bool {
        if let Some(until) = self.suppress_until {
            if std::time::Instant::now() < until {
                return true;
            }
        }
        false
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
