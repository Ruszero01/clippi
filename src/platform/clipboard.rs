//! Clipboard listener trait and platform implementations
//!
//! Provides multi-format clipboard monitoring with detection priority:
//! Image > Link > Color > RichText > PlainText

use crate::core::color::detect_color;
use crate::core::types::{is_url_or_path, ClipboardItem, ContentType, RichData};
use crate::core::paths::images_dir;
use crate::platform::source;
use clipboard_rs::common::RustImage;
use clipboard_rs::{Clipboard, ClipboardContext, ContentFormat};
use std::collections::hash_map::DefaultHasher;
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Shared clipboard buffer for passing data to services
#[derive(Clone)]
pub struct ClipboardShared {
    pub pending: Arc<Mutex<Vec<ClipboardItem>>>,
    pub batch_pasting: Arc<AtomicBool>,
    pub clear_selection_requested: Arc<AtomicBool>,
}

impl ClipboardShared {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(Vec::new())),
            batch_pasting: Arc::new(AtomicBool::new(false)),
            clear_selection_requested: Arc::new(AtomicBool::new(false)),
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

/// Shared clipboard content detection (platform-agnostic).
/// Priority: Image > Link > Color > RichText > PlainText
fn detect_clipboard_content(ctx: &ClipboardContext) -> Option<ClipboardItem> {
    // Capture source app info at detection time (only once, not on re-copy)
    let source_info = source::get_clipboard_owner_info();

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
                    source_info.as_ref(),
                ));
            }
        }
    }

    if let Ok(text) = ctx.get_text() {
        if text.is_empty() {
            return None;
        }

        if is_url_or_path(&text) {
            return Some(ClipboardItem::new_text(0, &text, ContentType::Link, source_info.as_ref(), None));
        }

        // Color detection: hash the normalized color value for dedup
        if let Some(color) = detect_color(&text) {
            let mut hasher = DefaultHasher::new();
            color.to_hex_normalized().hash(&mut hasher);
            let hash = hasher.finish();
            return Some(ClipboardItem::new_color(0, &text, hash, source_info.as_ref()));
        }

        if ctx.has(ContentFormat::Html) || ctx.has(ContentFormat::Rtf) {
            let html = ctx.get_html().ok();
            let rtf = ctx.get_rich_text().ok();
            if html.is_some() || rtf.is_some() {
                let rich = RichData { html, rtf };
                return Some(ClipboardItem::new_text(0, &text, ContentType::RichText, source_info.as_ref(), Some(&rich)));
            }
        }

        return Some(ClipboardItem::new_text(0, &text, ContentType::PlainText, source_info.as_ref(), None));
    }

    None
}

// ============================================================================
// Generic Polling Listener (used by both Windows and macOS)
// ============================================================================

/// Generic clipboard listener that polls at a fixed interval.
/// Works on any platform supported by `clipboard-rs`.
struct PollingClipboardListener {
    running: Arc<AtomicBool>,
    startup_end: Arc<Mutex<Option<Instant>>>,
}

impl PollingClipboardListener {
    fn new() -> Self {
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

impl ClipboardListener for PollingClipboardListener {
    fn start(&mut self, shared: &ClipboardShared) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.running.store(true, Ordering::SeqCst);

        let last_hash = Arc::new(Mutex::new(self.capture_baseline()));
        let running = self.running.clone();
        let startup_end = self.startup_end.clone();
        let pending = shared.pending.clone();
        let batch_pasting = shared.batch_pasting.clone();

        thread::spawn(move || {
            while running.load(Ordering::SeqCst) {
                let startup_done = startup_end
                    .lock()
                    .unwrap()
                    .map_or(false, |end| end.elapsed().as_millis() > 500);

                // 批量粘贴期间跳过记录，避免产生冗余条目
                if batch_pasting.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(50));
                    continue;
                }

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

                thread::sleep(Duration::from_millis(50));
            }
        });

        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

// ============================================================================
// Linux Stub
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

#[cfg(target_os = "linux")]
pub use linux::LinuxClipboardListener;

/// Create a platform-specific clipboard listener
pub fn create_listener() -> Box<dyn ClipboardListener> {
    #[cfg(not(target_os = "linux"))]
    {
        Box::new(PollingClipboardListener::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(LinuxClipboardListener::new())
    }
}
