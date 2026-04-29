//! Clipboard listener trait and platform implementations

use crate::core::types::ClipboardItem;
use std::error::Error;

/// Clipboard listener - platform-agnostic trait
pub trait ClipboardListener: Send {
    fn start(&mut self, on_change: Box<dyn Fn(ClipboardItem) + Send + 'static>) -> Result<(), Box<dyn Error + Send + Sync>>;
    fn stop(&mut self);
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use crate::core::types::ClipboardItem;
    use clipboard_rs::{Clipboard, ClipboardContext};
    use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;

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
        fn start(&mut self, on_change: Box<dyn Fn(ClipboardItem) + Send + 'static>) -> Result<(), Box<dyn Error + Send + Sync>> {
            *self.running.lock().unwrap() = true;
            let running = self.running.clone();
            let next_id = Arc::new(AtomicI64::new(1));
            let last_hash = Arc::new(AtomicU64::new(0));

            // Polling thread
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

                                if hash != last_hash.load(Ordering::SeqCst) {
                                    last_hash.store(hash, Ordering::SeqCst);
                                    let id = next_id.fetch_add(1, Ordering::SeqCst);
                                    let item = ClipboardItem::new_text(id, &text);
                                    on_change(item);
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
        fn start(&mut self, _on_change: Box<dyn Fn(ClipboardItem) + Send + 'static>) -> Result<(), Box<dyn Error + Send + Sync>> {
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
        fn start(&mut self, _on_change: Box<dyn Fn(ClipboardItem) + Send + 'static>) -> Result<(), Box<dyn Error + Send + Sync>> {
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
