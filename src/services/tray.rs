//! Tray service - handles tray business logic

use crate::core::frontend::Frontend;
use crate::looper::Pollable;
use crate::platform::tray::{TrayAction, TrayManager};

pub struct TrayService {
    tray: TrayManager,
    frontend: Frontend,
}

impl TrayService {
    pub fn new(frontend: Frontend) -> Self {
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
                    self.frontend.show();
                }
                TrayAction::OpenSettings => {
                    self.frontend.show_settings();
                }
                TrayAction::Quit => {
                    slint::quit_event_loop().ok();
                }
            }
        }
    }
}
