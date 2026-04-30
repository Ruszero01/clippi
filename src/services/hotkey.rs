//! Hotkey service - handles hotkey business logic

use crate::core::frontend::Frontend;
use crate::looper::Pollable;
use crate::platform::hotkey::HotkeyListener;
use crate::App;
use slint::SharedString;
use std::sync::{Arc, Mutex};

pub struct HotkeyService {
    hotkey: Option<Box<dyn HotkeyListener>>,
    frontend: Arc<Mutex<Frontend>>,
    app: slint::Weak<App>,
    is_recording: bool,
}

impl HotkeyService {
    pub fn new(frontend: Arc<Mutex<Frontend>>, app: slint::Weak<App>) -> Self {
        Self {
            hotkey: None,
            frontend,
            app,
            is_recording: false,
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
}

impl Pollable for HotkeyService {
    fn poll(&mut self) {
        let Some(app) = self.app.upgrade() else { return };

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
