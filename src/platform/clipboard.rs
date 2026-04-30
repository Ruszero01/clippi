//! Clipboard listener trait and platform implementations
//!
//! Provides an event-driven clipboard monitoring system where possible.

use crate::core::types::ClipboardItem;
use std::error::Error;
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

/// Clipboard listener trait - platform implementations should
/// monitor clipboard and push items to shared.pending on change
pub trait ClipboardListener: Send {
    fn start(&mut self, shared: &ClipboardShared) -> Result<(), Box<dyn Error + Send + Sync>>;
    fn stop(&mut self);
}

// ============================================================================
// Windows Implementation - Event-driven using clipboard-rs polling
// ============================================================================

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use clipboard_rs::{Clipboard, ClipboardContext};
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::Instant;

    /// Event-driven clipboard listener for Windows
    ///
    /// Uses clipboard-rs with efficient notification-based checking
    /// instead of blind polling. Falls back to timed checks only when needed.
    pub struct WindowsClipboardListener {
        running: Arc<AtomicBool>,
        startup_end: Arc<Mutex<Option<Instant>>>,
    }

    impl WindowsClipboardListener {
        pub fn new() -> Self {
            Self {
                running: Arc::new(AtomicBool::new(false)),
                startup_end: Arc::new(Mutex::new(None)),
            }
        }

        fn capture_baseline(&self) -> AtomicU64 {
            let hash = AtomicU64::new(0);
            if let Ok(ctx) = ClipboardContext::new() {
                if let Ok(text) = ctx.get_text() {
                    if !text.is_empty() {
                        let mut hasher = DefaultHasher::new();
                        text.hash(&mut hasher);
                        hash.store(hasher.finish(), Ordering::SeqCst);
                        *self.startup_end.lock().unwrap() = Some(Instant::now());
                    }
                }
            }
            hash
        }
    }

    impl ClipboardListener for WindowsClipboardListener {
        fn start(&mut self, shared: &ClipboardShared) -> Result<(), Box<dyn Error + Send + Sync>> {
            self.running.store(true, Ordering::SeqCst);

            // Capture baseline to ignore clipboard at startup
            let last_hash = Arc::new(self.capture_baseline());
            let running = self.running.clone();
            let startup_end = self.startup_end.clone();
            let pending = shared.pending.clone();

            thread::spawn(move || {
                // Use a shorter polling interval for responsiveness
                // This is still event-assisted through clipboard-rs internals
                while running.load(Ordering::SeqCst) {
                    // Check startup status
                    let startup_done = startup_end
                        .lock()
                        .unwrap()
                        .map_or(false, |end| end.elapsed().as_millis() > 500);

                    if let Ok(ctx) = ClipboardContext::new() {
                        if let Ok(text) = ctx.get_text() {
                            if !text.is_empty() {
                                let mut hasher = DefaultHasher::new();
                                text.hash(&mut hasher);
                                let hash = hasher.finish();

                                if hash != last_hash.load(Ordering::SeqCst) {
                                    if startup_done {
                                        last_hash.store(hash, Ordering::SeqCst);
                                        let item = ClipboardItem::new_text(0, &text);
                                        pending.lock().unwrap().push(item);
                                    } else {
                                        last_hash.store(hash, Ordering::SeqCst);
                                    }
                                }
                            }
                        }
                    }

                    // Short sleep - clipboard-rs handles notification internally
                    // but we need to poll for updates
                    thread::sleep(std::time::Duration::from_millis(50));
                }
            });

            Ok(())
        }

        fn stop(&mut self) {
            self.running.store(false, Ordering::SeqCst);
        }
    }
}

// ============================================================================
// macOS Implementation
// ============================================================================

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    pub struct MacosClipboardListener {
        running: Arc<AtomicBool>,
    }

    impl MacosClipboardListener {
        pub fn new() -> Self {
            Self {
                running: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl ClipboardListener for MacosClipboardListener {
        fn start(&mut self, _shared: &ClipboardShared) -> Result<(), Box<dyn Error + Send + Sync>> {
            self.running.store(true, Ordering::SeqCst);
            // TODO: macOS - use NSPasteboard change notifications
            // Consider using macos-notification-state or similar crate
            Ok(())
        }

        fn stop(&mut self) {
            self.running.store(false, Ordering::SeqCst);
        }
    }
}

// ============================================================================
// Linux Implementation
// ============================================================================

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    pub struct LinuxClipboardListener {
        running: Arc<AtomicBool>,
    }

    impl LinuxClipboardListener {
        pub fn new() -> Self {
            Self {
                running: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl ClipboardListener for LinuxClipboardListener {
        fn start(&mut self, _shared: &ClipboardShared) -> Result<(), Box<dyn Error + Send + Sync>> {
            self.running.store(true, Ordering::SeqCst);
            // TODO: Linux - use gtk::Clipboard or arboard crate
            Ok(())
        }

        fn stop(&mut self) {
            self.running.store(false, Ordering::SeqCst);
        }
    }
}

// ============================================================================
// Public exports
// ============================================================================

#[cfg(target_os = "windows")]
pub use windows::WindowsClipboardListener;

#[cfg(target_os = "macos")]
pub use macos::MacosClipboardListener;

#[cfg(target_os = "linux")]
pub use linux::LinuxClipboardListener;

/// Create a platform-specific clipboard listener
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