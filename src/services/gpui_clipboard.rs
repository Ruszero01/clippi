//! --- GPUI clipboard capture service. ---
//!
//! Reuses the platform clipboard watcher and drains captured items into the
//! --- framework-independent database/state layer. ---
//!
//! Also handles automatic post-processing for image items:
//! --- - QR code detection (synchronous — rqrr is fast) ---
//! --- - OCR text recognition (asynchronous — spawned on background thread) ---

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::core::types::{ClipboardItem, ContentType, RichData};
use crate::platform::clipboard::{create_listener, ClipboardListener, ClipboardShared};
use crate::state::app::AppState;

pub struct GpuiClipboardService {
    shared: ClipboardShared,
    listener: Box<dyn ClipboardListener>,
    /// Set to true by async OCR threads when results are written to DB.
    /// Checked at the start of each poll cycle to trigger a UI refresh.
    needs_refresh: Arc<AtomicBool>,
}

impl GpuiClipboardService {
    pub fn new() -> Self {
        let shared = ClipboardShared::new();
        let mut listener = create_listener();
        if let Err(err) = listener.start(&shared) {
            log::error!("Failed to start clipboard listener: {err}");
        }

        Self {
            shared,
            listener,
            needs_refresh: Arc::new(AtomicBool::new(false)),
        }
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
        let needs_reload = self.needs_refresh.swap(false, Ordering::SeqCst);

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
                if ocr_enabled && rd.ocr_text.is_none() {
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
                if qr_enabled && rd.qr_text.is_none() {
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
            if item.rich_data.is_empty() {
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
            if state.should_mark_sync_dirty(&item) {
                state.sync_dirty.store(true, Ordering::SeqCst);
            }
            changed = true;

            // ── Post-upsert: run detection for items that need it ──
            if need_qr {
                self.process_qr(&item, state);
            }
            if need_ocr {
                self.process_ocr(&item, &db_path);
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

    /// Run QR code detection on an image item and persist the result to DB.
    /// Called synchronously from the poll loop (rqrr is fast enough for this).
    fn process_qr(&self, item: &ClipboardItem, state: &mut AppState) {
        match crate::core::qr::detect_qr(std::path::Path::new(&item.image_path)) {
            Ok(Some(text)) => {
                let mut rd = RichData::from_json(&item.rich_data);
                rd.qr_text = Some(text);
                let json = rd.to_json();
                if let Ok(Some(existing)) = state.db.get_by_hash(item.content_hash) {
                    let _ = state.db.update_rich_data(existing.id, &json);
                }
            }
            Ok(None) => { /* no QR code found in this image */ }
            Err(e) => log::error!("QR detection error: {e}"),
        }
    }

    /// Spawn a background thread for OCR on an image item.
    /// Results are written to DB asynchronously; the needs_refresh flag
    /// triggers a UI reload on the next poll cycle.
    ///
    /// `db_path` is the user-configured database path from settings (empty
    /// string means use the default platform data directory).
    fn process_ocr(&self, item: &ClipboardItem, db_path: &str) {
        let img_path = item.image_path.clone();
        let content_hash = item.content_hash;
        let needs_refresh = self.needs_refresh.clone();
        let db_path = db_path.to_string();

        std::thread::spawn(move || {
            let engine = crate::core::ocr::create_ocr_engine();
            match engine.recognize(std::path::Path::new(&img_path)) {
                Ok(text) if !text.trim().is_empty() => {
                    let resolved = crate::core::paths::resolve_db_path(&db_path);
                    if let Ok(db) = crate::core::db::Database::open(&resolved.to_string_lossy()) {
                        if let Ok(Some(existing)) = db.get_by_hash(content_hash) {
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
        });
    }
}

impl Drop for GpuiClipboardService {
    fn drop(&mut self) {
        self.listener.stop();
    }
}

fn compute_size(item: &ClipboardItem) -> i64 {
    match item.content_type {
        ContentType::File => item.size,
        ContentType::PlainText | ContentType::RichText | ContentType::Link | ContentType::Path => {
            item.full_text.chars().count() as i64
        }
        ContentType::Image | ContentType::Color => 0,
    }
}
