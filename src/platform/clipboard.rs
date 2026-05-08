//! Clipboard listener trait and platform implementations
//!
//! Provides multi-format clipboard monitoring with detection priority:
//! Image > Link > RichText > PlainText

use crate::core::types::{is_url, ClipboardItem, ContentType};
use crate::core::paths::images_dir;
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
// Windows Implementation - Multi-format detection
// ============================================================================

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use clipboard_rs::common::RustImage;
    use clipboard_rs::{Clipboard, ClipboardContext, ContentFormat};
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

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

        fn capture_baseline(&self) -> u64 {
            let mut hasher = DefaultHasher::new();
            if let Ok(ctx) = ClipboardContext::new() {
                if let Ok(text) = ctx.get_text() {
                    if !text.is_empty() {
                        text.hash(&mut hasher);
                        *self.startup_end.lock().unwrap() = Some(Instant::now());
                    }
                }
                if ctx.has(ContentFormat::Image) {
                    hasher.write(b"[img]");
                    *self.startup_end.lock().unwrap() = Some(Instant::now());
                }
            }
            hasher.finish()
        }
    }

    fn detect_clipboard_content(ctx: &ClipboardContext) -> Option<ClipboardItem> {
        // Priority: Image > Link > RichText > PlainText

        // 1. Check for image data (screenshots, copied images)
        if ctx.has(ContentFormat::Image) {
            if let Ok(img) = ctx.get_image() {
                if !img.is_empty() {
                    let png_bytes = img.to_png().ok()?;
                    let mut hasher = DefaultHasher::new();
                    hasher.write(png_bytes.get_bytes());
                    let hash = hasher.finish();

                    let img_dir = images_dir();
                    let file_name = format!("{:016x}.png", hash);
                    let file_path = img_dir.join(&file_name);

                    if !file_path.exists() {
                        if png_bytes.save_to_path(file_path.to_str().unwrap_or("")).is_err() {
                            return None;
                        }
                    }

                    return Some(ClipboardItem::new_image(
                        0,
                        file_path.to_str().unwrap_or(""),
                        hash,
                    ));
                }
            }
        }

        // 2. Check text content for URL / RichText / PlainText
        if let Ok(text) = ctx.get_text() {
            if text.is_empty() {
                return None;
            }

            // Check for URL
            if is_url(&text) {
                return Some(ClipboardItem::new_text(0, &text, ContentType::Link));
            }

            // Check for HTML rich text
            if ctx.has(ContentFormat::Html) {
                return Some(ClipboardItem::new_text(0, &text, ContentType::RichText));
            }

            // Plain text fallback
            return Some(ClipboardItem::new_text(0, &text, ContentType::PlainText));
        }

        None
    }

    impl ClipboardListener for WindowsClipboardListener {
        fn start(&mut self, shared: &ClipboardShared) -> Result<(), Box<dyn Error + Send + Sync>> {
            self.running.store(true, Ordering::SeqCst);

            let last_hash = Arc::new(Mutex::new(self.capture_baseline()));
            let running = self.running.clone();
            let startup_end = self.startup_end.clone();
            let pending = shared.pending.clone();

            thread::spawn(move || {
                while running.load(Ordering::SeqCst) {
                    let startup_done = startup_end
                        .lock()
                        .unwrap()
                        .map_or(false, |end| end.elapsed().as_millis() > 500);

                    if let Ok(ctx) = ClipboardContext::new() {
                        let mut hasher = DefaultHasher::new();
                        let mut has_content = false;

                        if let Ok(text) = ctx.get_text() {
                            if !text.is_empty() {
                                text.hash(&mut hasher);
                                has_content = true;
                            }
                        }

                        // Also hash image presence to detect image-only changes
                        if ctx.has(ContentFormat::Image) {
                            hasher.write(b"[img]");
                            has_content = true;
                        }

                        if has_content {
                            let hash = hasher.finish();
                            let changed = {
                                let mut last = last_hash.lock().unwrap();
                                if hash != *last {
                                    *last = hash;
                                    true
                                } else {
                                    false
                                }
                            };

                            if changed && startup_done {
                                if let Some(item) = detect_clipboard_content(&ctx) {
                                    pending.lock().unwrap().push(item);
                                }
                            }
                        }
                    }

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
// macOS Implementation - Multi-format detection
// ============================================================================

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use clipboard_rs::common::RustImage;
    use clipboard_rs::{Clipboard, ClipboardContext, ContentFormat};
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    pub struct MacosClipboardListener {
        running: Arc<AtomicBool>,
        startup_end: Arc<Mutex<Option<Instant>>>,
    }

    impl MacosClipboardListener {
        pub fn new() -> Self {
            Self {
                running: Arc::new(AtomicBool::new(false)),
                startup_end: Arc::new(Mutex::new(None)),
            }
        }

        fn capture_baseline(&self) -> u64 {
            let mut hasher = DefaultHasher::new();
            if let Ok(ctx) = ClipboardContext::new() {
                if let Ok(text) = ctx.get_text() {
                    if !text.is_empty() {
                        text.hash(&mut hasher);
                        *self.startup_end.lock().unwrap() = Some(Instant::now());
                    }
                }
                if ctx.has(ContentFormat::Image) {
                    hasher.write(b"[img]");
                    *self.startup_end.lock().unwrap() = Some(Instant::now());
                }
            }
            hasher.finish()
        }
    }

    fn detect_clipboard_content(ctx: &ClipboardContext) -> Option<ClipboardItem> {
        // Priority: Image > Link > RichText > PlainText

        if ctx.has(ContentFormat::Image) {
            if let Ok(img) = ctx.get_image() {
                if !img.is_empty() {
                    let png_bytes = img.to_png().ok()?;
                    let mut hasher = DefaultHasher::new();
                    hasher.write(png_bytes.get_bytes());
                    let hash = hasher.finish();

                    let img_dir = images_dir();
                    let file_name = format!("{:016x}.png", hash);
                    let file_path = img_dir.join(&file_name);

                    if !file_path.exists() {
                        if png_bytes.save_to_path(file_path.to_str().unwrap_or("")).is_err() {
                            return None;
                        }
                    }

                    return Some(ClipboardItem::new_image(
                        0,
                        file_path.to_str().unwrap_or(""),
                        hash,
                    ));
                }
            }
        }

        if let Ok(text) = ctx.get_text() {
            if text.is_empty() {
                return None;
            }

            if is_url(&text) {
                return Some(ClipboardItem::new_text(0, &text, ContentType::Link));
            }

            if ctx.has(ContentFormat::Html) {
                return Some(ClipboardItem::new_text(0, &text, ContentType::RichText));
            }

            return Some(ClipboardItem::new_text(0, &text, ContentType::PlainText));
        }

        None
    }

    impl ClipboardListener for MacosClipboardListener {
        fn start(&mut self, shared: &ClipboardShared) -> Result<(), Box<dyn Error + Send + Sync>> {
            self.running.store(true, Ordering::SeqCst);

            let last_hash = Arc::new(Mutex::new(self.capture_baseline()));
            let running = self.running.clone();
            let startup_end = self.startup_end.clone();
            let pending = shared.pending.clone();

            thread::spawn(move || {
                while running.load(Ordering::SeqCst) {
                    let startup_done = startup_end
                        .lock()
                        .unwrap()
                        .map_or(false, |end| end.elapsed().as_millis() > 500);

                    if let Ok(ctx) = ClipboardContext::new() {
                        let mut hasher = DefaultHasher::new();
                        let mut has_content = false;

                        if let Ok(text) = ctx.get_text() {
                            if !text.is_empty() {
                                text.hash(&mut hasher);
                                has_content = true;
                            }
                        }

                        if ctx.has(ContentFormat::Image) {
                            hasher.write(b"[img]");
                            has_content = true;
                        }

                        if has_content {
                            let hash = hasher.finish();
                            let changed = {
                                let mut last = last_hash.lock().unwrap();
                                if hash != *last {
                                    *last = hash;
                                    true
                                } else {
                                    false
                                }
                            };

                            if changed && startup_done {
                                if let Some(item) = detect_clipboard_content(&ctx) {
                                    pending.lock().unwrap().push(item);
                                }
                            }
                        }
                    }

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
