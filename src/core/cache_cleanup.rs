//! --- Cache cleanup — removes orphaned image files, expired icon caches, ---
//! --- expired sync tombstones, and expired clipboard items. ---
//!
//! Each cleanup task is a standalone function. `run_cleanup` orchestrates
//! all of them synchronously; callers (startup, poll loop, UI button) go
//! through that single entry point.

use crate::core::db::Database;
use std::collections::HashSet;
use std::fs;

/// 30-day expiry for sync tombstones.
const TOMBSTONE_EXPIRY_DAYS: i64 = 30;

/// Aggregated stats from a full cleanup pass.
#[derive(Debug, Default, Clone)]
pub struct CleanupStats {
    /// Number of orphaned image / thumbnail files removed.
    pub orphan_images: u32,
    /// Number of unreferenced icon cache files removed.
    pub unreferenced_icons: u32,
    /// Number of expired tombstone rows removed.
    pub expired_tombstones: u32,
    /// Number of clipboard items removed due to retention_days expiry.
    pub expired_items: u32,
    /// Deleted item IDs that had custom hotkeys and need unregistering.
    pub expired_hotkey_item_ids: Vec<i64>,
    /// Whether cleanup wrote sync tombstones and should trigger a sync push.
    pub sync_dirty: bool,
}

impl CleanupStats {
    pub fn is_empty(&self) -> bool {
        // Side-effect fields (`expired_hotkey_item_ids`, `sync_dirty`) are
        // derived from row cleanup and are intentionally excluded here.
        self.orphan_images == 0
            && self.unreferenced_icons == 0
            && self.expired_tombstones == 0
            && self.expired_items == 0
    }
}

/// Sync settings needed when retention cleanup deletes live clipboard items.
#[derive(Debug, Clone)]
pub struct CleanupSyncScope {
    pub include_images: bool,
    pub favorites_only: bool,
    pub device_name: String,
}

// ── Per-task helpers ──────────────────────────────────────────────────

/// Remove image + thumbnail files that are no longer referenced by any
/// clipboard item in the database.
fn clean_orphan_images(db: &Database) -> u32 {
    let images_dir = crate::core::paths::images_dir();
    if !images_dir.exists() {
        return 0;
    }

    let referenced: HashSet<String> = match db.get_all_image_hashes() {
        Ok(hashes) => hashes.into_iter().collect(),
        Err(e) => {
            log::error!("clean_orphan_images: failed to query image hashes: {e}");
            return 0;
        }
    };

    let mut removed: u32 = 0;

    if let Ok(entries) = fs::read_dir(&images_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };

            // Only process *.png files at the top level (not recurring into icons/).
            if !name.ends_with(".png") || path.is_dir() {
                continue;
            }

            let hash = if let Some(rest) = name.strip_suffix(".png") {
                rest.strip_prefix("thumb_").unwrap_or(rest)
            } else {
                continue;
            };

            if !referenced.contains(hash) {
                if let Err(e) = fs::remove_file(&path) {
                    log::warn!("clean_orphan_images: failed to remove {name}: {e}");
                } else {
                    log::info!("clean_orphan_images: removed orphan file {name}");
                    removed += 1;
                }
            }
        }
    }

    removed
}

/// Remove icon cache files (favicon, source-app icons, file icons) that are no
/// longer referenced by any clipboard item in the database.  Mirrors
/// `clean_orphan_images` in strategy — reference-based instead of time-based.
fn clean_unreferenced_icons(db: &Database) -> u32 {
    let images_dir = crate::core::paths::images_dir();
    let icon_dirs = [images_dir.join("icons"), images_dir.join("file_icons")];

    // Collect every icon filename that is still referenced from the DB.
    // Keys are relative to icon root directories (e.g. "icons/Chrome" or
    // "file_icons/exe_3f8a2b1c9d4e5f06") — strip the directory prefix
    // for the actual filename comparison.
    let referenced: HashSet<String> = match db.get_all_referenced_icon_keys() {
        Ok(keys) => keys
            .into_iter()
            .map(|k| {
                // Strip "icons/" or "file_icons/" prefix → bare filename.
                if let Some(rest) = k.strip_prefix("icons/") {
                    rest.to_string()
                } else if let Some(rest) = k.strip_prefix("file_icons/") {
                    rest.to_string()
                } else {
                    k
                }
            })
            .collect(),
        Err(e) => {
            log::error!("clean_unreferenced_icons: failed to query icon keys: {e}");
            return 0;
        }
    };

    let mut removed: u32 = 0;

    for icons_dir in icon_dirs.iter().filter(|dir| dir.exists()) {
        let Ok(entries) = fs::read_dir(icons_dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            // File icon keys don't include the ".png" extension in the
            // DB-derived key but the on-disk file does — strip extension
            // for matching.
            let stem = name.strip_suffix(".png").unwrap_or(name);

            if !referenced.contains(stem) {
                let display = name.to_string();
                if let Err(e) = fs::remove_file(&path) {
                    log::warn!("clean_unreferenced_icons: failed to remove {display}: {e}");
                } else {
                    log::info!("clean_unreferenced_icons: removed {display}");
                    removed += 1;
                }
            }
        }
    }

    removed
}

/// Remove sync tombstones older than `TOMBSTONE_EXPIRY_DAYS` days.
fn clean_expired_tombstones(db: &Database) -> u32 {
    match db.cleanup_old_tombstones(TOMBSTONE_EXPIRY_DAYS) {
        Ok(deleted) => deleted,
        Err(e) => {
            log::error!("clean_expired_tombstones: {e}");
            0
        }
    }
}

/// Remove non-favorite clipboard items older than `retention_days`.
fn clean_expired_clipboard_items(
    db: &Database,
    retention_days: u32,
    sync_scope: Option<&CleanupSyncScope>,
) -> (u32, bool, Vec<i64>) {
    match db.prune_expired_items(retention_days) {
        Ok(items) => {
            let mut sync_dirty = false;
            let hotkey_item_ids = items
                .iter()
                .filter(|item| !item.custom_hotkey.is_empty())
                .map(|item| item.id)
                .collect();
            if let Some(scope) = sync_scope {
                let now = chrono::Utc::now().to_rfc3339();
                for item in &items {
                    if crate::core::sync_scope::item_in_sync_scope(
                        item.content_type,
                        item.is_favorite,
                        scope.include_images,
                        scope.favorites_only,
                    ) {
                        if let Err(e) =
                            db.record_item_deletion(item.content_hash, &now, &scope.device_name)
                        {
                            log::error!(
                                "clean_expired_clipboard_items: record tombstone {}: {e}",
                                item.content_hash
                            );
                        } else {
                            sync_dirty = true;
                        }
                    }
                }
            }
            (items.len() as u32, sync_dirty, hotkey_item_ids)
        }
        Err(e) => {
            log::error!("clean_expired_clipboard_items: {e}");
            (0, false, Vec::new())
        }
    }
}

// ── Unified entry point ───────────────────────────────────────────────

/// Run all cleanup tasks synchronously. Returns aggregated stats.
///
/// Called at startup and periodically via the poll loop / UI button.
pub fn run_cleanup(
    db: &Database,
    retention_days: u32,
    sync_scope: Option<&CleanupSyncScope>,
) -> CleanupStats {
    let orphan_images = clean_orphan_images(db);
    let unreferenced_icons = clean_unreferenced_icons(db);
    let expired_tombstones = clean_expired_tombstones(db);
    let (expired_items, sync_dirty, expired_hotkey_item_ids) =
        clean_expired_clipboard_items(db, retention_days, sync_scope);

    let stats = CleanupStats {
        orphan_images,
        unreferenced_icons,
        expired_tombstones,
        expired_items,
        expired_hotkey_item_ids,
        sync_dirty,
    };

    if !stats.is_empty() {
        log::info!(
            "run_cleanup: {} orphan images, {} unreferenced icons, {} expired tombstones, {} expired items",
            stats.orphan_images,
            stats.unreferenced_icons,
            stats.expired_tombstones,
            stats.expired_items,
        );
    }

    stats
}
