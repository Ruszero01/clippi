//! Local URL metadata backfill.
//!
//! Page titles live in `rich_data` and sync across devices. Favicons are local
//! cache files, so every device should fetch its own copy when a link appears.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::core::db::Database;
use crate::core::types::{url_to_domain, RichData};

/// Ensure the favicon for `url` exists in the local disk cache.
/// Returns `true` only when this call created a previously missing cache file.
pub fn ensure_url_favicon_cached(url: &str) -> bool {
    let domain = url_to_domain(url);
    if domain.is_empty() || crate::services::favicon::favicon_cache_path(&domain).is_some() {
        return false;
    }

    crate::services::favicon::ensure_favicon_cached(&domain).is_some()
}

pub fn spawn_ensure_url_favicon_cached(url: String) {
    std::thread::spawn(move || {
        ensure_url_favicon_cached(&url);
    });
}

/// Backfill local-only URL assets for synced link items.
///
/// This deliberately does not update DB rows: favicon cache files are not part
/// of sync semantics and should not mark items dirty or change `updated_at`.
pub fn backfill_link_favicons_from_db(db: &Mutex<Database>) -> usize {
    let urls = match db.lock() {
        Ok(db) => match db.get_all_sync_items_with_tags() {
            Ok(items) => items
                .into_iter()
                .filter(|item| item.meta_type == "link")
                .map(|item| item.full_text)
                .collect::<Vec<_>>(),
            Err(e) => {
                log::warn!("url_assets: failed to query link items for favicon backfill: {e}");
                Vec::new()
            }
        },
        Err(e) => {
            log::warn!("url_assets: failed to lock DB for favicon backfill: {e}");
            Vec::new()
        }
    };

    urls.into_iter()
        .filter(|url| ensure_url_favicon_cached(url))
        .count()
}

pub fn spawn_link_open_backfill(
    url: String,
    content_hash: u64,
    db_path: String,
    rich_data: String,
    fetch_title: bool,
    sync_dirty: Arc<AtomicBool>,
    mark_sync_dirty: bool,
) {
    std::thread::spawn(move || {
        ensure_url_favicon_cached(&url);

        if fetch_title
            && RichData::from_json(&rich_data).page_title.is_none()
            && crate::services::url_title::fetch_and_store_title(&url, content_hash, &db_path)
            && mark_sync_dirty
        {
            sync_dirty.store(true, Ordering::SeqCst);
        }
    });
}
