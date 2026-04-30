//! Focus service - handles focus events and auto-hide logic

use crate::core::frontend::Frontend;
use crate::looper::Pollable;
use crate::platform::blacklist::is_clippi_foreground;
use crate::platform::focus::{start_focus_watcher, FocusWatcher};
use crate::App;
use slint::{ComponentHandle, SharedString};
use std::sync::{Arc, Mutex};

pub struct FocusService {
    watcher: Option<FocusWatcher>,
    frontend: Arc<Mutex<Frontend>>,
    app: slint::Weak<App>,
    auto_hide: bool,
    pinned: bool,
    was_clippi_foreground: bool,
}

impl FocusService {
    pub fn new(frontend: Arc<Mutex<Frontend>>, app: slint::Weak<App>) -> Result<Self, String> {
        let (watcher, _rx) = start_focus_watcher()?;
        Ok(Self {
            watcher: Some(watcher),
            frontend,
            app,
            auto_hide: true,
            pinned: false,
            was_clippi_foreground: false,
        })
    }

    pub fn set_auto_hide(&mut self, auto_hide: bool) {
        self.auto_hide = auto_hide;
    }

    pub fn set_pinned(&mut self, pinned: bool) {
        self.pinned = pinned;
    }
}

impl Pollable for FocusService {
    fn poll(&mut self) {
        let app = match self.app.upgrade() {
            Some(app) => app,
            None => return,
        };

        let is_clippi = is_clippi_foreground();

        // Detect when Clippi gains focus - record the previous window as paste target
        if is_clippi && !self.was_clippi_foreground {
            // Clippi just gained focus, record current foreground (which will be Clippi)
            // but we need the previous one... actually GetForegroundWindow already returns Clippi
            // So we need a different approach - record when we're about to lose focus
            // But that requires knowing the future...
        }
        self.was_clippi_foreground = is_clippi;

        // Check if we should auto-hide
        if !self.auto_hide || self.pinned {
            return;
        }

        // Window must be visible
        if !app.window().is_visible() {
            return;
        }

        // Must be in clipboard view (not settings)
        if app.get_current_view() != SharedString::from("clipboard") {
            return;
        }

        // Check suppress (200ms after show)
        let is_suppressed = {
            if let Ok(fe) = self.frontend.lock() {
                fe.is_suppressed()
            } else {
                false
            }
        };
        if is_suppressed {
            return;
        }

        // Check if Clippi is in foreground
        if is_clippi {
            return;
        }

        // All conditions met, hide the window
        if let Ok(mut fe) = self.frontend.lock() {
            fe.hide();
        }
    }

    fn stop(&mut self) {
        if let Some(mut w) = self.watcher.take() {
            w.stop();
        }
    }
}
