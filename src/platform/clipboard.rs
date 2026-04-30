//! Clipboard listener trait and platform implementations

use crate::core::types::ClipboardItem;
use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

/// Shared clipboard buffer for passing data to services
#[derive(Clone)]
pub struct ClipboardShared {
    pub pending: Arc<Mutex<Vec<ClipboardItem>>>,
}

impl ClipboardShared {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Default for ClipboardShared {
    fn default() -> Self {
        Self::new()
    }
}

/// Clipboard listener trait
pub trait ClipboardListener: Send {
    fn start(&mut self, shared: &ClipboardShared) -> Result<(), Box<dyn Error + Send + Sync>>;
    fn stop(&mut self);
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use clipboard_rs::{Clipboard, ClipboardContext};
    use std::time::Instant;

    pub struct WindowsClipboardListener {
        running: Arc<Mutex<bool>>,
    }

    impl WindowsClipboardListener {
        pub fn new() -> Self {
            Self {
                running: Arc::new(Mutex::new(false)),
            }
        }
    }

    impl ClipboardListener for WindowsClipboardListener {
        fn start(&mut self, shared: &ClipboardShared) -> Result<(), Box<dyn Error + Send + Sync>> {
            *self.running.lock().unwrap() = true;
            let running = self.running.clone();
            let pending = shared.pending.clone();
            let last_hash = Arc::new(AtomicU64::new(0));
            let startup_end = Arc::new(Mutex::new(Option::<Instant>::None));

            // Capture baseline hash of current clipboard at startup
            if let Ok(ctx) = ClipboardContext::new() {
                if let Ok(text) = ctx.get_text() {
                    if !text.is_empty() {
                        use std::collections::hash_map::DefaultHasher;
                        use std::hash::{Hash, Hasher};
                        let mut hasher = DefaultHasher::new();
                        text.hash(&mut hasher);
                        let baseline = hasher.finish();
                        last_hash.store(baseline, Ordering::SeqCst);
                        *startup_end.lock().unwrap() = Some(Instant::now());
                    }
                }
            }

            let startup_end_clone = startup_end.clone();
            thread::spawn(move || {
                while *running.lock().unwrap() {
                    if let Ok(ctx) = ClipboardContext::new() {
                        if let Ok(text) = ctx.get_text() {
                            if !text.is_empty() {
                                use std::collections::hash_map::DefaultHasher;
                                use std::hash::{Hash, Hasher};
                                let mut hasher = DefaultHasher::new();
                                text.hash(&mut hasher);
                                let hash = hasher.finish();

                                // Only record if hash changed AND startup period is over
                                if hash != last_hash.load(Ordering::SeqCst) {
                                    let startup = startup_end_clone.lock().unwrap();
                                    if startup.map_or(false, |end| end.elapsed().as_millis() > 500) {
                                        last_hash.store(hash, Ordering::SeqCst);
                                        let item = ClipboardItem::new_text(0, &text);
                                        pending.lock().unwrap().push(item);
                                    } else {
                                        // Within startup period, just update hash but don't record
                                        last_hash.store(hash, Ordering::SeqCst);
                                    }
                                }
                            }
                        }
                    }
                    thread::sleep(std::time::Duration::from_millis(200));
                }
            });

            Ok(())
        }

        fn stop(&mut self) {
            *self.running.lock().unwrap() = false;
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    pub struct MacosClipboardListener;

    impl MacosClipboardListener {
        pub fn new() -> Self {
            Self
        }
    }

    impl ClipboardListener for MacosClipboardListener {
        fn start(&mut self, _shared: &ClipboardShared) -> Result<(), Box<dyn Error + Send + Sync>> {
            todo!("macOS clipboard listener")
        }

        fn stop(&mut self) {}
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    pub struct LinuxClipboardListener;

    impl LinuxClipboardListener {
        pub fn new() -> Self {
            Self
        }
    }

    impl ClipboardListener for LinuxClipboardListener {
        fn start(&mut self, _shared: &ClipboardShared) -> Result<(), Box<dyn Error + Send + Sync>> {
            todo!("Linux clipboard listener")
        }

        fn stop(&mut self) {}
    }
}

#[cfg(target_os = "windows")]
pub use windows::WindowsClipboardListener;

#[cfg(target_os = "macos")]
pub use macos::MacosClipboardListener;

#[cfg(target_os = "linux")]
pub use linux::LinuxClipboardListener;

pub fn create_listener() -> Box<dyn ClipboardListener> {
    #[cfg(target_os = "windows")]
    {
        Box::new(WindowsClipboardListener::new())
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(MacosClipboardListener::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(LinuxClipboardListener::new())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        panic!("Unsupported platform")
    }
}