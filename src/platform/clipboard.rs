//! --- Clipboard listener trait and platform implementations ---
//!
//! --- Provides multi-format clipboard monitoring with detection priority: ---
//! --- Files > Image (Image+RichText coexistence -> Text) > Link > Path > Color > Email/Phone > RichText > Markdown > PlainText ---

use crate::core::color::detect_color;
use crate::core::paths::images_dir;
use crate::core::settings::capture_gate;
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
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use objc2_app_kit::NSPasteboard;

static CLIPBOARD_ACCESS: Mutex<()> = Mutex::new(());
static THUMBNAIL_READY: AtomicBool = AtomicBool::new(false);
static RECENT_IMAGE_FILE_REFERENCE: Mutex<Option<Instant>> = Mutex::new(None);
static THUMBNAIL_JOBS: Mutex<Vec<u64>> = Mutex::new(Vec::new());
const MAX_THUMBNAIL_JOBS: usize = 2;

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
        if jobs.len() >= MAX_THUMBNAIL_JOBS {
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
    /// App-level clipboard blacklist snapshot.
    /// Listener thread reads; main thread writes via `GpuiClipboardService::set_app_blacklist`.
    pub clipboard_app_blacklist: Arc<RwLock<Vec<String>>>,
}

impl ClipboardShared {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(Vec::new())),
            batch_pasting: Arc::new(AtomicBool::new(false)),
            clear_selection_requested: Arc::new(AtomicBool::new(false)),
            skip_next: Arc::new(AtomicBool::new(false)),
            clipboard_app_blacklist: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Default for ClipboardShared {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared gate: combines startup grace-period + app-blacklist checks with an
/// injectable content detector.  Returns `Some(item)` when capture is allowed,
/// `None` when the source should be skipped.
///
/// `detector` is only called when the gate passes — this guarantees that
/// blacklisted sources and the startup grace period never trigger expensive
/// clipboard-content extraction (image encoding / thumbnail writes / disk I/O).
pub(crate) fn detect_if_allowed(
    source_info: &Option<SourceAppInfo>,
    blacklist: &[String],
    startup_done: bool,
    detector: impl FnOnce() -> Option<ClipboardItem>,
) -> Option<ClipboardItem> {
    let source_app_name = source_info
        .as_ref()
        .map(|s| s.app_name.as_str())
        .unwrap_or("");
    if capture_gate(source_app_name, blacklist, startup_done) {
        detector()
    } else {
        None
    }
}

/// Clipboard listener trait - platform implementations should
/// monitor clipboard and push items to shared.pending on change
pub trait ClipboardListener: Send {
    fn start(&mut self, shared: &ClipboardShared) -> Result<(), Box<dyn Error + Send + Sync>>;
    fn stop(&mut self);
}

/// Shared clipboard content detection (platform-agnostic).
/// Priority: Files > Image (Image+RichText coexistence -> Text) > Link > Path > Color > Email/Phone > RichText > Markdown > PlainText
fn detect_clipboard_content(
    ctx: &ClipboardContext,
    source_info: &Option<SourceAppInfo>,
) -> Option<ClipboardItem> {
    detect_files(ctx, source_info)
        .or_else(|| {
            // When both image and rich text (HTML/RTF) coexist on the clipboard,
            // prefer the text. Apps like OneNote and Excel put both a rendered
            // image and formatted text on the clipboard simultaneously; users
            // expect to see the text content, not the image rendering.
            if ctx.has(ContentFormat::Image)
                && (ctx.has(ContentFormat::Html) || ctx.has(ContentFormat::Rtf))
            {
                detect_text_content(ctx, source_info).or_else(|| detect_image(ctx, source_info))
            } else {
                detect_image(ctx, source_info)
            }
        })
        .or_else(|| detect_text_content(ctx, source_info))
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
                let file_data = FileData {
                    files: entries,
                    ..Default::default()
                };
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

        let rich_data = read_clipboard_rich_data(ctx);

        if is_url(&text) {
            // --- Prefetch favicon in background thread (non-critical) ---
            let domain = crate::core::types::url_to_domain(&text);
            let _ = favicon::ensure_favicon_cached(&domain);
            let mut item = ClipboardItem::new_text(
                0,
                &text,
                ContentType::PlainText,
                source_info.as_ref(),
                rich_data.as_ref(),
            );
            item.meta_type = "link".to_string();
            return Some(item);
        }

        if is_path(&text) {
            let drive_label = crate::core::types::path_drive_label(&text);
            let mut rich = rich_data.clone();
            if let Some(label) = drive_label {
                rich.get_or_insert_with(RichData::default).drive_label = Some(label);
            }
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
                rich_data.as_ref(),
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
                rich_data.as_ref(),
            );
            item.meta_type = meta;
            return Some(item);
        }

        if let Some(rich) = rich_data {
            return Some(ClipboardItem::new_text(
                0,
                &text,
                ContentType::RichText,
                source_info.as_ref(),
                Some(&rich),
            ));
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

fn read_clipboard_rich_data(ctx: &ClipboardContext) -> Option<RichData> {
    if !(ctx.has(ContentFormat::Html) || ctx.has(ContentFormat::Rtf)) {
        return None;
    }

    let html = ctx
        .get_html()
        .ok()
        .map(|html| normalize_clipboard_html(&html));
    let rtf = ctx.get_rich_text().ok();
    (html.is_some() || rtf.is_some()).then_some(RichData {
        html,
        rtf,
        ..Default::default()
    })
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
            // Lossless PNG optimization — thumbnail is ~310px wide, takes < 1ms
            if let Ok(data) = std::fs::read(&thumb_path) {
                if let Ok(optimized) =
                    oxipng::optimize_from_memory(&data, &oxipng::Options::from_preset(2))
                {
                    let _ = std::fs::write(&thumb_path, &optimized);
                }
            }
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
        // Do NOT read clipboard content at startup.
        // - Windows: record the sequence number; the grace-period gate handles the
        //   first real detection after startup.
        // - macOS: frontmostApplication() returns Clippi itself at startup, so the
        //   original clipboard source is unknowable.  If we called content detection
        //   here, images from a blacklisted app would be written to disk before any
        //   filtering could run.
        *self.startup_end.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        0
    }
}

/// Clipboard owner handle. On Windows this is the HWND of the window
/// that most recently called `EmptyClipboard()`; it becomes NULL when that
/// window is destroyed (e.g. after delayed rendering on window close).
/// On macOS the concept doesn't apply — always returns 0.
#[cfg(target_os = "windows")]
fn current_clipboard_owner() -> isize {
    unsafe { windows_sys::Win32::System::DataExchange::GetClipboardOwner() as isize }
}

#[cfg(not(target_os = "windows"))]
fn current_clipboard_owner() -> isize {
    0
}

/// Snapshot of the most recent clipboard detection, used to distinguish
/// user-initiated copies from system-triggered delayed rendering.
#[derive(Clone, Copy)]
struct LastClipState {
    hash: u64,
    /// Windows HWND, or 0 on other platforms / no owner.
    owner: isize,
    time: Instant,
    /// Clipboard sequence number at time of last push. A delta of 1
    /// on the next detection means a single `SetClipboardData` call
    /// without a preceding `EmptyClipboard` — definitive delayed rendering.
    seq: u32,
}

/// Read the current clipboard sequence number.
/// Windows: `GetClipboardSequenceNumber`; macOS: `NSPasteboard.changeCount`.
#[cfg(target_os = "windows")]
fn current_clipboard_seq() -> u32 {
    unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() }
}

#[cfg(target_os = "macos")]
fn current_clipboard_seq() -> u32 {
    // `changeCount` doesn't require opening the pasteboard — no
    // `with_clipboard_access` lock needed, avoiding a deadlock when
    // called from inside `with_clipboard_context`.
    objc2_app_kit::NSPasteboard::generalPasteboard().changeCount() as u32
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn current_clipboard_seq() -> u32 {
    0
}

impl ClipboardListener for PollingClipboardListener {
    fn start(&mut self, shared: &ClipboardShared) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.running.store(true, Ordering::SeqCst);

        let last_state = Arc::new(Mutex::new(LastClipState {
            hash: self.capture_baseline(),
            owner: current_clipboard_owner(),
            time: Instant::now(),
            seq: current_clipboard_seq(),
        }));
        let running = self.running.clone();
        let startup_end = self.startup_end.clone();
        let pending = shared.pending.clone();
        let batch_pasting = shared.batch_pasting.clone();
        let skip_next = shared.skip_next.clone();
        let app_blacklist = shared.clipboard_app_blacklist.clone(); // Arc<RwLock<_>>

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

                // ── Startup grace-period gate ──
                // Skip content detection entirely during the first 500ms so that
                // clipboard content present at startup (from a potentially
                // blacklisted app) is never written to disk.
                let startup_done = startup_end
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_none_or(|end| end.elapsed().as_millis() > 500);
                if !startup_done {
                    thread::sleep(Duration::from_millis(50));
                    continue;
                }

                // ── Capture source identity once (before opening clipboard) ──
                let source_info = source::get_clipboard_owner_info();

                // ── Clone blacklist snapshot and release the read-lock ──
                // The guard is dropped at the end of this statement so the
                // RwLock is never held during expensive content detection
                // (image encoding / thumbnail generation / disk writes).
                let blacklist_snapshot = app_blacklist
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();

                // ── Unified gate + content detection ──
                if let Some(item) = detect_if_allowed(
                    &source_info,
                    &blacklist_snapshot,
                    true, // startup_done already checked above
                    || {
                        with_clipboard_context(|ctx| detect_clipboard_content(ctx, &source_info))
                            .flatten()
                    },
                ) {
                    // ── Duplicate suppression ──────────────────────────
                    let now = Instant::now();
                    let owner = current_clipboard_owner();
                    let seq = current_clipboard_seq();
                    let mut guard = last_state.lock().unwrap_or_else(|e| e.into_inner());

                    #[cfg(target_os = "windows")]
                    let suppress_duplicate = {
                        let same_hash = guard.hash == item.content_hash;
                        let delayed_fill = same_hash && seq.wrapping_sub(guard.seq) <= 2;
                        let owner_destroyed = same_hash && owner == 0 && guard.owner != 0;
                        let rapid_same = same_hash
                            && owner == guard.owner
                            && seq.wrapping_sub(guard.seq) > 2
                            && now.duration_since(guard.time).as_millis() < 2_000;
                        delayed_fill || owner_destroyed || rapid_same
                    };
                    #[cfg(not(target_os = "windows"))]
                    let suppress_duplicate = {
                        let _ = (&guard.hash, &guard.owner, &guard.time, &guard.seq);
                        false
                    };

                    if suppress_duplicate {
                        drop(guard);
                        thread::sleep(Duration::from_millis(50));
                        continue;
                    }

                    *guard = LastClipState {
                        hash: item.content_hash,
                        owner,
                        time: now,
                        seq,
                    };
                    drop(guard);
                    pending.lock().unwrap_or_else(|e| e.into_inner()).push(item);
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

#[cfg(test)]
mod gate_tests {
    use super::*;
    use crate::core::types::ClipboardItem;

    fn dummy_item() -> ClipboardItem {
        ClipboardItem::new_text(
            0,
            "test",
            crate::core::types::ContentType::PlainText,
            None,
            None,
        )
    }

    fn dummy_source(app_name: &str) -> Option<SourceAppInfo> {
        Some(SourceAppInfo {
            app_name: app_name.into(),
            icon_base64: String::new(),
        })
    }

    #[test]
    fn detector_not_called_during_grace_period() {
        let mut calls = 0u32;
        let result = detect_if_allowed(&None, &[], false, || {
            calls += 1;
            None
        });
        assert!(result.is_none());
        assert_eq!(calls, 0);
    }

    #[test]
    fn detector_not_called_for_blacklisted() {
        let info = dummy_source("KeePass");
        let mut calls = 0u32;
        let result = detect_if_allowed(&info, &["KeePass".into()], true, || {
            calls += 1;
            None
        });
        assert!(result.is_none());
        assert_eq!(calls, 0);
    }

    #[test]
    fn detector_called_for_normal() {
        let info = dummy_source("Notepad");
        let mut calls = 0u32;
        let result = detect_if_allowed(&info, &[], true, || {
            calls += 1;
            Some(dummy_item())
        });
        assert!(result.is_some());
        assert_eq!(calls, 1);
    }

    #[test]
    fn detector_called_for_unknown_source() {
        let mut calls = 0u32;
        let result = detect_if_allowed(&None, &[], true, || {
            calls += 1;
            Some(dummy_item())
        });
        assert!(result.is_some());
        assert_eq!(calls, 1);
    }

    #[test]
    fn gate_uses_latest_blacklist_argument() {
        let info = dummy_source("Notepad");
        let mut calls = 0u32;
        // Empty blacklist → allowed
        let r1 = detect_if_allowed(&info, &[], true, || {
            calls += 1;
            Some(dummy_item())
        });
        assert!(r1.is_some());
        assert_eq!(calls, 1);
        // Now blacklisted → blocked, detector not called again
        let r2 = detect_if_allowed(&info, &["Notepad".into()], true, || {
            calls += 1;
            None
        });
        assert!(r2.is_none());
        assert_eq!(calls, 1);
    }
}
