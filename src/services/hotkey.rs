//! Hotkey service - handles hotkey business logic

use crate::core::frontend::Frontend;
use crate::core::settings::AppSettings;
use crate::looper::Pollable;
use crate::platform::hotkey::HotkeyListener;
use crate::services::focus::ForegroundAppName;
use crate::App;
use crate::BlacklistEntry;
use slint::{Model, ModelRc, SharedString, VecModel};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// Load a cached app icon from disk, falling back to a blank image.
fn load_app_icon(app_name: &str) -> slint::Image {
    let icon_path = crate::core::paths::app_icon_path(app_name);
    slint::Image::load_from_path(&icon_path).unwrap_or_else(|_| slint::Image::default())
}

pub struct HotkeyService {
    hotkey: Option<Box<dyn HotkeyListener>>,
    frontend: Arc<Mutex<Frontend>>,
    app: slint::Weak<App>,
    is_recording: bool,
    /// Blacklist entries with app name + cached icon.
    blacklist_model: Rc<VecModel<BlacklistEntry>>,
    /// Shared foreground app name (written by FocusService, read here for blacklist).
    foreground_app_name: ForegroundAppName,
    settings: Arc<Mutex<AppSettings>>,
}

impl HotkeyService {
    pub fn new(
        frontend: Arc<Mutex<Frontend>>,
        app: slint::Weak<App>,
        foreground_app_name: ForegroundAppName,
        settings: Arc<Mutex<AppSettings>>,
    ) -> Self {
        Self {
            hotkey: None,
            frontend,
            app,
            is_recording: false,
            blacklist_model: Rc::new(VecModel::default()),
            foreground_app_name,
            settings,
        }
    }

    pub fn set_hotkey(&mut self, hotkey: Box<dyn HotkeyListener>) {
        self.hotkey = Some(hotkey);
    }

    pub fn start_recording(&mut self) {
        if let Some(ref mut h) = self.hotkey {
            h.start_recording();
            self.is_recording = true;
        }
    }

    pub fn update_hotkey(&mut self, hotkey_str: &str) -> Result<(), String> {
        if let Some(ref mut h) = self.hotkey {
            h.update_hotkey(hotkey_str)
        } else {
            Err("No hotkey listener".to_string())
        }
    }

    /// Unregister the global hotkey (for blacklisted apps).
    pub fn unregister_hotkey(&mut self) {
        if let Some(ref mut h) = self.hotkey {
            h.unregister();
        }
    }

    /// Re-register the global hotkey (when no longer blacklisted).
    pub fn register_hotkey(&mut self) {
        if let Some(ref mut h) = self.hotkey {
            h.register();
        }
    }

    // ── Blacklist management ──

    /// Get the blacklist model for Slint binding.
    pub fn blacklist_model(&self) -> ModelRc<BlacklistEntry> {
        ModelRc::from(self.blacklist_model.clone())
    }

    /// Initialize blacklist from settings (names only, icons loaded from cache).
    pub fn load_blacklist(&mut self, apps: &[String]) {
        let entries: Vec<BlacklistEntry> = apps
            .iter()
            .map(|name| BlacklistEntry {
                name: SharedString::from(name.as_str()),
                icon: load_app_icon(name),
            })
            .collect();
        self.blacklist_model.set_vec(entries);
    }

    /// Check if an app name is in the blacklist.
    pub fn is_blacklisted(&self, app_name: &str) -> bool {
        if app_name.is_empty() {
            return false;
        }
        self.blacklist_model
            .iter()
            .any(|e| e.name.as_str() == app_name)
    }

    /// Add an app to the blacklist (icon loaded from cache). Returns true if added.
    pub fn add_to_blacklist(&mut self, app_name: &str) -> bool {
        if app_name.is_empty() || self.is_blacklisted(app_name) {
            return false;
        }
        self.blacklist_model.push(BlacklistEntry {
            name: SharedString::from(app_name),
            icon: load_app_icon(app_name),
        });
        true
    }

    /// Remove an app from the blacklist.
    pub fn remove_from_blacklist(&mut self, app_name: &str) {
        let pos = self
            .blacklist_model
            .iter()
            .position(|e| e.name.as_str() == app_name);
        if let Some(idx) = pos {
            self.blacklist_model.remove(idx);
        }
    }

    /// Return the current blacklist as Vec<String> for persisting to settings.
    pub fn blacklist_apps(&self) -> Vec<String> {
        self.blacklist_model
            .iter()
            .map(|e| e.name.to_string())
            .collect()
    }
}

impl Pollable for HotkeyService {
    fn poll(&mut self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };

        // ── Dynamic registration based on blacklist ──
        let fg_name = self
            .foreground_app_name
            .lock()
            .ok()
            .map(|fg| fg.clone())
            .unwrap_or_default();
        if !fg_name.is_empty() && self.is_blacklisted(&fg_name) {
            self.unregister_hotkey();
        } else {
            self.register_hotkey();
        }

        // Poll hotkey press
        if let Some(ref h) = self.hotkey {
            if h.poll_pressed() {
                if let Ok(mut fe) = self.frontend.lock() {
                    fe.show_and_focus();
                }
            }
        }

        // Poll recording
        if self.is_recording {
            if let Some(ref mut h) = self.hotkey {
                if let Some(new_hotkey) = h.poll_recording_pressed() {
                    if !new_hotkey.is_empty() {
                        if let Err(e) = h.update_hotkey(&new_hotkey) {
                            app.set_settings_error(SharedString::from(e));
                        }
                        h.finish_recording();
                        self.is_recording = false;
                        app.set_hotkey_display(SharedString::from(&new_hotkey));
                        app.set_recording_hotkey(false);
                        if let Ok(mut settings) = self.settings.lock() {
                            settings.hotkey = new_hotkey;
                            settings.save();
                        }
                    }
                }
            }
        }
    }

    fn stop(&mut self) {
        if let Some(ref mut h) = self.hotkey {
            h.stop();
        }
    }
}
