//! Tray service - handles tray business logic

use crate::core::frontend::Frontend;
use crate::looper::Pollable;
use crate::platform::tray::{TrayAction, TrayManager};
use std::sync::{Arc, Mutex};

pub struct TrayService {
    tray: TrayManager,
    frontend: Arc<Mutex<Frontend>>,
}

impl TrayService {
    pub fn new(frontend: Arc<Mutex<Frontend>>) -> Self {
        Self {
            tray: TrayManager::new(),
            frontend,
        }
    }
}

impl Pollable for TrayService {
    fn poll(&mut self) {
        if let Some(action) = self.tray.poll() {
            match action {
                TrayAction::Show => {
                    if let Ok(mut fe) = self.frontend.lock() {
                        fe.show_and_focus();
                    }
                }
                TrayAction::OpenSettings => {
                    if let Ok(mut fe) = self.frontend.lock() {
                        fe.show_settings();
                    }
                }
                TrayAction::Quit => {
                    slint::quit_event_loop().ok();
                }
            }
        }
    }
}
