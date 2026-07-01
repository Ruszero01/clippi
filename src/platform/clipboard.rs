//! --- Clipboard listener trait and platform implementations ---
//!
//! --- Provides multi-format clipboard monitoring with detection priority: ---
//! --- Files > Image > Link > Color > RichText > PlainText ---

use crate::core::color::detect_color;
use crate::core::paths::images_dir;
use crate::core::types::{
    is_email, is_image_extension, is_markdown_like, is_path, is_phone, is_url, ClipboardItem,
    ContentType, FileData, FileInfo, RichData, SourceAppInfo,
};
use crate::platform::source;
use crate::services::favicon;
use clipboard_rs::common::RustImage;
use clipboard_rs::{Clipboard, ClipboardContext, ContentFormat};
use std::collections::hash_map::DefaultHasher;
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use objc2_app_kit::NSPasteboard;

static CLIPBOARD_ACCESS: Mutex<()> = Mutex::new(());
static THUMBNAIL_READY: AtomicBool = AtomicBool::new(false);
static RECENT_IMAGE_FILE_REFERENCE: Mutex<Option<Instant>> = Mutex::new(None);
static THUMBNAIL_JOBS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

pub(crate) fn take_thumbnail_ready() -> bool {
    THUMBNAIL_READY.swap(false, Ordering::SeqCst)
}

pub(crate) fn image_thumbnail_path(hash: u64) -> Option<std::path::PathBuf> {
    let thumb_path = images_dir().join(format!("thumb_{hash:016x}.png"));
    thumbnail_file_is_valid(&thumb_path).then_some(thumb_path)
}

pub(crate) fn ensure_thumbnail_for_image(image_path: &str, hash: u64) {
    let source = std::path::PathBuf::from(image_path);
    if !source.exists() || image_thumbnail_path(hash).is_some() {
        return;
    }

    {
        let mut jobs = THUMBNAIL_JOBS.lock().unwrap_or_else(|e| e.into_inner());
        if jobs.contains(&hash) {
            return;
        }
        jobs.push(hash);
    }

    let img_dir = images_dir();
    std::thread::spawn(move || {
        generate_thumbnail(&source, &img_dir, hash);
        THUMBNAIL_JOBS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|job_hash| *job_hash != hash);
    });
}

fn mark_recent_image_file_reference() {
    *RECENT_IMAGE_FILE_REFERENCE
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
}

fn has_recent_image_file_reference() -> bool {
    RECENT_IMAGE_FILE_REFERENCE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some_and(|at| at.elapsed() < Duration::from_millis(1500))
}

pub(crate) fn with_clipboard_access<T>(operation: impl FnOnce() -> T) -> T {
    let _guard = CLIPBOARD_ACCESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    operation()
}

pub(crate) fn with_clipboard_context<T>(
    operation: impl FnOnce(&ClipboardContext) -> T,
) -> Option<T> {
    with_clipboard_access(|| ClipboardContext::new().ok().map(|ctx| operation(&ctx)))
}

/// Shared clipboard buffer for passing data to services
#[derive(Clone)]
pub struct ClipboardShared {
    pub pending: Arc<Mutex<Vec<ClipboardItem>>>,
    pub batch_pasting: Arc<AtomicBool>,
    pub clear_selection_requested: Arc<AtomicBool>,
    pub skip_next: Arc<AtomicBool>,
}

impl ClipboardShared {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(Vec::new())),
            batch_pasting: Arc::new(AtomicBool::new(false)),
            clear_selection_requested: Arc::new(AtomicBool::new(false)),
            skip_next: Arc::new(AtomicBool::new(false)),
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
/// Priority: Files > Image > Link > Color > RichText > PlainText
fn detect_clipboard_content(ctx: &ClipboardContext) -> Option<ClipboardItem> {
    // --- Capture source app info at detection time (only once, not on re-copy) ---
    let source_info = source::get_clipboard_owner_info();
    detect_files(ctx, &source_info)
        .or_else(|| detect_image(ctx, &source_info))
        .or_else(|| detect_text_content(ctx, &source_info))
}

/// --- File list detection (CF_HDROP on Windows, NSFilenames on macOS) ---
fn detect_files(
    ctx: &ClipboardContext,
    source_info: &Option<SourceAppInfo>,
) -> Option<ClipboardItem> {
    let has_files = ctx.has(ContentFormat::Files);
    if has_files {
        let files_result = ctx.get_files();
        if let Ok(files) = files_result {
            if !files.is_empty() {
                let entries: Vec<FileInfo> = files
                    .iter()
                    .map(|path| {
                        let p = std::path::Path::new(path);
                        FileInfo {
                            name: p
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| path.clone()),
                            path: path.clone(),
                            is_dir: p.is_dir(),
                        }
                    })
                    .collect();
                // Compute total file size in background thread (avoids blocking main thread poll)
                let total_size: i64 = entries
                    .iter()
                    .filter_map(|f| std::fs::metadata(&f.path).ok())
                    .map(|m| m.len())
                    .sum::<u64>() as i64;

                // Single image file → treat as Image type for thumbnail preview
                if entries.len() == 1 && !entries[0].is_dir && is_image_extension(&entries[0].path)
                {
                    let path = entries[0].path.clone();
                    let hash = hash_image_file_reference(&path);
                    let (iw, ih) = image::image_dimensions(&path).unwrap_or((0, 0));
                    ensure_thumbnail_for_image(&path, hash);

                    mark_recent_image_file_reference();
                    return Some(ClipboardItem::new_image(
                        0,
                        &path,
                        hash,
                        iw,
                        ih,
                        source_info.as_ref(),
                    ));
                }

                // --- Multi-file, single directory, or single non-image file → File type ---
                let file_data = FileData { files: entries };
                let mut hasher = DefaultHasher::new();
                for f in &file_data.files {
                    f.path.hash(&mut hasher);
                }
                let hash = hasher.finish();

                return Some(ClipboardItem::new_file(
                    0,
                    &file_data,
                    hash,
                    source_info.as_ref(),
                    total_size,
                ));
            }
        }
    }
    None
}

fn hash_image_file_reference(path: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);

    if let Ok(meta) = std::fs::metadata(path) {
        meta.len().hash(&mut hasher);
        if let Ok(modified) = meta.modified() {
            if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
                duration.as_nanos().hash(&mut hasher);
            }
        }
    }

    hasher.finish()
}

fn detect_image(
    ctx: &ClipboardContext,
    source_info: &Option<SourceAppInfo>,
) -> Option<ClipboardItem> {
    if ctx.has(ContentFormat::Image) {
        if let Some((png_data, iw, ih)) = read_clipboard_image_png(ctx) {
            let mut hasher = DefaultHasher::new();
            hasher.write(&png_data);
            let hash = hasher.finish();

            let img_dir = images_dir();
            let file_name = format!("{:016x}.png", hash);
            let file_path = img_dir.join(&file_name);

            if !file_path.exists() {
                if let Err(e) = std::fs::write(&file_path, &png_data) {
                    log::warn!(
                        "detect_image: failed to save clipboard image cache {}: {e}",
                        file_path.display()
                    );
                    return None;
                }
            }

            ensure_thumbnail_for_image(file_path.to_str().unwrap_or(""), hash);

            return Some(ClipboardItem::new_image(
                0,
                file_path.to_str().unwrap_or(""),
                hash,
                iw,
                ih,
                source_info.as_ref(),
            ));
        }
    }
    None
}

fn detect_text_content(
    ctx: &ClipboardContext,
    source_info: &Option<SourceAppInfo>,
) -> Option<ClipboardItem> {
    if let Ok(text) = ctx.get_text() {
        if text.is_empty() {
            return None;
        }

        if is_url(&text) {
            // --- Prefetch favicon in background thread (non-critical) ---
            let domain = crate::core::types::url_to_domain(&text);
            let _ = favicon::ensure_favicon_cached(&domain);
            let mut item = ClipboardItem::new_text(
                0,
                &text,
                ContentType::PlainText,
                source_info.as_ref(),
                None,
            );
            item.meta_type = "link".to_string();
            return Some(item);
        }

        if is_path(&text) {
            let drive_label = crate::core::types::path_drive_label(&text);
            let rich = drive_label.as_ref().map(|label| RichData {
                drive_label: Some(label.clone()),
                ..Default::default()
            });
            let mut item = ClipboardItem::new_text(
                0,
                &text,
                ContentType::PlainText,
                source_info.as_ref(),
                rich.as_ref(),
            );
            item.meta_type = "path".to_string();
            return Some(item);
        }

        // Color detection: hash the normalized color value for dedup
        if let Some(color) = detect_color(&text) {
            let mut hasher = DefaultHasher::new();
            color.to_hex_normalized().hash(&mut hasher);
            let hash = hasher.finish();
            let mut item = ClipboardItem::new_text(
                0,
                &text,
                ContentType::PlainText,
                source_info.as_ref(),
                None,
            );
            // Override the text-based hash with the normalized color hash for dedup
            item.content_hash = hash;
            item.meta_type = "color".to_string();
            return Some(item);
        }

        // --- Email / phone detection: record as plain_text with meta_type set ---
        if is_email(&text) || is_phone(&text) {
            let meta = if is_email(&text) {
                "email".to_string()
            } else {
                "phone".to_string()
            };
            let mut item = ClipboardItem::new_text(
                0,
                &text,
                ContentType::PlainText,
                source_info.as_ref(),
                None,
            );
            item.meta_type = meta;
            return Some(item);
        }

        if ctx.has(ContentFormat::Html) || ctx.has(ContentFormat::Rtf) {
            let html = ctx
                .get_html()
                .ok()
                .map(|html| normalize_clipboard_html(&html));
            let rtf = ctx.get_rich_text().ok();
            if html.is_some() || rtf.is_some() {
                let rich = RichData {
                    html,
                    rtf,
                    ..Default::default()
                };
                return Some(ClipboardItem::new_text(
                    0,
                    &text,
                    ContentType::RichText,
                    source_info.as_ref(),
                    Some(&rich),
                ));
            }
        }

        if is_markdown_like(&text) {
            let mut item = ClipboardItem::new_text(
                0,
                &text,
                ContentType::RichText,
                source_info.as_ref(),
                None,
            );
            item.meta_type = "markdown".into();
            return Some(item);
        }

        return Some(ClipboardItem::new_text(
            0,
            &text,
            ContentType::PlainText,
            source_info.as_ref(),
            None,
        ));
    }

    None
}

fn normalize_clipboard_html(html: &str) -> String {
    let Some(header_end) = html.find("<html").or_else(|| html.find("<!DOCTYPE")) else {
        return html.to_string();
    };

    let header = &html[..header_end];
    if !header.lines().any(|line| line.starts_with("Version:")) {
        return html.to_string();
    }

    if let (Some(start), Some(end)) = (
        parse_cf_html_offset(header, "StartFragment:"),
        parse_cf_html_offset(header, "EndFragment:"),
    ) {
        if start < end && end <= html.len() {
            return String::from_utf8_lossy(&html.as_bytes()[start..end])
                .trim()
                .to_string();
        }
    }

    html[header_end..]
        .replace("<!--StartFragment-->", "")
        .replace("<!--EndFragment-->", "")
        .trim()
        .to_string()
}

fn parse_cf_html_offset(header: &str, key: &str) -> Option<usize> {
    header.lines().find_map(|line| {
        line.strip_prefix(key)
            .and_then(|value| value.trim().parse::<usize>().ok())
    })
}

/// Parse PNG image dimensions from raw bytes without full decoding.
/// Only reads the 24-byte header (signature + IHDR chunk header).
fn png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 24 || data[0..8] != [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        return None;
    }
    let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Some((w, h))
}

/// Extract PNG bytes and dimensions from a clipboard image.
/// Fast path: `get_buffer("PNG")` for native PNG (avoids decode→ re-encode).
/// Fallback: `get_image()` + `to_png()` for TIFF etc (e.g. macOS screenshots).
fn read_clipboard_image_png(ctx: &ClipboardContext) -> Option<(Vec<u8>, u32, u32)> {
    // --- Fast path: raw PNG bytes already on pasteboard ---
    if let Ok(raw_png) = ctx.get_buffer("PNG") {
        if let Some((w, h)) = png_dimensions(&raw_png) {
            return Some((raw_png, w, h));
        }
    }
    if has_recent_image_file_reference() {
        return None;
    }
    // --- Fallback: decode + re-encode (e.g. TIFF screenshots on macOS) ---
    let img = ctx.get_image().ok()?;
    if img.is_empty() {
        return None;
    }
    let (w, h) = img.get_size();
    let png_bytes = img.to_png().ok()?;
    Some((png_bytes.get_bytes().to_vec(), w, h))
}

/// Target thumbnail width matching the card content area logical-pixel width.
const THUMB_WIDTH: u32 = 310;

fn thumbnail_file_is_valid(path: &std::path::Path) -> bool {
    let mut header = [0_u8; 24];
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    file.read_exact(&mut header).is_ok() && png_dimensions(&header).is_some()
}

/// Generate a thumbnail by scaling the full image to match the card width,
/// preserving aspect ratio. Small images (≤ target width) are kept as-is.
fn generate_thumbnail(image_path: &std::path::Path, img_dir: &std::path::Path, hash: u64) {
    let thumb_path = img_dir.join(format!("thumb_{:016x}.png", hash));
    if !image_path.exists() {
        return;
    }
    if thumb_path.exists() {
        if thumbnail_file_is_valid(&thumb_path) {
            return;
        }
        let _ = std::fs::remove_file(&thumb_path);
    }
    if let Err(e) = std::fs::create_dir_all(img_dir) {
        log::warn!(
            "generate_thumbnail: failed to create image cache dir {}: {e}",
            img_dir.display()
        );
        return;
    }
    if let Ok(img) = image::open(image_path) {
        use image::GenericImageView;
        let (w, h) = img.dimensions();
        let thumb = if w <= THUMB_WIDTH {
            img
        } else {
            let ratio = THUMB_WIDTH as f64 / w as f64;
            let nh = (h as f64 * ratio) as u32;
            img.resize(THUMB_WIDTH, nh, image::imageops::FilterType::Lanczos3)
        };

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let tmp_path = img_dir.join(format!("thumb_{hash:016x}.{unique}.tmp.png"));

        if thumb
            .save_with_format(&tmp_path, image::ImageFormat::Png)
            .is_ok()
            && thumbnail_file_is_valid(&tmp_path)
            && std::fs::rename(&tmp_path, &thumb_path).is_ok()
        {
            THUMBNAIL_READY.store(true, Ordering::SeqCst);
        } else {
            let _ = std::fs::remove_file(&tmp_path);
        }
    }
}

// ============================================================================
// --- Generic Polling Listener (used by both Windows and macOS) ---
// ============================================================================

/// Generic clipboard listener that polls at a fixed interval.
/// Works on any platform supported by `clipboard-rs`.
struct PollingClipboardListener {
    running: Arc<AtomicBool>,
    startup_end: Arc<Mutex<Option<Instant>>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl PollingClipboardListener {
    fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            startup_end: Arc::new(Mutex::new(None)),
            handle: None,
        }
    }

    fn capture_baseline(&self) -> u64 {
        *self.startup_end.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        with_clipboard_context(detect_clipboard_content)
            .flatten()
            .map_or(0, |item| item.content_hash)
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
        let skip_next = shared.skip_next.clone();

        // Windows: use cheap sequence-number check to avoid opening the clipboard
        // --- and encoding large bitmaps to PNG every 50ms when nothing changed. ---
        #[cfg(target_os = "windows")]
        let last_seq = Arc::new(Mutex::new(
            // SAFETY: `GetClipboardSequenceNumber` is a stateless query that
            // returns a DWORD; callable from any thread at any time.
            unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() },
        ));

        // macOS: use NSPasteboard.changeCount for the same fast-path purpose.
        // --- changeCount increments on every pasteboard write, giving an efficient ---
        // --- signal before the expensive clipboard open + PNG encode. ---
        #[cfg(target_os = "macos")]
        let last_cc = Arc::new(Mutex::new(with_clipboard_access(|| {
            objc2_app_kit::NSPasteboard::generalPasteboard().changeCount()
        })));

        self.handle = Some(thread::spawn(move || {
            while running.load(Ordering::SeqCst) {
                // --- Skip recording during batch paste to avoid redundant entries ---
                if batch_pasting.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(50));
                    continue;
                }

                // --- Internal copy: caller already pushed to pending, listener skips this cycle ---
                // --- Also update last_seq/last_cc to prevent detecting internal writes next cycle ---
                if skip_next.swap(false, Ordering::SeqCst) {
                    #[cfg(target_os = "windows")]
                    {
                        let seq = unsafe {
                            windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber()
                        };
                        *last_seq.lock().unwrap_or_else(|e| e.into_inner()) = seq;
                    }
                    #[cfg(target_os = "macos")]
                    {
                        let cc = with_clipboard_access(|| {
                            NSPasteboard::generalPasteboard().changeCount()
                        });
                        *last_cc.lock().unwrap_or_else(|e| e.into_inner()) = cc;
                    }
                    thread::sleep(Duration::from_millis(50));
                    continue;
                }

                // Fast-path: if the clipboard sequence number hasn't changed,
                // --- skip the expensive clipboard open + image encoding entirely. ---
                #[cfg(target_os = "windows")]
                {
                    let seq = unsafe {
                        windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber()
                    };
                    let mut last = last_seq.lock().unwrap_or_else(|e| e.into_inner());
                    if seq == *last {
                        drop(last);
                        thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    *last = seq;
                }

                // --- macOS equivalent: NSPasteboard.changeCount increments on every ---
                // pasteboard write, serving the same role as the Windows sequence number.
                #[cfg(target_os = "macos")]
                {
                    let cc =
                        with_clipboard_access(|| NSPasteboard::generalPasteboard().changeCount());
                    let mut last = last_cc.lock().unwrap_or_else(|e| e.into_inner());
                    if cc == *last {
                        drop(last);
                        thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    *last = cc;
                }

                let startup_done = startup_end
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_none_or(|end| end.elapsed().as_millis() > 500);

                if let Some(item) = with_clipboard_context(detect_clipboard_content).flatten() {
                    // --- Update the hash tracker so external consumers can see ---
                    // --- the latest hash. We intentionally push even when the ---
                    // --- hash matches last round — a re-copy of the same content ---
                    // --- should refresh updated_at and bump the item to the top. ---
                    // --- The sequence-number fast-path above already skips ---
                    // --- no-change cycles efficiently. ---
                    {
                        let mut last = last_hash.lock().unwrap_or_else(|e| e.into_inner());
                        *last = item.content_hash;
                    }
                    if startup_done {
                        pending.lock().unwrap_or_else(|e| e.into_inner()).push(item);
                    }
                }

                thread::sleep(Duration::from_millis(50));
            }
        }));

        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

// ============================================================================
// --- Linux Stub ---
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

#[cfg(test)]
mod access_tests {
    use super::with_clipboard_access;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn clipboard_access_is_serialized_between_threads() {
        let (first_entered_tx, first_entered_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let (second_entered_tx, second_entered_rx) = mpsc::channel();

        let first = std::thread::spawn(move || {
            with_clipboard_access(|| {
                first_entered_tx.send(()).unwrap();
                release_first_rx.recv().unwrap();
            });
        });
        first_entered_rx.recv().unwrap();

        let second = std::thread::spawn(move || {
            with_clipboard_access(|| {
                second_entered_tx.send(()).unwrap();
            });
        });

        assert!(second_entered_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err());
        release_first_tx.send(()).unwrap();
        second_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();

        first.join().unwrap();
        second.join().unwrap();
    }
}
