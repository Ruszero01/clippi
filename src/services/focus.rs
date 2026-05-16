//! Focus service - handles focus events, auto-hide logic, and foreground info

use crate::core::frontend::Frontend;
use crate::looper::Pollable;
use crate::platform::blacklist::is_clippi_foreground;
use crate::platform::focus::{get_foreground_app_info, start_focus_watcher, FocusWatcher};
use crate::App;
use slint::{ComponentHandle, Image, SharedString};
use std::sync::{Arc, Mutex};

/// Shared current foreground app name for cross-service coordination.
pub type ForegroundAppName = Arc<Mutex<String>>;

pub fn shared_foreground_app_name() -> ForegroundAppName {
    Arc::new(Mutex::new(String::new()))
}

pub struct FocusService {
    watcher: Option<FocusWatcher>,
    frontend: Arc<Mutex<Frontend>>,
    app: slint::Weak<App>,
    /// Shared foreground app name (written by focus, read by hotkey service).
    foreground_app_name: ForegroundAppName,
    /// Last foreground app name for deduplication.
    last_fg_app_name: String,
    auto_hide: bool,
    pinned: bool,
}

impl FocusService {
    pub fn new(
        frontend: Arc<Mutex<Frontend>>,
        app: slint::Weak<App>,
        foreground_app_name: ForegroundAppName,
    ) -> Result<Self, String> {
        let watcher = start_focus_watcher()?;
        Ok(Self {
            watcher: Some(watcher),
            frontend,
            app,
            foreground_app_name,
            last_fg_app_name: String::new(),
            auto_hide: true,
            pinned: false,
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

        // ── Update foreground app info for the blacklist UI ──
        // When Clippi itself is foreground, only clear the shared state so the
        // hotkey stays registered. Keep the UI showing the last foreground app
        // so the user can still manage the blacklist while Clippi is focused.
        if is_clippi {
            if let Ok(mut fg) = self.foreground_app_name.lock() {
                fg.clear();
            }
        } else if let Some(info) = get_foreground_app_info() {
            app.set_foreground_app_name(SharedString::from(&info.app_name));
            app.set_foreground_window_title(SharedString::from(&info.window_title));
            // Update shared state for hotkey service
            if let Ok(mut fg) = self.foreground_app_name.lock() {
                *fg = info.app_name.clone();
            }
            // Save icon to disk and load as Image when app name changes
            if info.app_name != self.last_fg_app_name {
                self.last_fg_app_name = info.app_name.clone();
                if !info.icon_base64.is_empty() {
                    if let Ok(bytes) = base64::Engine::decode(
                        &base64::engine::general_purpose::STANDARD,
                        &info.icon_base64,
                    ) {
                        let icon_path = crate::core::paths::app_icon_path(&info.app_name);
                        if std::fs::write(&icon_path, &bytes).is_ok() {
                            if let Ok(img) = Image::load_from_path(&icon_path) {
                                app.set_foreground_app_icon(img);
                            }
                        }
                    }
                }
            }
            if info.app_name.is_empty() {
                self.last_fg_app_name.clear();
            }
        } else {
            self.last_fg_app_name.clear();
            app.set_foreground_app_name(SharedString::default());
            app.set_foreground_window_title(SharedString::default());
            if let Ok(mut fg) = self.foreground_app_name.lock() {
                fg.clear();
            }
        }

        // ── Auto-hide logic ──
        if !self.auto_hide || self.pinned {
            return;
        }

        // Window must be visible
        if !app.window().is_visible() {
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
