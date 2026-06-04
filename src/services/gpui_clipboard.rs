//! GPUI clipboard capture service.
//!
//! Reuses the platform clipboard watcher and drains captured items into the
//! framework-independent database/state layer.

use crate::core::types::{ClipboardItem, ContentType};
use crate::platform::clipboard::{create_listener, ClipboardListener, ClipboardShared};
use crate::state::app::AppState;

pub struct GpuiClipboardService {
    shared: ClipboardShared,
    listener: Box<dyn ClipboardListener>,
}

impl GpuiClipboardService {
    pub fn new() -> Self {
        let shared = ClipboardShared::new();
        let mut listener = create_listener();
        if let Err(err) = listener.start(&shared) {
            log::error!("Failed to start clipboard listener: {err}");
        }

        Self { shared, listener }
    }

    /// Access the `batch_pasting` flag shared with the clipboard listener.
    /// Set to `true` during batch paste to prevent the listener from
    /// recording intermediate clipboard writes (e.g., newline separators).
    pub fn batch_pasting(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.shared.batch_pasting.clone()
    }

    pub fn poll_state(&mut self, state: &mut AppState) -> bool {
        if self
            .shared
            .clear_selection_requested
            .swap(false, std::sync::atomic::Ordering::SeqCst)
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
            return false;
        }

        let mut changed = false;
        for mut item in pending {
            if state.settings.copy_as_plain_text && item.content_type == ContentType::RichText {
                item.content_type = ContentType::PlainText;
                item.rich_data.clear();
            }

            item.size = compute_size(&item);
            if let Err(err) = state.db.upsert(&item) {
                log::error!("Failed to upsert clipboard item: {err}");
                continue;
            }
            changed = true;
        }

        if changed {
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

        changed
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
