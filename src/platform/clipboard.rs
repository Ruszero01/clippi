//! --- Clipboard listener trait and platform implementations ---
//!
//! --- Provides multi-format clipboard monitoring with detection priority: ---
//! --- Files > Image (Image+RichText coexistence -> Text) > Link > Path > Color > Email/Phone > RichText > Markdown > PlainText ---

use crate::core::color::detect_color;
use crate::core::paths::images_dir;
use crate::core::settings::capture_gate;
use crate::core::types::{
    is_email, is_image_extension, is_markdown_like, is_path, is_phone, is_url, ClipboardItem,
    ContentType, FileData, FileInfo, PendingImageView, RichData, SourceAppInfo,
};
use crate::platform::source;
use crate::services::favicon;
use chrono::Utc;
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
use clipboard_rs::common::RustImage;
use clipboard_rs::{Clipboard, ClipboardContext, ContentFormat};
use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use objc2_app_kit::NSPasteboard;

static CLIPBOARD_ACCESS: Mutex<()> = Mutex::new(());
static THUMBNAIL_READY: AtomicBool = AtomicBool::new(false);
static RECENT_IMAGE_FILE_REFERENCE: Mutex<Option<Instant>> = Mutex::new(None);
static THUMBNAIL_JOBS: Mutex<Vec<u64>> = Mutex::new(Vec::new());
const MAX_THUMBNAIL_JOBS: usize = 2;
const MAX_CAPTURED_IMAGE_DIMENSION: u32 = 100_000;
// 128 MP covers 2000×40000 screenshots while keeping 8-bit RGBA below 512 MiB.
const MAX_CAPTURED_IMAGE_PIXELS: u64 = 128_000_000;
const MAX_DECODED_IMAGE_BYTES: u64 = 512 * 1024 * 1024;
#[cfg(any(target_os = "windows", target_os = "macos"))]
const MAX_CAPTURED_IMAGE_BYTES: usize = 512 * 1024 * 1024;
const MAX_IMAGE_PERSIST_JOBS: usize = 2;
const MAX_IMAGE_PERSIST_BYTES: usize = 512 * 1024 * 1024;

/// 结构化诊断日志：特性关闭时展开为空，生产构建零影响。
/// 注册 "PNG" 剪贴板格式并缓存其 ID（进程内只注册一次）。
#[cfg(target_os = "windows")]
fn png_format_id() -> u32 {
    use std::sync::OnceLock;
    static PNG_FORMAT_ID: OnceLock<u32> = OnceLock::new();
    *PNG_FORMAT_ID.get_or_init(|| {
        let name: [u16; 4] = [b'P' as u16, b'N' as u16, b'G' as u16, 0];
        unsafe { windows_sys::Win32::System::DataExchange::RegisterClipboardFormatW(name.as_ptr()) }
    })
}

/// 检查 Windows 剪贴板是否提供原生 PNG 格式（不触发渲染）。
#[cfg(target_os = "windows")]
fn png_format_available() -> bool {
    unsafe {
        windows_sys::Win32::System::DataExchange::IsClipboardFormatAvailable(png_format_id()) != 0
    }
}

/// 记录检测到序列号变化的时间点。
/// 记录从序列号变化到开始读取的耗时。
/// 记录剪贴板当前可用格式（IsClipboardFormatAvailable 不触发渲染、不读内容）。
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
    /// In-memory image placeholders awaiting background persistence. Not persisted.
    pub pending_images: Arc<Mutex<Vec<PendingImageView>>>,
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
            pending_images: Arc::new(Mutex::new(Vec::new())),
            clipboard_app_blacklist: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Default for ClipboardShared {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of clipboard content detection on the listener thread.
pub(crate) enum DetectionResult {
    /// A complete, ready-to-persist item (files, text, links, paths, colors, ...).
    Item(Box<ClipboardItem>),
    /// A raw image captured for background persistence. The final `ClipboardItem`
    /// is constructed by the worker only after the cache file is committed.
    Image(CapturedImage),
    /// Nothing recognized.
    None,
}

/// Raw image captured on the listener thread, handed to the persistence worker.
pub(crate) struct CapturedImage {
    raw: RawClipboardImage,
    width: u32,
    height: u32,
    raw_hash: u64,
    source: Option<SourceAppInfo>,
}

/// Shared gate: combines startup grace-period + app-blacklist checks with an
/// injectable content detector.
///
/// `detector` is only called when the gate passes — this guarantees that
/// blacklisted sources and the startup grace period never trigger expensive
/// clipboard-content extraction (image encoding / thumbnail writes / disk I/O).
pub(crate) fn detect_if_allowed(
    source_info: &Option<SourceAppInfo>,
    blacklist: &[String],
    startup_done: bool,
    detector: impl FnOnce() -> DetectionResult,
) -> DetectionResult {
    let source_app_name = source_info
        .as_ref()
        .map(|s| s.app_name.as_str())
        .unwrap_or("");
    if capture_gate(source_app_name, blacklist, startup_done) {
        detector()
    } else {
        DetectionResult::None
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
) -> DetectionResult {
    if let Some(item) = detect_files(ctx, source_info) {
        return DetectionResult::Item(Box::new(item));
    }

    // When both image and rich text (HTML/RTF) coexist on the clipboard,
    // prefer the text. Apps like OneNote and Excel put both a rendered
    // image and formatted text on the clipboard simultaneously; users
    // expect to see the text content, not the image rendering.
    if clipboard_has_image(ctx) && (ctx.has(ContentFormat::Html) || ctx.has(ContentFormat::Rtf)) {
        if let Some(item) = detect_text_content(ctx, source_info) {
            return DetectionResult::Item(Box::new(item));
        }
        if let Some(image) = detect_image(ctx, source_info) {
            return DetectionResult::Image(image);
        }
    } else if let Some(image) = detect_image(ctx, source_info) {
        return DetectionResult::Image(image);
    }

    if let Some(item) = detect_text_content(ctx, source_info) {
        return DetectionResult::Item(Box::new(item));
    }

    DetectionResult::None
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
                let entries_with_remote: Vec<(FileInfo, Option<String>)> = files
                    .iter()
                    .map(|path| {
                        let p = std::path::Path::new(path);
                        let remote_host =
                            crate::platform::remote_path::remote_host_label(path.as_str());
                        (
                            FileInfo {
                                name: p
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| path.clone()),
                                path: path.clone(),
                                // Never query a remote source merely to classify it.
                                // Remote folders remain usable as file references.
                                is_dir: remote_host.is_none() && p.is_dir(),
                            },
                            remote_host,
                        )
                    })
                    .collect();
                let entries: Vec<FileInfo> = entries_with_remote
                    .iter()
                    .map(|(entry, _)| entry.clone())
                    .collect();
                let first_remote_host = entries_with_remote
                    .iter()
                    .find_map(|(_, host)| host.as_ref())
                    .cloned();
                let common_remote_host = first_remote_host.map(|first| {
                    if entries_with_remote
                        .iter()
                        .all(|(_, host)| host.as_deref() == Some(first.as_str()))
                    {
                        first
                    } else {
                        // Mixed local/remote or multi-host selections still need
                        // the remote marker so later previews perform no I/O.
                        "NAS".to_string()
                    }
                });

                // Remote resources intentionally keep an unknown size. Querying
                // NAS metadata here can stall the clipboard listener.
                let total_size: i64 = entries_with_remote
                    .iter()
                    .filter(|(_, host)| host.is_none())
                    .filter_map(|(entry, _)| std::fs::metadata(&entry.path).ok())
                    .map(|m| m.len())
                    .sum::<u64>() as i64;

                // Single image file → treat as Image type for thumbnail preview
                if entries.len() == 1 && !entries[0].is_dir && is_image_extension(&entries[0].path)
                {
                    let path = entries[0].path.clone();
                    let remote_host = entries_with_remote[0].1.clone();
                    let is_remote = remote_host.is_some();
                    let hash = hash_image_file_reference(&path, !is_remote);
                    let (iw, ih) = if is_remote {
                        (0, 0)
                    } else {
                        image::image_dimensions(&path).unwrap_or((0, 0))
                    };
                    if !is_remote {
                        ensure_thumbnail_for_image(&path, hash);
                    }

                    mark_recent_image_file_reference();
                    let mut item =
                        ClipboardItem::new_image(0, &path, hash, iw, ih, source_info.as_ref());
                    // Capture-time existence evidence: the screenshot tool's
                    // temp file was observed existing right now. This is the
                    // basis for later stale cleanup (design §9.2 / Phase 0).
                    if !is_remote && std::path::Path::new(&path).exists() {
                        item.existence_observed_at = Utc::now().to_rfc3339();
                    }
                    if remote_host.is_some() {
                        item.rich_data = RichData {
                            remote_host,
                            ..Default::default()
                        }
                        .to_json();
                    }
                    return Some(item);
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

                let mut item =
                    ClipboardItem::new_file(0, &file_data, hash, source_info.as_ref(), total_size);
                // Capture-time existence evidence: every local path in the
                // list was observed existing (remote entries are never
                // probed — those items are protected anyway).
                let all_local_paths_exist =
                    entries_with_remote.iter().all(|(entry, remote_host)| {
                        remote_host.is_some() || std::path::Path::new(&entry.path).exists()
                    });
                if all_local_paths_exist {
                    item.existence_observed_at = Utc::now().to_rfc3339();
                }
                if common_remote_host.is_some() {
                    item.rich_data = RichData {
                        remote_host: common_remote_host,
                        ..Default::default()
                    }
                    .to_json();
                }
                return Some(item);
            }
        }
    }
    None
}

fn hash_image_file_reference(path: &str, probe_metadata: bool) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);

    if probe_metadata {
        let Ok(meta) = std::fs::metadata(path) else {
            return hasher.finish();
        };
        meta.len().hash(&mut hasher);
        if let Ok(modified) = meta.modified() {
            if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
                duration.as_nanos().hash(&mut hasher);
            }
        }
    }

    hasher.finish()
}

// ============================================================================
// --- Async image persistence (placeholder-first capture) ---
// 监听线程只拷贝原始字节并立即入条目；解码/重编码/落盘/缩略图在后台完成。
// ============================================================================

/// 原始剪贴板图片负载，交由后台线程落盘。
enum RawClipboardImage {
    /// 已是 PNG 字节，直接落盘。
    Png(Vec<u8>),
    /// 原始 CF_DIB / CF_DIBV5 内存块（不含 BITMAPFILEHEADER），后台解码。
    #[cfg(target_os = "windows")]
    Bitmap(Vec<u8>),
    /// macOS 原生 TIFF 负载，后台解码并转换为 PNG。
    #[cfg(target_os = "macos")]
    Tiff(Vec<u8>),
}

impl RawClipboardImage {
    fn len(&self) -> usize {
        match self {
            Self::Png(bytes) => bytes.len(),
            #[cfg(target_os = "windows")]
            Self::Bitmap(bytes) => bytes.len(),
            #[cfg(target_os = "macos")]
            Self::Tiff(bytes) => bytes.len(),
        }
    }
}

/// Background image persistence job. Raw payloads are memory-heavy, so the
/// worker queue is bounded by both item count and total bytes.
struct PersistJob {
    captured: CapturedImage,
    pending: Arc<Mutex<Vec<ClipboardItem>>>,
    pending_images: Arc<Mutex<Vec<PendingImageView>>>,
}

struct ImagePersistWorker {
    queue: Arc<(Mutex<VecDeque<PersistJob>>, Condvar)>,
}

static IMAGE_PERSIST_WORKER: LazyLock<ImagePersistWorker> = LazyLock::new(ImagePersistWorker::new);

impl ImagePersistWorker {
    fn new() -> Self {
        let queue = Arc::new((Mutex::new(VecDeque::<PersistJob>::new()), Condvar::new()));
        let worker_queue = queue.clone();
        // 单 worker 守护线程：进程生命周期内持续处理落盘任务。
        std::thread::spawn(move || loop {
            let job = {
                let (lock, condvar) = &*worker_queue;
                let mut jobs = lock.lock().unwrap_or_else(|e| e.into_inner());
                while jobs.is_empty() {
                    jobs = condvar.wait(jobs).unwrap_or_else(|e| e.into_inner());
                }
                jobs.pop_front()
            };
            let Some(job) = job else {
                continue;
            };
            let img_dir = images_dir();
            let result = persist_image(&job.captured, &img_dir);
            // Publish completion under the same pair of locks used by
            // poll_state(), so a snapshot cannot contain both placeholder and
            // ready item (or neither of them).
            let mut placeholders = job.pending_images.lock().unwrap_or_else(|e| e.into_inner());
            let mut ready = job.pending.lock().unwrap_or_else(|e| e.into_inner());
            placeholders.retain(|p| p.raw_hash != job.captured.raw_hash);
            if let Some(item) = result {
                ready.push(item);
            }
        });
        Self { queue }
    }

    fn enqueue(
        &self,
        captured: CapturedImage,
        pending: Arc<Mutex<Vec<ClipboardItem>>>,
        pending_images: Arc<Mutex<Vec<PendingImageView>>>,
    ) {
        let (lock, condvar) = &*self.queue;
        let mut jobs = lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = jobs
            .iter_mut()
            .find(|job| job.captured.raw_hash == captured.raw_hash)
        {
            existing.captured = captured;
            existing.pending = pending;
            existing.pending_images = pending_images;
            return;
        }

        let incoming_bytes = captured.raw.len();
        let mut queued_bytes = jobs.iter().map(|job| job.captured.raw.len()).sum::<usize>();
        let mut dropped = Vec::new();
        while persist_queue_over_budget(jobs.len(), queued_bytes, incoming_bytes) {
            let Some(oldest) = jobs.pop_front() else {
                break;
            };
            queued_bytes = queued_bytes.saturating_sub(oldest.captured.raw.len());
            dropped.push(oldest);
        }
        jobs.push_back(PersistJob {
            captured,
            pending,
            pending_images,
        });
        condvar.notify_one();
        drop(jobs);

        for dropped_job in dropped {
            log::warn!(
                "image persistence queue is full; dropped oldest pending image {:016x}",
                dropped_job.captured.raw_hash
            );
            dropped_job
                .pending_images
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|view| view.raw_hash != dropped_job.captured.raw_hash);
        }
    }
}

fn persist_queue_over_budget(
    queued_jobs: usize,
    queued_bytes: usize,
    incoming_bytes: usize,
) -> bool {
    queued_jobs >= MAX_IMAGE_PERSIST_JOBS
        || queued_bytes.saturating_add(incoming_bytes) > MAX_IMAGE_PERSIST_BYTES
}

/// 对原始载荷计算内容哈希。
fn hash_raw_image(raw: &RawClipboardImage) -> u64 {
    let bytes = match raw {
        RawClipboardImage::Png(bytes) => bytes,
        #[cfg(target_os = "windows")]
        RawClipboardImage::Bitmap(bytes) => bytes,
        #[cfg(target_os = "macos")]
        RawClipboardImage::Tiff(bytes) => bytes,
    };
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

/// 从 DIB 内存块头解析尺寸（纯函数）。
#[cfg(any(target_os = "windows", test))]
fn parse_dib_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let bi_size = u32::from_le_bytes(data.get(0..4)?.try_into().ok()?);
    match bi_size {
        12 => {
            let w = u16::from_le_bytes(data.get(4..6)?.try_into().ok()?) as u32;
            let h = u16::from_le_bytes(data.get(6..8)?.try_into().ok()?) as u32;
            (w > 0 && h > 0).then_some((w, h))
        }
        n if n >= 40 => {
            let w = i32::from_le_bytes(data.get(4..8)?.try_into().ok()?);
            let h = i32::from_le_bytes(data.get(8..12)?.try_into().ok()?);
            let (w, h) = (w.unsigned_abs(), h.unsigned_abs());
            (w > 0 && h > 0).then_some((w, h))
        }
        _ => None,
    }
}

fn image_exceeds_capture_limit(width: u32, height: u32) -> bool {
    width == 0
        || height == 0
        || width > MAX_CAPTURED_IMAGE_DIMENSION
        || height > MAX_CAPTURED_IMAGE_DIMENSION
        || u64::from(width)
            .checked_mul(u64::from(height))
            .is_none_or(|pixels| pixels > MAX_CAPTURED_IMAGE_PIXELS)
}

#[cfg(any(target_os = "windows", test))]
fn select_image_format(png_id: u32, png: bool, dib: bool, dib_v5: bool) -> Option<u32> {
    if png {
        Some(png_id)
    } else if dib {
        Some(8)
    } else if dib_v5 {
        Some(17)
    } else {
        None
    }
}

#[cfg(target_os = "windows")]
fn selected_windows_image_format() -> Option<u32> {
    let png_id = png_format_id();
    select_image_format(
        png_id,
        png_format_available(),
        dib_format_available(8),
        dib_format_available(17),
    )
}

fn clipboard_has_image(ctx: &ClipboardContext) -> bool {
    #[cfg(target_os = "windows")]
    {
        let _ = ctx;
        selected_windows_image_format().is_some()
    }
    #[cfg(not(target_os = "windows"))]
    {
        ctx.has(ContentFormat::Image)
    }
}

/// 检查 Windows 剪贴板是否提供指定格式（不触发渲染）。
#[cfg(target_os = "windows")]
fn dib_format_available(format: u32) -> bool {
    unsafe { windows_sys::Win32::System::DataExchange::IsClipboardFormatAvailable(format) != 0 }
}

/// 一次性复制指定剪贴板格式的原始字节（OpenClipboard -> GetClipboardData 一次
/// -> GlobalSize/GlobalLock -> memcpy -> GlobalUnlock -> CloseClipboard）。
/// 同时用于 PNG 与 DIB/DIBV5；调用方必须已选定唯一格式，且读取失败不回退其他格式。
#[cfg(target_os = "windows")]
fn copy_clipboard_format_once(format: u32) -> Option<Vec<u8>> {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, OpenClipboard,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

    let mut opened = false;
    for _ in 0..10 {
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            opened = true;
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    if !opened {
        return None;
    }

    let result = (|| {
        let handle = unsafe { GetClipboardData(format) };
        if handle.is_null() {
            return None;
        }
        let size = unsafe { GlobalSize(handle) };
        if size == 0 || size > MAX_CAPTURED_IMAGE_BYTES {
            if size > MAX_CAPTURED_IMAGE_BYTES {
                log::warn!(
                    "clipboard image payload is too large: {size} bytes (limit {MAX_CAPTURED_IMAGE_BYTES})"
                );
            }
            return None;
        }
        let ptr = unsafe { GlobalLock(handle) };
        if ptr.is_null() {
            return None;
        }
        let mut out = vec![0u8; size];
        unsafe {
            std::ptr::copy_nonoverlapping(ptr as *const u8, out.as_mut_ptr(), size);
            GlobalUnlock(handle);
        }
        Some(out)
    })();

    unsafe {
        CloseClipboard();
    }
    result
}

/// 解码原始 DIB 块（不含 BITMAPFILEHEADER）。
#[cfg(target_os = "windows")]
fn decode_dib(bytes: &[u8]) -> Option<image::DynamicImage> {
    use image::ImageDecoder;
    use std::io::Cursor;
    let mut decoder =
        image::codecs::bmp::BmpDecoder::new_without_file_header(Cursor::new(bytes)).ok()?;
    if decoder.total_bytes() > MAX_DECODED_IMAGE_BYTES {
        return None;
    }
    decoder.set_limits(capture_decode_limits()).ok()?;
    image::DynamicImage::from_decoder(decoder).ok()
}

/// 后台落盘图片并生成缩略图。
fn enqueue_image_persist(
    captured: CapturedImage,
    pending: Arc<Mutex<Vec<ClipboardItem>>>,
    pending_images: Arc<Mutex<Vec<PendingImageView>>>,
) {
    IMAGE_PERSIST_WORKER.enqueue(captured, pending, pending_images);
}

fn capture_decode_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_CAPTURED_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_CAPTURED_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED_IMAGE_BYTES);
    limits
}

fn decode_encoded_image(bytes: &[u8], format: image::ImageFormat) -> Option<image::DynamicImage> {
    use image::ImageDecoder;
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    reader.limits(capture_decode_limits());
    let result = reader.into_decoder().and_then(|decoder| {
        let (width, height) = decoder.dimensions();
        if image_exceeds_capture_limit(width, height)
            || decoder.total_bytes() > MAX_DECODED_IMAGE_BYTES
        {
            return Err(image::ImageError::Limits(
                image::error::LimitError::from_kind(
                    image::error::LimitErrorKind::InsufficientMemory,
                ),
            ));
        }
        // TIFF needs a separate decoding buffer as well as the output buffer.
        // ImageReader::decode subtracts the output from max_alloc first,
        // unintentionally rejecting supported 80 MP TIFFs. Bound both buffers
        // separately: at most 512 MiB output plus 512 MiB decoder workspace.
        image::DynamicImage::from_decoder(decoder)
    });
    match result {
        Ok(image) => Some(image),
        Err(error) => {
            log::warn!("clipboard image decoding failed: {error}");
            None
        }
    }
}

/// 将原始图片负载写入缓存并生成缩略图，成功后构造完整 ClipboardItem。
/// 返回 None 表示解码/落盘失败，不发布条目。
fn persist_image(captured: &CapturedImage, img_dir: &std::path::Path) -> Option<ClipboardItem> {
    if image_exceeds_capture_limit(captured.width, captured.height) {
        return None;
    }
    let file_path = img_dir.join(format!("{:016x}.png", captured.raw_hash));
    let file_path_str = file_path.to_string_lossy().to_string();

    if let Ok(existing) = image::open(&file_path) {
        if existing.width() == captured.width && existing.height() == captured.height {
            write_thumbnail(&existing, img_dir, captured.raw_hash);
            return build_ready_image_item(captured, &file_path_str);
        }
        log::warn!(
            "persist_image: cached image dimensions do not match capture; rebuilding {}",
            file_path.display()
        );
    }
    if file_path.exists() {
        let _ = std::fs::remove_file(&file_path);
    }

    let decoded = match &captured.raw {
        RawClipboardImage::Png(bytes) => decode_encoded_image(bytes, image::ImageFormat::Png),
        #[cfg(target_os = "windows")]
        RawClipboardImage::Bitmap(bytes) => decode_dib(bytes),
        #[cfg(target_os = "macos")]
        RawClipboardImage::Tiff(bytes) => decode_encoded_image(bytes, image::ImageFormat::Tiff),
    }?;
    if decoded.width() != captured.width || decoded.height() != captured.height {
        log::warn!(
            "persist_image: decoded dimensions {}x{} do not match captured {}x{}",
            decoded.width(),
            decoded.height(),
            captured.width,
            captured.height
        );
        return None;
    }

    if let Err(error) = std::fs::create_dir_all(img_dir) {
        log::warn!(
            "persist_image: failed to create image cache directory {}: {error}",
            img_dir.display()
        );
        return None;
    }

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temp_path = img_dir.join(format!(".{:016x}.{unique}.tmp.png", captured.raw_hash));
    let write_result: Result<(), String> = match &captured.raw {
        RawClipboardImage::Png(bytes) => {
            std::fs::write(&temp_path, bytes).map_err(|error| error.to_string())
        }
        #[cfg(target_os = "windows")]
        RawClipboardImage::Bitmap(_) => decoded
            .save_with_format(&temp_path, image::ImageFormat::Png)
            .map_err(|error| error.to_string()),
        #[cfg(target_os = "macos")]
        RawClipboardImage::Tiff(_) => decoded
            .save_with_format(&temp_path, image::ImageFormat::Png)
            .map_err(|error| error.to_string()),
    };
    if let Err(error) = write_result {
        log::warn!(
            "persist_image: failed to write temporary PNG {}: {error}",
            temp_path.display()
        );
        let _ = std::fs::remove_file(&temp_path);
        return None;
    }

    let temp_valid = image::image_dimensions(&temp_path)
        .is_ok_and(|dimensions| dimensions == (captured.width, captured.height));
    if !temp_valid {
        log::warn!(
            "persist_image: temporary PNG validation failed {}",
            temp_path.display()
        );
        let _ = std::fs::remove_file(&temp_path);
        return None;
    }
    if let Err(error) = std::fs::rename(&temp_path, &file_path) {
        log::warn!(
            "persist_image: failed to commit PNG {}: {error}",
            file_path.display()
        );
        let _ = std::fs::remove_file(&temp_path);
        return None;
    }

    write_thumbnail(&decoded, img_dir, captured.raw_hash);
    build_ready_image_item(captured, &file_path_str)
}

fn build_ready_image_item(captured: &CapturedImage, file_path: &str) -> Option<ClipboardItem> {
    if !std::path::Path::new(file_path).is_file() {
        return None;
    }
    let mut item = ClipboardItem::new_image(
        0,
        file_path,
        captured.raw_hash,
        captured.width,
        captured.height,
        captured.source.as_ref(),
    );
    item.existence_observed_at = Utc::now().to_rfc3339();
    Some(item)
}

fn detect_image(
    ctx: &ClipboardContext,
    source_info: &Option<SourceAppInfo>,
) -> Option<CapturedImage> {
    if !clipboard_has_image(ctx) {
        return None;
    }
    let (raw, width, height) = read_clipboard_image_png(ctx)?;
    let raw_hash = hash_raw_image(&raw);
    Some(CapturedImage {
        raw,
        width,
        height,
        raw_hash,
        source: source_info.clone(),
    })
}

fn detect_text_content(
    ctx: &ClipboardContext,
    source_info: &Option<SourceAppInfo>,
) -> Option<ClipboardItem> {
    detect_text_content_with_readers(
        ctx.has(ContentFormat::Text),
        || ctx.get_text().ok(),
        || read_clipboard_rich_data(ctx),
        source_info,
    )
}

fn detect_text_content_with_readers(
    has_text: bool,
    read_text: impl FnOnce() -> Option<String>,
    read_rich: impl FnOnce() -> Option<RichData>,
    source_info: &Option<SourceAppInfo>,
) -> Option<ClipboardItem> {
    // Do not open the clipboard when there is no text. `get_text()` would
    // call `OpenClipboard`, which can race with a source app that is still
    // writing a large clipboard payload (e.g. FastStone's non-atomic
    // EmptyClipboard -> encode -> SetClipboardData sequence). Checking format
    // availability first is a non-blocking `IsClipboardFormatAvailable` call
    // that never opens the clipboard.
    let text = has_text.then(read_text).flatten().filter(|s| !s.is_empty());
    // HTML is an independent representation: a missing or unreadable plain
    // text flavor must not discard a spreadsheet's HTML in favor of its bitmap.
    // The rich reader checks format availability before opening the clipboard.
    let rich_data = read_rich();
    let text = text.or_else(|| {
        rich_data
            .as_ref()?
            .html
            .as_deref()
            .map(crate::core::html_text::visible_text)
            .filter(|text| !text.is_empty())
    });
    if let Some(text) = text {
        if is_url(&text) {
            // --- Prefetch favicon in background thread (non-critical) ---
            if let Some(host) = crate::core::secret::url_clean_host(&text) {
                let _ = favicon::ensure_favicon_cached(&host);
            }
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

        // --- Secret detection (password, API key, token, private key) ---
        if crate::core::secret::detect_secret(&text).is_some() {
            let mut item = ClipboardItem::new_text(
                0,
                &text,
                ContentType::PlainText,
                source_info.as_ref(),
                rich_data.as_ref(),
            );
            item.meta_type = "secret".to_string();
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
            // Capture-time existence evidence for text path references
            // (design §5.2 / §9.4): without this, the item can never be
            // auto-deleted. Remote and foreign paths are never probed.
            if crate::platform::remote_path::remote_host_label(&text).is_none()
                && std::path::Path::new(&text).exists()
            {
                item.existence_observed_at = Utc::now().to_rfc3339();
            }
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

    #[cfg(target_os = "windows")]
    let html = ctx
        .get_buffer("HTML Format")
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .map(|html| crate::core::html_text::preserve_clipboard_html_document(&html))
        .or_else(|| ctx.get_html().ok());
    #[cfg(not(target_os = "windows"))]
    let html = ctx
        .get_html()
        .ok()
        .map(|html| crate::core::html_text::preserve_clipboard_html_document(&html));
    let rtf = ctx.get_rich_text().ok();
    (html.is_some() || rtf.is_some()).then_some(RichData {
        html,
        rtf,
        ..Default::default()
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
fn read_clipboard_image_png(ctx: &ClipboardContext) -> Option<(RawClipboardImage, u32, u32)> {
    if has_recent_image_file_reference() {
        return None;
    }

    // Windows: choose one format (PNG > DIB > DIBV5), then read it once.
    #[cfg(target_os = "windows")]
    {
        let _ = ctx;
        let png = png_format_id();
        let format = selected_windows_image_format()?;
        let bytes = copy_clipboard_format_once(format)?;
        let (w, h) = if format == png {
            png_dimensions(&bytes)?
        } else {
            parse_dib_dimensions(&bytes)?
        };
        if image_exceeds_capture_limit(w, h) {
            log::warn!("skipped oversized clipboard image {w}x{h}");
            return None;
        }
        if format == png {
            Some((RawClipboardImage::Png(bytes), w, h))
        } else {
            Some((RawClipboardImage::Bitmap(bytes), w, h))
        }
    }

    // macOS: copy the native encoded payload only. Full TIFF decoding and PNG
    // conversion happen in ImagePersistWorker, so the listener can publish a
    // placeholder and resume polling without synchronously re-encoding pixels.
    #[cfg(target_os = "macos")]
    {
        use std::io::Cursor;

        let (raw, w, h) = if let Ok(bytes) = ctx.get_buffer("public.png") {
            if bytes.is_empty() || bytes.len() > MAX_CAPTURED_IMAGE_BYTES {
                return None;
            }
            let (w, h) = png_dimensions(&bytes)?;
            (RawClipboardImage::Png(bytes), w, h)
        } else {
            let bytes = ctx.get_buffer("public.tiff").ok()?;
            if bytes.is_empty() || bytes.len() > MAX_CAPTURED_IMAGE_BYTES {
                return None;
            }
            // `into_dimensions` reads image metadata without allocating or
            // decoding the full pixel buffer.
            let (w, h) = image::ImageReader::new(Cursor::new(&bytes))
                .with_guessed_format()
                .ok()?
                .into_dimensions()
                .ok()?;
            (RawClipboardImage::Tiff(bytes), w, h)
        };
        if image_exceeds_capture_limit(w, h) {
            log::warn!("skipped oversized clipboard image {w}x{h}");
            return None;
        }
        Some((raw, w, h))
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let img = ctx.get_image().ok()?;
        if img.is_empty() {
            return None;
        }
        let (w, h) = img.get_size();
        if image_exceeds_capture_limit(w, h) {
            return None;
        }
        let png_bytes = img.to_png().ok()?;
        Some((RawClipboardImage::Png(png_bytes.get_bytes().to_vec()), w, h))
    }
}

/// Target thumbnail width matching the card content area logical-pixel width.
const THUMB_WIDTH: u32 = 310;
const THUMB_MAX_HEIGHT: u32 = 8192;

fn thumbnail_dimensions(width: u32, height: u32) -> (u32, u32) {
    let scale = (THUMB_WIDTH as f64 / width.max(1) as f64)
        .min(THUMB_MAX_HEIGHT as f64 / height.max(1) as f64)
        .min(1.0);
    (
        ((width as f64 * scale) as u32).max(1),
        ((height as f64 * scale) as u32).max(1),
    )
}

fn thumbnail_file_is_valid(path: &std::path::Path) -> bool {
    let mut header = [0_u8; 24];
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    file.read_exact(&mut header).is_ok()
        && png_dimensions(&header).is_some_and(|(width, height)| {
            width > 0 && height > 0 && width <= THUMB_WIDTH && height <= THUMB_MAX_HEIGHT
        })
}

/// Generate a thumbnail by scaling the full image to match the card width,
/// preserving aspect ratio. Small images (≤ target width) are kept as-is.
fn generate_thumbnail(image_path: &std::path::Path, img_dir: &std::path::Path, hash: u64) {
    if !image_path.exists() {
        return;
    }
    let Ok(img) = image::open(image_path) else {
        return;
    };
    write_thumbnail(&img, img_dir, hash);
}

/// Write the thumbnail from an already-decoded image. Reused by the DIB persist
/// path so the bitmap is decoded exactly once (no `image::open` round-trip on
/// the just-written full PNG).
fn write_thumbnail(img: &image::DynamicImage, img_dir: &std::path::Path, hash: u64) {
    use image::GenericImageView;
    let thumb_path = img_dir.join(format!("thumb_{:016x}.png", hash));
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
    let (w, h) = img.dimensions();
    let (thumb_w, thumb_h) = thumbnail_dimensions(w, h);
    let thumb = if (w, h) == (thumb_w, thumb_h) {
        img.clone()
    } else {
        img.resize_exact(thumb_w, thumb_h, image::imageops::FilterType::Lanczos3)
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

fn should_process_clipboard_sequence<T: Eq>(startup_done: bool, current: &T, previous: &T) -> bool {
    startup_done && current != previous
}

#[cfg(target_os = "macos")]
const MACOS_PASTEBOARD_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(200),
];

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct MacosPasteboardRetry {
    change_count: isize,
    failed_attempts: usize,
    retry_after: Instant,
}

#[cfg(target_os = "macos")]
fn record_macos_pasteboard_failure(
    retry: &mut Option<MacosPasteboardRetry>,
    change_count: isize,
    now: Instant,
) -> bool {
    let failed_attempts = retry
        .filter(|state| state.change_count == change_count)
        .map_or(1, |state| state.failed_attempts + 1);
    let Some(delay) = MACOS_PASTEBOARD_RETRY_DELAYS.get(failed_attempts - 1) else {
        *retry = None;
        return false;
    };
    *retry = Some(MacosPasteboardRetry {
        change_count,
        failed_attempts,
        retry_after: now + *delay,
    });
    true
}

/// Decide whether a newly captured item/image duplicates the previous capture
/// (delayed rendering, owner destruction, or rapid same-content re-copy).
/// Returns `true` when the capture should be skipped; otherwise records the
/// new state in `last_state` and returns `false`.
fn suppress_duplicate_and_update(last_state: &Mutex<LastClipState>, hash: u64) -> bool {
    let now = Instant::now();
    let owner = current_clipboard_owner();
    let seq = current_clipboard_seq();
    let mut guard = last_state.lock().unwrap_or_else(|e| e.into_inner());

    #[cfg(target_os = "windows")]
    let suppress = {
        let same_hash = guard.hash == hash;
        let delayed_fill = same_hash && seq.wrapping_sub(guard.seq) <= 2;
        let owner_destroyed = same_hash && owner == 0 && guard.owner != 0;
        let rapid_same = same_hash
            && owner == guard.owner
            && seq.wrapping_sub(guard.seq) > 2
            && now.duration_since(guard.time).as_millis() < 2_000;
        delayed_fill || owner_destroyed || rapid_same
    };
    #[cfg(not(target_os = "windows"))]
    let suppress = {
        let _ = (&guard.hash, &guard.owner, &guard.time, &guard.seq);
        false
    };

    if !suppress {
        *guard = LastClipState {
            hash,
            owner,
            time: now,
            seq,
        };
    }
    suppress
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
        let pending_images = shared.pending_images.clone();
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
        #[cfg(target_os = "macos")]
        let macos_retry = Arc::new(Mutex::new(None::<MacosPasteboardRetry>));

        self.handle = Some(thread::spawn(move || {
            // One poll cycle, extracted so the macOS branch can run inside an
            // autorelease pool: NSPasteboard/NSImage/NSWorkspace calls on this
            // self-managed thread can otherwise leave autoreleased Objective-C
            // temporaries that only accumulate during long idle periods.
            // Early returns mirror the old `continue` (each already slept).
            let cycle = || {
                // --- Skip recording during batch paste to avoid redundant entries ---
                if batch_pasting.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(50));
                    return;
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
                        *macos_retry.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    }
                    thread::sleep(Duration::from_millis(50));
                    return;
                }

                // Do not consume a real clipboard change during startup. The
                // platform sequence baseline remains unchanged, so a copy made
                // during the grace period is processed on the first later poll.
                let startup_done = startup_end
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_none_or(|end| end.elapsed().as_millis() > 500);
                // Fast-path: if the clipboard sequence number hasn't changed,
                // --- skip the expensive clipboard open + image encoding entirely. ---
                #[cfg(target_os = "windows")]
                {
                    let seq = unsafe {
                        windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber()
                    };
                    let mut last = last_seq.lock().unwrap_or_else(|e| e.into_inner());
                    if !should_process_clipboard_sequence(startup_done, &seq, &*last) {
                        drop(last);
                        thread::sleep(Duration::from_millis(50));
                        return;
                    }
                    *last = seq;
                }

                // --- macOS equivalent: NSPasteboard.changeCount increments on every ---
                // pasteboard write, serving the same role as the Windows sequence number.
                #[cfg(target_os = "macos")]
                let observed_cc = {
                    let cc =
                        with_clipboard_access(|| NSPasteboard::generalPasteboard().changeCount());
                    let last = last_cc.lock().unwrap_or_else(|e| e.into_inner());
                    if !should_process_clipboard_sequence(startup_done, &cc, &*last) {
                        drop(last);
                        thread::sleep(Duration::from_millis(50));
                        return;
                    }
                    drop(last);
                    let retry = macos_retry.lock().unwrap_or_else(|e| e.into_inner());
                    if retry.is_some_and(|state| {
                        state.change_count == cc && Instant::now() < state.retry_after
                    }) {
                        drop(retry);
                        thread::sleep(Duration::from_millis(25));
                        return;
                    }
                    cc
                };

                #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                if !startup_done {
                    thread::sleep(Duration::from_millis(50));
                    return;
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
                #[cfg(target_os = "macos")]
                let capture_allowed = capture_gate(
                    source_info
                        .as_ref()
                        .map(|source| source.app_name.as_str())
                        .unwrap_or(""),
                    &blacklist_snapshot,
                    true,
                );

                // ── Unified gate + content detection ──
                let detection = detect_if_allowed(
                    &source_info,
                    &blacklist_snapshot,
                    true, // startup_done already checked above
                    || {
                        with_clipboard_context(|ctx| detect_clipboard_content(ctx, &source_info))
                            .unwrap_or(DetectionResult::None)
                    },
                );

                // `clearContents()` increments AppKit's change count before a
                // writer necessarily supplies its representations. Commit the
                // baseline after a readable result, or after bounded retries
                // when the new content is genuinely unsupported.
                #[cfg(target_os = "macos")]
                {
                    // A blacklist rejection is intentional, not a transient
                    // empty pasteboard, and must consume the baseline at once.
                    let readable = !capture_allowed || !matches!(&detection, DetectionResult::None);
                    let mut retry = macos_retry.lock().unwrap_or_else(|e| e.into_inner());
                    let should_retry = !readable
                        && record_macos_pasteboard_failure(&mut retry, observed_cc, Instant::now());
                    if readable || !should_retry {
                        *last_cc.lock().unwrap_or_else(|e| e.into_inner()) = observed_cc;
                        *retry = None;
                    }
                }

                match detection {
                    DetectionResult::Item(item) => {
                        if !suppress_duplicate_and_update(&last_state, item.content_hash) {
                            pending
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push(*item);
                        }
                    }
                    DetectionResult::Image(captured) => {
                        if !suppress_duplicate_and_update(&last_state, captured.raw_hash) {
                            // 注册纯内存占位：UI 立即显示「处理中」，缩略图就绪后预览提前出现。
                            {
                                let placeholder = PendingImageView {
                                    raw_hash: captured.raw_hash,
                                    width: captured.width,
                                    height: captured.height,
                                    source_name: captured
                                        .source
                                        .as_ref()
                                        .map(|s| s.app_name.clone())
                                        .unwrap_or_default(),
                                    source_icon: captured
                                        .source
                                        .as_ref()
                                        .map(|s| s.icon_base64.clone())
                                        .unwrap_or_default(),
                                    started_at: Instant::now(),
                                };
                                let mut views =
                                    pending_images.lock().unwrap_or_else(|e| e.into_inner());
                                if let Some(existing) = views
                                    .iter_mut()
                                    .find(|view| view.raw_hash == placeholder.raw_hash)
                                {
                                    *existing = placeholder;
                                } else {
                                    views.push(placeholder);
                                }
                            }
                            enqueue_image_persist(
                                captured,
                                pending.clone(),
                                pending_images.clone(),
                            );
                        }
                    }
                    DetectionResult::None => {}
                }

                thread::sleep(Duration::from_millis(50));
            };

            while running.load(Ordering::SeqCst) {
                #[cfg(target_os = "macos")]
                objc2::rc::autoreleasepool(|_| cycle());
                #[cfg(not(target_os = "macos"))]
                cycle();
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
mod rich_capture_tests {
    use super::*;
    use crate::core::types::DisplayKind;

    fn spreadsheet_rich_data() -> RichData {
        let html = "<html><head><style>.et2{color:#ff6600}</style></head><body><!--StartFragment--><table><tr><td class=et2>测试文本</td><td>第二格</td></tr></table><!--EndFragment--></body></html>";
        RichData {
            html: Some(crate::core::html_text::preserve_clipboard_html_document(
                &crate::core::html_text::encode_cf_html(html),
            )),
            ..Default::default()
        }
    }

    #[test]
    fn spreadsheet_html_is_captured_without_plain_text_format() {
        let rich = spreadsheet_rich_data();
        let item = detect_text_content_with_readers(
            false,
            || panic!("an unavailable text format must not be read"),
            || Some(rich.clone()),
            &None,
        )
        .expect("HTML must be captured before falling back to a spreadsheet bitmap");
        assert_eq!(item.display_kind(), DisplayKind::Html);
        assert!(item.full_text.contains("测试文本"));
        assert!(item.full_text.contains("第二格"));
        assert!(!item.full_text.contains("color"));
        assert_eq!(RichData::from_json(&item.rich_data).html, rich.html);
    }

    #[test]
    fn spreadsheet_html_survives_empty_or_failed_plain_text_reads() {
        for text in [None, Some(String::new())] {
            let item = detect_text_content_with_readers(
                true,
                || text,
                || Some(spreadsheet_rich_data()),
                &None,
            )
            .expect("plain text failure must not discard readable HTML");
            assert_eq!(item.display_kind(), DisplayKind::Html);
            assert!(item.full_text.contains("测试文本"));
        }
    }

    #[test]
    fn spreadsheet_native_text_and_html_styles_are_preserved_together() {
        let text = "测试文本\t第二格\r\n";
        let rich = spreadsheet_rich_data();
        let item = detect_text_content_with_readers(
            true,
            || Some(text.into()),
            || Some(rich.clone()),
            &None,
        )
        .unwrap();
        assert_eq!(item.full_text, text);
        assert_eq!(item.display_kind(), DisplayKind::Html);
        assert_eq!(RichData::from_json(&item.rich_data).html, rich.html);
    }

    #[test]
    fn absent_or_image_only_html_still_allows_image_capture() {
        for rich in [
            None,
            Some(RichData {
                html: Some("<html><body><img src='preview.png'></body></html>".into()),
                ..Default::default()
            }),
        ] {
            let item = detect_text_content_with_readers(
                false,
                || panic!("image-only clipboard must not read an unavailable text format"),
                || rich,
                &None,
            );
            assert!(item.is_none());
        }
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
            DetectionResult::None
        });
        assert!(matches!(result, DetectionResult::None));
        assert_eq!(calls, 0);
    }

    #[test]
    fn detector_not_called_for_blacklisted() {
        let info = dummy_source("KeePass");
        let mut calls = 0u32;
        let result = detect_if_allowed(&info, &["KeePass".into()], true, || {
            calls += 1;
            DetectionResult::None
        });
        assert!(matches!(result, DetectionResult::None));
        assert_eq!(calls, 0);
    }

    #[test]
    fn detector_called_for_normal() {
        let info = dummy_source("Notepad");
        let mut calls = 0u32;
        let result = detect_if_allowed(&info, &[], true, || {
            calls += 1;
            DetectionResult::Item(Box::new(dummy_item()))
        });
        assert!(matches!(result, DetectionResult::Item(_)));
        assert_eq!(calls, 1);
    }

    #[test]
    fn detector_called_for_unknown_source() {
        let mut calls = 0u32;
        let result = detect_if_allowed(&None, &[], true, || {
            calls += 1;
            DetectionResult::Item(Box::new(dummy_item()))
        });
        assert!(matches!(result, DetectionResult::Item(_)));
        assert_eq!(calls, 1);
    }

    #[test]
    fn gate_uses_latest_blacklist_argument() {
        let info = dummy_source("Notepad");
        let mut calls = 0u32;
        // Empty blacklist → allowed
        let r1 = detect_if_allowed(&info, &[], true, || {
            calls += 1;
            DetectionResult::Item(Box::new(dummy_item()))
        });
        assert!(matches!(r1, DetectionResult::Item(_)));
        assert_eq!(calls, 1);
        // Now blacklisted → blocked, detector not called again
        let r2 = detect_if_allowed(&info, &["Notepad".into()], true, || {
            calls += 1;
            DetectionResult::None
        });
        assert!(matches!(r2, DetectionResult::None));
        assert_eq!(calls, 1);
    }

    #[test]
    fn startup_grace_defers_sequence_without_consuming_it() {
        let previous = 10_u32;
        let copied_during_startup = 11_u32;

        assert!(!should_process_clipboard_sequence(
            false,
            &copied_during_startup,
            &previous
        ));
        assert!(should_process_clipboard_sequence(
            true,
            &copied_during_startup,
            &previous
        ));
        assert!(!should_process_clipboard_sequence(
            true, &previous, &previous
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_empty_pasteboard_read_retries_with_bounded_backoff() {
        let now = Instant::now();
        let mut retry = None;

        assert!(record_macos_pasteboard_failure(&mut retry, 42, now));
        let first = retry.unwrap();
        assert_eq!(first.failed_attempts, 1);
        assert_eq!(
            first.retry_after.duration_since(now),
            Duration::from_millis(50)
        );

        assert!(record_macos_pasteboard_failure(&mut retry, 42, now));
        assert_eq!(retry.unwrap().failed_attempts, 2);
        assert!(record_macos_pasteboard_failure(&mut retry, 42, now));
        assert_eq!(retry.unwrap().failed_attempts, 3);

        assert!(!record_macos_pasteboard_failure(&mut retry, 42, now));
        assert!(retry.is_none(), "retry budget must be exhausted");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_new_change_count_resets_retry_budget() {
        let now = Instant::now();
        let mut retry = None;
        assert!(record_macos_pasteboard_failure(&mut retry, 42, now));
        assert!(record_macos_pasteboard_failure(&mut retry, 42, now));
        assert!(record_macos_pasteboard_failure(&mut retry, 43, now));
        assert_eq!(retry.unwrap().failed_attempts, 1);
    }
}

#[cfg(test)]
mod image_capture_tests {
    use super::*;

    #[test]
    fn parse_dib_dimensions_reads_bitmap_core_header() {
        // biSize = 12：BITMAPCOREHEADER，宽高是 u16，位于偏移 4 / 6。
        let mut data = vec![0u8; 12];
        data[0..4].copy_from_slice(&12u32.to_le_bytes());
        data[4..6].copy_from_slice(&300u16.to_le_bytes());
        data[6..8].copy_from_slice(&200u16.to_le_bytes());
        assert_eq!(parse_dib_dimensions(&data), Some((300, 200)));
    }

    #[test]
    fn parse_dib_dimensions_reads_bitmap_info_header() {
        // biSize = 40：BITMAPINFOHEADER，宽高是 i32，位于偏移 4 / 8。
        let mut data = vec![0u8; 40];
        data[0..4].copy_from_slice(&40u32.to_le_bytes());
        data[4..8].copy_from_slice(&300i32.to_le_bytes());
        data[8..12].copy_from_slice(&200i32.to_le_bytes());
        assert_eq!(parse_dib_dimensions(&data), Some((300, 200)));
    }

    #[test]
    fn parse_dib_dimensions_handles_negative_height() {
        // 负高度（bottom-up）应取绝对值。
        let mut data = vec![0u8; 40];
        data[0..4].copy_from_slice(&40u32.to_le_bytes());
        data[4..8].copy_from_slice(&300i32.to_le_bytes());
        data[8..12].copy_from_slice(&(-100i32).to_le_bytes());
        assert_eq!(parse_dib_dimensions(&data), Some((300, 100)));
    }

    #[test]
    fn parse_dib_dimensions_handles_i32_min_height_without_overflow() {
        let mut data = vec![0u8; 40];
        data[0..4].copy_from_slice(&40u32.to_le_bytes());
        data[4..8].copy_from_slice(&300i32.to_le_bytes());
        data[8..12].copy_from_slice(&i32::MIN.to_le_bytes());
        assert_eq!(parse_dib_dimensions(&data), Some((300, 2_147_483_648)));
        assert!(image_exceeds_capture_limit(300, 2_147_483_648));
    }

    #[test]
    fn parse_dib_dimensions_rejects_invalid_inputs() {
        // 零宽度
        let mut data = vec![0u8; 40];
        data[0..4].copy_from_slice(&40u32.to_le_bytes());
        data[4..8].copy_from_slice(&0i32.to_le_bytes());
        data[8..12].copy_from_slice(&200i32.to_le_bytes());
        assert_eq!(parse_dib_dimensions(&data), None);

        // 截断
        assert_eq!(parse_dib_dimensions(&[0x28, 0, 0, 0]), None);

        // 非法 biSize
        let mut bad = vec![0u8; 40];
        bad[0..4].copy_from_slice(&20u32.to_le_bytes());
        assert_eq!(parse_dib_dimensions(&bad), None);
    }

    #[test]
    fn image_limits_cover_dimensions_pixels_and_boundaries() {
        assert!(!image_exceeds_capture_limit(1200, 26_500));
        assert!(!image_exceeds_capture_limit(2000, 40_000));
        assert!(!image_exceeds_capture_limit(100_000, 1));
        assert!(image_exceeds_capture_limit(100_001, 1));
        assert!(!image_exceeds_capture_limit(100_000, 1280));
        assert!(image_exceeds_capture_limit(100_000, 1281));
        assert!(image_exceeds_capture_limit(80_001, 8_000));
        assert!(image_exceeds_capture_limit(0, 100));
        assert!(image_exceeds_capture_limit(100, 0));
        assert!(!image_exceeds_capture_limit(10_001, 1));
        assert!(!image_exceeds_capture_limit(8_001, 8_000));
    }

    #[test]
    fn extreme_aspect_ratio_thumbnails_stay_nonzero_and_fit_gpu_limits() {
        assert_eq!(thumbnail_dimensions(100_000, 1), (310, 1));
        assert_eq!(thumbnail_dimensions(1, 100_000), (1, THUMB_MAX_HEIGHT));
        assert_eq!(thumbnail_dimensions(100, 100), (100, 100));
        assert_eq!(thumbnail_dimensions(2000, 40_000), (310, 6200));
    }

    #[test]
    #[ignore = "80 MP decode stress test; run explicitly before release"]
    fn long_screenshot_png_and_tiff_decode_with_bounded_memory() {
        for format in [image::ImageFormat::Png, image::ImageFormat::Tiff] {
            let image = image::DynamicImage::new_rgba8(2000, 40_000);
            let mut encoded = std::io::Cursor::new(Vec::new());
            image.write_to(&mut encoded, format).unwrap();
            drop(image);
            let decoded = decode_encoded_image(encoded.get_ref(), format)
                .expect("a supported long screenshot must decode in either native format");
            assert_eq!((decoded.width(), decoded.height()), (2000, 40_000));
        }
    }

    #[test]
    fn image_format_selection_includes_dib_v5_without_fallback_reads() {
        const PNG_ID: u32 = 49_152;
        assert_eq!(select_image_format(PNG_ID, true, true, true), Some(PNG_ID));
        assert_eq!(select_image_format(PNG_ID, false, true, true), Some(8));
        assert_eq!(select_image_format(PNG_ID, false, false, true), Some(17));
        assert_eq!(select_image_format(PNG_ID, false, false, false), None);
    }

    #[test]
    fn persistence_queue_is_bounded_by_count_and_bytes() {
        assert!(!persist_queue_over_budget(0, 0, 1024));
        assert!(persist_queue_over_budget(MAX_IMAGE_PERSIST_JOBS, 0, 1024));
        assert!(!persist_queue_over_budget(
            1,
            MAX_IMAGE_PERSIST_BYTES - 1024,
            1024
        ));
        assert!(persist_queue_over_budget(
            1,
            MAX_IMAGE_PERSIST_BYTES - 1024,
            1025
        ));
    }

    #[test]
    fn persist_image_rebuilds_corrupt_cache_before_publishing() {
        use std::io::Cursor;

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "clippi-image-persist-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();

        let image = image::DynamicImage::new_rgba8(2, 2);
        let mut png = Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let raw = RawClipboardImage::Png(png.into_inner());
        let hash = hash_raw_image(&raw);
        let final_path = root.join(format!("{hash:016x}.png"));
        std::fs::write(&final_path, b"broken").unwrap();

        let captured = CapturedImage {
            raw,
            width: 2,
            height: 2,
            raw_hash: hash,
            source: None,
        };
        let item = persist_image(&captured, &root).expect("valid image should be published");

        assert_eq!(image::image_dimensions(&final_path).unwrap(), (2, 2));
        assert_eq!(item.image_path, final_path.to_string_lossy());
        assert!(std::fs::read_dir(&root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp.png")
        }));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn persist_image_converts_native_tiff_to_png() {
        use std::io::Cursor;

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "clippi-image-tiff-persist-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();

        let image = image::DynamicImage::new_rgba8(3, 2);
        let mut tiff = Cursor::new(Vec::new());
        image.write_to(&mut tiff, image::ImageFormat::Tiff).unwrap();
        let raw = RawClipboardImage::Tiff(tiff.into_inner());
        let hash = hash_raw_image(&raw);
        let captured = CapturedImage {
            raw,
            width: 3,
            height: 2,
            raw_hash: hash,
            source: None,
        };

        let item = persist_image(&captured, &root).expect("TIFF should be converted and published");
        let final_path = root.join(format!("{hash:016x}.png"));
        assert_eq!(image::image_dimensions(&final_path).unwrap(), (3, 2));
        assert_eq!(item.image_path, final_path.to_string_lossy());

        std::fs::remove_dir_all(&root).unwrap();
    }
}
