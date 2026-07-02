//! --- Cache cleanup — removes orphaned image files, expired icon caches, ---
//! --- and expired sync tombstones. ---
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
}

impl CleanupStats {
    pub fn is_empty(&self) -> bool {
        self.orphan_images == 0 && self.unreferenced_icons == 0 && self.expired_tombstones == 0
    }
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

// ── Unified entry point ───────────────────────────────────────────────

/// Run all cleanup tasks synchronously. Returns aggregated stats.
///
/// Called at startup and periodically via the poll loop / UI button.
pub fn run_cleanup(db: &Database) -> CleanupStats {
    let orphan_images = clean_orphan_images(db);
    let unreferenced_icons = clean_unreferenced_icons(db);
    let expired_tombstones = clean_expired_tombstones(db);

    let stats = CleanupStats {
        orphan_images,
        unreferenced_icons,
        expired_tombstones,
    };

    if !stats.is_empty() {
        log::info!(
            "run_cleanup: {} orphan images, {} unreferenced icons, {} expired tombstones",
            stats.orphan_images,
            stats.unreferenced_icons,
            stats.expired_tombstones,
        );
    }

    stats
}

/// Backward-compatible alias. Prefer `run_cleanup`.
pub fn cleanup_unused_cache(db: &Database) {
    run_cleanup(db);
}
