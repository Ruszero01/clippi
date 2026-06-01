//! Cache cleanup — removes orphaned image files and expired icon caches.
//! Runs once at startup in a background thread.

use crate::core::db::Database;
use std::collections::HashSet;
use std::fs;
use std::time::{Duration, SystemTime};

/// 30-day expiry for icon cache files.
const ICON_EXPIRY_DAYS: u64 = 30;

/// Run all cache cleanup tasks. Non-fatal — errors are logged and skipped.
pub fn cleanup_unused_cache(db: &Database) {
    let images_dir = crate::core::paths::images_dir();
    let icons_dir = images_dir.join("icons");

    if !images_dir.exists() {
        return;
    }

    // Collect referenced image hashes from the database.
    let referenced: HashSet<String> = match db.get_all_image_hashes() {
        Ok(hashes) => hashes.into_iter().collect(),
        Err(e) => {
            log::error!("cache_cleanup: failed to query image hashes: {e}");
            return;
        }
    };

    // Clean orphaned image files and thumbnails in images/ (not recursing into icons/).
    if let Ok(entries) = fs::read_dir(&images_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };

            // Only process *.png files at the top level (not in icons/).
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
                    log::warn!("cache_cleanup: failed to remove orphan file {name}: {e}");
                } else {
                    log::info!("cache_cleanup: removed orphan file {name}");
                }
            }
        }
    }

    // Clean expired icon cache files.
    if icons_dir.exists() {
        let cutoff = SystemTime::now()
            .checked_sub(Duration::from_secs(ICON_EXPIRY_DAYS * 86400))
            .unwrap_or(SystemTime::UNIX_EPOCH);

        if let Ok(entries) = fs::read_dir(&icons_dir) {
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
                        log::warn!("cache_cleanup: failed to remove expired icon {name}: {e}");
                    } else {
                        log::info!("cache_cleanup: removed expired icon {name}");
                    }
                }
            }
        }
    }
}
