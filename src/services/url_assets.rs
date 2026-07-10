//! Local URL metadata backfill.
//!
//! Page titles live in `rich_data` and sync across devices. Favicons are local
//! cache files, so every device should fetch its own copy when a link appears.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};

use crate::core::db::Database;
use crate::core::types::{url_to_domain, RichData};

const URL_ASSET_WORKERS: usize = 2;
const MAX_URL_ASSET_QUEUE: usize = 128;

enum UrlAssetJob {
    Favicon {
        url: String,
    },
    Title {
        url: String,
        content_hash: u64,
        db_path: String,
        needs_refresh: Arc<AtomicBool>,
        sync_dirty: Arc<AtomicBool>,
        mark_sync_dirty: bool,
    },
    LinkOpenBackfill {
        url: String,
        content_hash: u64,
        db_path: String,
        rich_data: String,
        fetch_title: bool,
        sync_dirty: Arc<AtomicBool>,
        mark_sync_dirty: bool,
    },
}

static URL_ASSET_QUEUE: OnceLock<SyncSender<UrlAssetJob>> = OnceLock::new();

fn queue() -> &'static SyncSender<UrlAssetJob> {
    URL_ASSET_QUEUE.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel(MAX_URL_ASSET_QUEUE);
        let receiver = Arc::new(Mutex::new(receiver));
        for _ in 0..URL_ASSET_WORKERS {
            let receiver = receiver.clone();
            std::thread::spawn(move || loop {
                let job = {
                    let Ok(receiver) = receiver.lock() else {
                        break;
                    };
                    receiver.recv()
                };
                match job {
                    Ok(job) => run_job(job),
                    Err(_) => break,
                }
            });
        }
        sender
    })
}

fn submit(job: UrlAssetJob) {
    match queue().try_send(job) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            log::warn!("url_assets: queue is full; dropped background URL asset job");
        }
        Err(TrySendError::Disconnected(_)) => {
            log::warn!("url_assets: queue disconnected; dropped background URL asset job");
        }
    }
}

fn run_job(job: UrlAssetJob) {
    match job {
        UrlAssetJob::Favicon { url } => {
            ensure_url_favicon_cached(&url);
        }
        UrlAssetJob::Title {
            url,
            content_hash,
            db_path,
            needs_refresh,
            sync_dirty,
            mark_sync_dirty,
        } => {
            if crate::services::url_title::fetch_and_store_title(&url, content_hash, &db_path) {
                needs_refresh.store(true, Ordering::SeqCst);
                if mark_sync_dirty {
                    sync_dirty.store(true, Ordering::SeqCst);
                }
            }
        }
        UrlAssetJob::LinkOpenBackfill {
            url,
            content_hash,
            db_path,
            rich_data,
            fetch_title,
            sync_dirty,
            mark_sync_dirty,
        } => {
            ensure_url_favicon_cached(&url);

            if fetch_title
                && RichData::from_json(&rich_data).page_title.is_none()
                && crate::services::url_title::fetch_and_store_title(&url, content_hash, &db_path)
                && mark_sync_dirty
            {
                sync_dirty.store(true, Ordering::SeqCst);
            }
        }
    }
}

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
    submit(UrlAssetJob::Favicon { url });
}

pub fn spawn_fetch_url_title(
    url: String,
    content_hash: u64,
    db_path: String,
    needs_refresh: Arc<AtomicBool>,
    sync_dirty: Arc<AtomicBool>,
    mark_sync_dirty: bool,
) {
    submit(UrlAssetJob::Title {
        url,
        content_hash,
        db_path,
        needs_refresh,
        sync_dirty,
        mark_sync_dirty,
    });
}

/// Backfill local-only URL assets for synced link items.
///
/// This deliberately does not update DB rows: favicon cache files are not part
/// of sync semantics and should not mark items dirty or change `updated_at`.
pub fn backfill_link_favicons_from_db(db: &Mutex<Database>) -> usize {
    let urls = match db.lock() {
        Ok(db) => match db.get_all_sync_items_with_tags(false) {
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
    submit(UrlAssetJob::LinkOpenBackfill {
        url,
        content_hash,
        db_path,
        rich_data,
        fetch_title,
        sync_dirty,
        mark_sync_dirty,
    });
}
