//! Tray service - handles tray business logic

use crate::core::frontend::Frontend;
use crate::looper::Pollable;
use crate::platform::tray::{TrayAction, TrayManager};
use crate::services::update;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// GitHub releases page for the Clippi project.
const RELEASES_URL: &str = "https://github.com/Ruszero01/clippi/releases";

pub struct TrayService {
    tray: TrayManager,
    frontend: Arc<Mutex<Frontend>>,
    restart_flag: Arc<AtomicBool>,
}

impl TrayService {
    pub fn new(frontend: Arc<Mutex<Frontend>>, restart_flag: Arc<AtomicBool>) -> Self {
        Self {
            tray: TrayManager::new(),
            frontend,
            restart_flag,
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
                TrayAction::Restart => {
                    self.restart_flag.store(true, Ordering::SeqCst);
                    slint::quit_event_loop().ok();
                }
                TrayAction::Quit => {
                    slint::quit_event_loop().ok();
                }
                TrayAction::CheckUpdate => {
                    update::open_releases_page(RELEASES_URL);
                }
            }
        }
    }
}
