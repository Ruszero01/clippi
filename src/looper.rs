//! Looper - unified polling center using a single timer

use crate::services::clipboard::ClipboardService;
use crate::services::hotkey::HotkeyService;
use slint::{Timer, TimerMode};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Trait for services that can be polled from the timer
pub trait Pollable {
    fn poll(&mut self);
    fn stop(&mut self) {}
}

pub struct Looper {
    timer: Timer,
    services: Vec<Box<dyn Pollable>>,
    clipboard_service: Arc<Mutex<Option<ClipboardService>>>,
    hotkey_service: Arc<Mutex<Option<HotkeyService>>>,
}

impl Looper {
    pub fn new() -> Self {
        Self {
            timer: Timer::default(),
            services: Vec::new(),
            clipboard_service: Arc::new(Mutex::new(None)),
            hotkey_service: Arc::new(Mutex::new(None)),
        }
    }

    /// Register a service to be polled
    pub fn add_service(&mut self, service: Box<dyn Pollable>) {
        self.services.push(service);
    }

    /// Register clipboard service (keeps concrete type for special methods)
    pub fn set_clipboard_service(&mut self, service: ClipboardService) {
        *self.clipboard_service.lock().unwrap() = Some(service);
    }

    /// Register hotkey service (keeps concrete type for special methods)
    pub fn set_hotkey_service(&mut self, service: HotkeyService) {
        *self.hotkey_service.lock().unwrap() = Some(service);
    }

    /// Try to access clipboard service
    pub fn try_with_clipboard_service<F, R>(&self, f: F) -> Result<R, ()>
    where
        F: FnOnce(&mut ClipboardService) -> R,
    {
        let mut cs = match self.clipboard_service.lock() {
            Ok(cs) => cs,
            Err(_) => return Err(()),
        };
        if let Some(ref mut cs) = *cs {
            Ok(f(cs))
        } else {
            Err(())
        }
    }

    /// Try to access hotkey service without panicking
    pub fn try_with_hotkey_service<F, R>(&self, f: F) -> Result<R, ()>
    where
        F: FnOnce(&mut HotkeyService) -> R,
    {
        let mut hk = match self.hotkey_service.lock() {
            Ok(hk) => hk,
            Err(_) => return Err(()),
        };
        if let Some(ref mut hk) = *hk {
            Ok(f(hk))
        } else {
            Err(())
        }
    }

    pub fn start(&mut self) {
        let mut services: Vec<Box<dyn Pollable>> = Vec::new();
        std::mem::swap(&mut self.services, &mut services);

        let cs = Arc::clone(&self.clipboard_service);
        let hk = Arc::clone(&self.hotkey_service);

        self.timer.start(TimerMode::Repeated, Duration::from_millis(200), move || {
            for svc in &mut services {
                svc.poll();
            }
            if let Some(ref mut cs) = *cs.lock().unwrap() {
                cs.poll();
            }
            if let Some(ref mut hk) = *hk.lock().unwrap() {
                hk.poll();
            }
        });
    }

    pub fn stop(&mut self) {
        self.timer.stop();
        if let Some(ref mut cs) = *self.clipboard_service.lock().unwrap() {
            cs.stop();
        }
        if let Some(ref mut hk) = *self.hotkey_service.lock().unwrap() {
            hk.stop();
        }
    }
}

impl Default for Looper {
    fn default() -> Self {
        Self::new()
    }
}