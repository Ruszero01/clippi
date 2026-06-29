//! --- Cache cleanup — removes orphaned image files, expired icon caches, ---
//! --- and expired sync tombstones. ---
//!
//! Each cleanup task is a standalone function. `run_cleanup` orchestrates
//! all of them synchronously; callers (startup, poll loop, UI button) go
//! through that single entry point.
//!
//! Also provides point-to-point deletion: when a single clipboard item is
//! removed, `delete_item_images` cleans its image + thumbnail immediately,
//! avoiding the need for a full sweep during the session.

use crate::core::db::Database;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// 30-day expiry for icon cache files and sync tombstones.
const ICON_EXPIRY_DAYS: u64 = 30;
const TOMBSTONE_EXPIRY_DAYS: i64 = 30;

/// Aggregated stats from a full cleanup pass.
#[derive(Debug, Default, Clone)]
pub struct CleanupStats {
    /// Number of orphaned image / thumbnail files removed.
    pub orphan_images: u32,
    /// Number of expired icon cache files removed.
    pub expired_icons: u32,
    /// Number of expired tombstone rows removed.
    pub expired_tombstones: u32,
}

impl CleanupStats {
    pub fn is_empty(&self) -> bool {
        self.orphan_images == 0 && self.expired_icons == 0 && self.expired_tombstones == 0
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

/// Remove icon cache files (favicon, source-app icons, file icons) that haven't been
/// accessed in `ICON_EXPIRY_DAYS` days.
fn clean_expired_icons() -> u32 {
    let images_dir = crate::core::paths::images_dir();
    let icon_dirs = [images_dir.join("icons"), images_dir.join("file_icons")];

    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(ICON_EXPIRY_DAYS * 86400))
        .unwrap_or(SystemTime::UNIX_EPOCH);

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

            let expired = match path.metadata() {
                Ok(m) => m.modified().map(|t| t < cutoff).unwrap_or(false),
                Err(_) => false,
            };

            if expired {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                if let Err(e) = fs::remove_file(&path) {
                    log::warn!("clean_expired_icons: failed to remove {name}: {e}");
                } else {
                    log::info!("clean_expired_icons: removed {name}");
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
    let expired_icons = clean_expired_icons();
    let expired_tombstones = clean_expired_tombstones(db);

    let stats = CleanupStats {
        orphan_images,
        expired_icons,
        expired_tombstones,
    };

    if !stats.is_empty() {
        log::info!(
            "run_cleanup: {} orphan images, {} expired icons, {} expired tombstones",
            stats.orphan_images,
            stats.expired_icons,
            stats.expired_tombstones,
        );
    }

    stats
}

/// Backward-compatible alias. Prefer `run_cleanup`.
pub fn cleanup_unused_cache(db: &Database) {
    run_cleanup(db);
}

// ── Point-to-point helpers ────────────────────────────────────────────

/// Delete the original image and its thumbnail for a clipboard item.
///
/// `image_path` is the stored full path from the DB.
/// The thumbnail is always looked up in `images_dir()` (where
/// `generate_thumbnail` writes it), regardless of where the original lives.
///
/// Returns the number of files actually deleted (0–2). Errors are logged and
/// swallowed — the next full sweep will catch any leftovers.
#[allow(dead_code)] // kept for future use
pub fn delete_item_images(image_path: &str) -> u32 {
    if image_path.is_empty() {
        return 0;
    }

    let original = Path::new(image_path);
    let mut deleted: u32 = 0;

    // --- Delete original ---
    if original.exists() {
        if let Err(e) = fs::remove_file(original) {
            log::warn!("delete_item_images: failed to remove {image_path}: {e}");
        } else {
            deleted += 1;
        }
    }

    // --- Delete thumbnail (always in images_dir, not next to original) ---
    if let Some(thumb) = thumbnail_path(original) {
        if thumb.exists() {
            if let Err(e) = fs::remove_file(&thumb) {
                log::warn!(
                    "delete_item_images: failed to remove thumbnail {}: {e}",
                    thumb.display()
                );
            } else {
                deleted += 1;
            }
        }
    }

    if deleted > 0 {
        log::info!("delete_item_images: cleaned {deleted} file(s) for {image_path}");
    }
    deleted
}

/// Derive the thumbnail path from an original image path.
///
/// Thumbnails are always written to `images_dir()` by `generate_thumbnail`,
/// regardless of where the original image lives. This function extracts the
/// file stem (hash) from the original path and builds the thumbnail path
/// inside `images_dir()`.
#[allow(dead_code)] // kept for future use
fn thumbnail_path(original: &Path) -> Option<PathBuf> {
    let stem = original.file_stem()?.to_str()?;
    let img_dir = crate::core::paths::images_dir();
    Some(img_dir.join(format!("thumb_{stem}.png")))
}
