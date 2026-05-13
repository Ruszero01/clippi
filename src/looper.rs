//! Looper - unified polling center using a single timer

use crate::services::clipboard::ClipboardService;
use crate::services::focus::FocusService;
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
    focus_service: Arc<Mutex<Option<FocusService>>>,
}

impl Looper {
    pub fn new() -> Self {
        Self {
            timer: Timer::default(),
            services: Vec::new(),
            #[allow(clippy::arc_with_non_send_sync)]
            clipboard_service: Arc::new(Mutex::new(None)),
            #[allow(clippy::arc_with_non_send_sync)]
            hotkey_service: Arc::new(Mutex::new(None)),
            #[allow(clippy::arc_with_non_send_sync)]
            focus_service: Arc::new(Mutex::new(None)),
        }
    }

    /// Register a service to be polled
    pub fn add_service(&mut self, service: Box<dyn Pollable>) {
        self.services.push(service);
    }

    /// Register clipboard service (keeps concrete type for special methods)
    pub fn set_clipboard_service(&mut self, service: ClipboardService) {
        *self.clipboard_service.lock().expect("clipboard service lock poisoned") = Some(service);
    }

    /// Register hotkey service (keeps concrete type for special methods)
    pub fn set_hotkey_service(&mut self, service: HotkeyService) {
        *self.hotkey_service.lock().expect("hotkey service lock poisoned") = Some(service);
    }

    /// Register focus service (keeps concrete type for special methods)
    pub fn set_focus_service(&mut self, service: FocusService) {
        *self.focus_service.lock().expect("focus service lock poisoned") = Some(service);
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

    /// Try to access focus service
    pub fn try_with_focus_service<F, R>(&self, f: F) -> Result<R, ()>
    where
        F: FnOnce(&mut FocusService) -> R,
    {
        let mut fs = match self.focus_service.lock() {
            Ok(fs) => fs,
            Err(_) => return Err(()),
        };
        if let Some(ref mut fs) = *fs {
            Ok(f(fs))
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
        let fs = Arc::clone(&self.focus_service);

        self.timer.start(TimerMode::Repeated, Duration::from_millis(200), move || {
            for svc in &mut services {
                svc.poll();
            }
            if let Some(ref mut cs) = *cs.lock().expect("clipboard service lock poisoned") {
                cs.poll();
            }
            if let Some(ref mut hk) = *hk.lock().expect("hotkey service lock poisoned") {
                hk.poll();
            }
            if let Some(ref mut fs) = *fs.lock().expect("focus service lock poisoned") {
                fs.poll();
            }
        });
    }

    pub fn stop(&mut self) {
        self.timer.stop();
        if let Some(ref mut cs) = *self.clipboard_service.lock().expect("clipboard service lock poisoned") {
            cs.stop();
        }
        if let Some(ref mut hk) = *self.hotkey_service.lock().expect("hotkey service lock poisoned") {
            hk.stop();
        }
        if let Some(ref mut fs) = *self.focus_service.lock().expect("focus service lock poisoned") {
            fs.stop();
        }
    }
}

impl Default for Looper {
    fn default() -> Self {
        Self::new()
    }
}