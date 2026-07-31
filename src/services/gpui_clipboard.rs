//! --- GPUI clipboard capture service. ---
//!
//! Reuses the platform clipboard watcher and drains captured items into the
//! --- framework-independent database/state layer. ---
//!
//! Also handles automatic post-processing for image items:
//! --- - QR code detection (synchronous — rqrr is fast) ---
//! --- - OCR text recognition (asynchronous — spawned on background thread) ---

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use crate::core::types::{ClipboardItem, ContentType, RichData};
use crate::platform::clipboard::{create_listener, ClipboardListener, ClipboardShared};
use crate::state::app::AppState;

const MAX_IMAGE_ANALYSIS_QUEUE: usize = 64;

#[derive(Clone)]
struct ImageAnalysisJob {
    img_path: String,
    content_hash: u64,
    db_path: String,
    do_qr: bool,
    do_ocr: bool,
}

struct ImageAnalysisWorker {
    queue: Arc<(Mutex<VecDeque<ImageAnalysisJob>>, Condvar)>,
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ImageAnalysisWorker {
    fn new(needs_refresh: Arc<AtomicBool>) -> Self {
        let queue = Arc::new((
            Mutex::new(VecDeque::<ImageAnalysisJob>::new()),
            Condvar::new(),
        ));
        let running = Arc::new(AtomicBool::new(true));
        let worker_queue = queue.clone();
        let worker_running = running.clone();

        let handle = thread::spawn(move || {
            while worker_running.load(Ordering::SeqCst) {
                let job = {
                    let (lock, condvar) = &*worker_queue;
                    let mut jobs = lock.lock().unwrap_or_else(|e| e.into_inner());
                    while jobs.is_empty() && worker_running.load(Ordering::SeqCst) {
                        jobs = condvar.wait(jobs).unwrap_or_else(|e| e.into_inner());
                    }
                    if !worker_running.load(Ordering::SeqCst) && jobs.is_empty() {
                        None
                    } else {
                        jobs.pop_front()
                    }
                };

                let Some(job) = job else {
                    break;
                };

                if job.do_qr {
                    run_qr_analysis(&job, &needs_refresh);
                }
                if job.do_ocr {
                    run_ocr_analysis(&job, &needs_refresh);
                }
            }
        });

        Self {
            queue,
            running,
            handle: Some(handle),
        }
    }

    fn enqueue(&self, job: ImageAnalysisJob) {
        let (lock, condvar) = &*self.queue;
        let mut jobs = lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = jobs
            .iter_mut()
            .find(|existing| existing.content_hash == job.content_hash)
        {
            existing.do_qr |= job.do_qr;
            existing.do_ocr |= job.do_ocr;
            return;
        }
        if jobs.len() >= MAX_IMAGE_ANALYSIS_QUEUE {
            jobs.pop_front();
            log::warn!("Image analysis queue is full; dropped the oldest pending job");
        }
        jobs.push_back(job);
        condvar.notify_one();
    }
}

impl Drop for ImageAnalysisWorker {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        self.queue.1.notify_one();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub struct GpuiClipboardService {
    shared: ClipboardShared,
    listener: Box<dyn ClipboardListener>,
    /// Set to true by async OCR threads when results are written to DB.
    /// Checked at the start of each poll cycle to trigger a UI refresh.
    needs_refresh: Arc<AtomicBool>,
    image_analysis: ImageAnalysisWorker,
}

impl GpuiClipboardService {
    pub fn new(initial_app_blacklist: Vec<String>) -> Self {
        let shared = ClipboardShared::new();
        // Populate the blacklist snapshot BEFORE starting the listener so
        // capture_baseline and the first poll find the correct value.
        *shared
            .clipboard_app_blacklist
            .write()
            .unwrap_or_else(|error| error.into_inner()) = initial_app_blacklist;
        let mut listener = create_listener();
        if let Err(err) = listener.start(&shared) {
            log::error!("Failed to start clipboard listener: {err}");
        }
        let needs_refresh = Arc::new(AtomicBool::new(false));
        let image_analysis = ImageAnalysisWorker::new(needs_refresh.clone());

        Self {
            shared,
            listener,
            needs_refresh,
            image_analysis,
        }
    }

    /// Update the app-blacklist snapshot used by the listener thread.
    /// Call after every add / remove so the next poll picks up the change.
    pub fn set_app_blacklist(&self, blacklist: Vec<String>) {
        *self
            .shared
            .clipboard_app_blacklist
            .write()
            .unwrap_or_else(|error| error.into_inner()) = blacklist;
    }

    /// Access the `batch_pasting` flag shared with the clipboard listener.
    /// Set to `true` during batch paste to prevent the listener from
    /// recording intermediate clipboard writes (e.g., newline separators).
    pub fn batch_pasting(&self) -> Arc<AtomicBool> {
        self.shared.batch_pasting.clone()
    }

    /// Access the `skip_next` flag shared with the clipboard listener.
    /// Set to `true` before writing OCR/color-conversion text to clipboard.
    /// The listener skips one detection cycle and updates the baseline
    /// sequence number so the internal write is never treated as a new
    /// history entry.
    pub fn skip_next(&self) -> Arc<AtomicBool> {
        self.shared.skip_next.clone()
    }

    pub fn poll_state(&mut self, state: &mut AppState) -> bool {
        // --- ── Handle async OCR completion ── ---
        let needs_reload = self.needs_refresh.swap(false, Ordering::SeqCst)
            || crate::platform::clipboard::take_thumbnail_ready();

        if self
            .shared
            .clear_selection_requested
            .swap(false, Ordering::SeqCst)
        {
            state.clear_selection();
        }

        let pending = {
            let Ok(mut pending) = self.shared.pending.lock() else {
                log::error!("Clipboard pending lock poisoned");
                return false;
            };
            pending.drain(..).collect::<Vec<_>>()
        };

        if pending.is_empty() {
            if needs_reload {
                state.reload_items();
                return true;
            }
            return false;
        }

        let db_path = state.settings.db_path.clone();
        let qr_enabled = state.settings.qr_enabled;
        let ocr_enabled = state.settings.ocr_enabled;
        let mut changed = false;

        for mut item in pending {
            if state.settings.copy_as_plain_text && item.content_type == ContentType::RichText {
                item.content_type = ContentType::PlainText;
                item.rich_data.clear();
                item.meta_type.clear();
            }

            item.size = compute_size(&item);

            // --- ── Pre-upsert: carry over cached OCR/QR from existing DB record ── ---
            // --- When an image is re-copied, the existing DB record may already have ---
            // --- OCR/QR results. Carry them into the new item so they aren't lost. ---
            let mut need_ocr = false;
            let mut need_qr = false;
            if item.content_type == ContentType::Image && !item.image_path.is_empty() {
                let rd = RichData::from_json(&item.rich_data);
                let allow_automatic_analysis = rd.remote_host.is_none();
                if allow_automatic_analysis && ocr_enabled && rd.ocr_text.is_none() {
                    if let Ok(Some(ref existing)) = state.db.get_by_hash(item.content_hash) {
                        let erd = RichData::from_json(&existing.rich_data);
                        if let Some(ref cached) = erd.ocr_text {
                            let mut merged = rd.clone();
                            merged.ocr_text = Some(cached.clone());
                            item.rich_data = merged.to_json();
                        } else {
                            need_ocr = true;
                        }
                    } else {
                        need_ocr = true; // Brand-new image
                    }
                }
                if allow_automatic_analysis && qr_enabled && rd.qr_text.is_none() {
                    if let Ok(Some(ref existing)) = state.db.get_by_hash(item.content_hash) {
                        let erd = RichData::from_json(&existing.rich_data);
                        if let Some(ref cached) = erd.qr_text {
                            let mut merged = RichData::from_json(&item.rich_data);
                            merged.qr_text = Some(cached.clone());
                            item.rich_data = merged.to_json();
                        } else {
                            need_qr = true;
                        }
                    } else {
                        need_qr = true; // Brand-new image
                    }
                }
            }

            // ── Pre-upsert: carry over rich_data from existing DB record ──
            // When paste-as-plain-text writes plain text to the clipboard,
            // the listener detects a PlainText item with empty rich_data. If
            // the content_hash matches an existing RichText record, carry over
            // rich_data so it isn't overwritten by the upsert below.
            if !state.settings.copy_as_plain_text && item.rich_data.is_empty() {
                if let Ok(Some(ref existing)) = state.db.get_by_hash(item.content_hash) {
                    if !existing.rich_data.is_empty() {
                        item.rich_data = existing.rich_data.clone();
                        item.meta_type = existing.meta_type.clone();
                        if existing.content_type == ContentType::RichText {
                            item.content_type = ContentType::RichText;
                        }
                    }
                }
            }

            if let Err(err) = state.db.upsert(&item) {
                log::error!("Failed to upsert clipboard item: {err}");
                continue;
            }

            // Play copy sound on every successful clipboard detection.
            // This confirms the copy *action* (not new data) to the user.
            if state.settings.copy_sound_enabled {
                crate::services::copy_sound::play_copy_sound(&state.settings.copy_sound_file);
            }

            if state.should_mark_sync_dirty(&item) {
                state.sync_dirty.store(true, Ordering::SeqCst);
            }
            changed = true;

            // ── Post-upsert: run detection for items that need it ──
            if need_qr || need_ocr {
                self.process_image_analysis(&item, &db_path, need_qr, need_ocr);
            }

            // ── Post-upsert: spawn URL metadata fetch for link items ──
            if item.meta_type == "link" {
                crate::services::url_assets::spawn_ensure_url_favicon_cached(
                    item.full_text.clone(),
                );
            }
            if item.meta_type == "link"
                && should_fetch_url_title(&item.full_text, state.settings.auto_fetch_url_title)
            {
                let url = item.full_text.clone();
                let content_hash = item.content_hash;
                let db_path = db_path.clone();
                let needs_refresh = self.needs_refresh.clone();
                let sync_dirty = state.sync_dirty.clone();
                let mark_sync_dirty = state.should_mark_sync_dirty(&item);
                crate::services::url_assets::spawn_fetch_url_title(
                    url,
                    content_hash,
                    db_path,
                    needs_refresh,
                    sync_dirty,
                    mark_sync_dirty,
                );
            }
        }

        if changed || needs_reload {
            if state.settings.max_items > 0 {
                if let Err(err) = state
                    .db
                    .prune_excess_non_favorites(state.settings.max_items)
                {
                    log::error!("Failed to prune clipboard items: {err}");
                }
            }
            state.reload_items();
            state.reload_tags();
        }

        changed || needs_reload
    }

    fn process_image_analysis(
        &self,
        item: &ClipboardItem,
        db_path: &str,
        do_qr: bool,
        do_ocr: bool,
    ) {
        self.image_analysis.enqueue(ImageAnalysisJob {
            img_path: item.image_path.clone(),
            content_hash: item.content_hash,
            db_path: db_path.to_string(),
            do_qr,
            do_ocr,
        });
    }
}

fn should_fetch_url_title(url: &str, enabled: bool) -> bool {
    enabled && crate::core::secret::url_sensitive_ranges(url).is_empty()
}

impl Drop for GpuiClipboardService {
    fn drop(&mut self) {
        self.listener.stop();
    }
}

fn compute_size(item: &ClipboardItem) -> i64 {
    match item.content_type {
        ContentType::File => item.size,
        ContentType::PlainText | ContentType::RichText => {
            if item.meta_type == "color" {
                0
            } else {
                item.full_text.chars().count() as i64
            }
        }
        ContentType::Image => 0,
    }
}

fn run_qr_analysis(job: &ImageAnalysisJob, needs_refresh: &AtomicBool) {
    match crate::core::qr::detect_qr(std::path::Path::new(&job.img_path)) {
        Ok(Some(text)) => {
            let resolved = crate::core::paths::resolve_db_path(&job.db_path);
            if let Ok(db) = crate::core::db::Database::open(&resolved.to_string_lossy()) {
                if let Ok(Some(existing)) = db.get_by_hash(job.content_hash) {
                    let mut rd = RichData::from_json(&existing.rich_data);
                    if rd.qr_text.is_none() {
                        rd.qr_text = Some(text);
                        let json = rd.to_json();
                        let _ = db.update_rich_data(existing.id, &json);
                        needs_refresh.store(true, Ordering::SeqCst);
                    }
                }
            }
        }
        Ok(None) => { /* no QR code found in this image */ }
        Err(e) => log::error!("QR detection error: {e}"),
    }
}

fn run_ocr_analysis(job: &ImageAnalysisJob, needs_refresh: &AtomicBool) {
    let engine = crate::core::ocr::create_ocr_engine();
    match engine.recognize(std::path::Path::new(&job.img_path)) {
        Ok(text) if !text.trim().is_empty() => {
            let resolved = crate::core::paths::resolve_db_path(&job.db_path);
            if let Ok(db) = crate::core::db::Database::open(&resolved.to_string_lossy()) {
                if let Ok(Some(existing)) = db.get_by_hash(job.content_hash) {
                    let mut rd = RichData::from_json(&existing.rich_data);
                    if rd.ocr_text.is_none() {
                        rd.ocr_text = Some(text);
                        let json = rd.to_json();
                        let _ = db.update_rich_data(existing.id, &json);
                        needs_refresh.store(true, Ordering::SeqCst);
                    }
                }
            }
        }
        Ok(_) => { /* empty result, skip */ }
        Err(e) => log::error!("OCR error: {e}"),
    }
}

#[cfg(test)]
mod url_title_policy_tests {
    use super::should_fetch_url_title;

    #[test]
    fn skips_title_fetch_for_sensitive_urls() {
        assert!(!should_fetch_url_title(
            "https://user:password@example.com/reset",
            true
        ));
        assert!(!should_fetch_url_title(
            "https://example.com/reset?token=one-time-secret",
            true
        ));
    }

    #[test]
    fn allows_title_fetch_for_normal_urls_when_enabled() {
        assert!(should_fetch_url_title("https://example.com/docs", true));
        assert!(!should_fetch_url_title("https://example.com/docs", false));
    }
}
