//! --- Cache cleanup — removes orphaned image files, expired icon caches, ---
//! --- expired sync tombstones, expired clipboard items, and stale items. ---
//!
//! Each cleanup task is a standalone function. `run_cleanup_with_options`
//! orchestrates all of them; callers go through that single entry point.

use crate::core::db::Database;
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::fs;

/// 30-day expiry for sync tombstones.
const TOMBSTONE_EXPIRY_DAYS: i64 = 30;

// ── Configuration types ────────────────────────────────────────────────

/// Options controlling which phases a cleanup pass should execute.
#[derive(Debug, Clone)]
pub struct CleanupOptions {
    /// Remove orphaned image / thumbnail cache files.
    pub clean_orphan_cache: bool,
    /// Remove expired sync tombstones (>30 days).
    pub clean_expired_tombstones: bool,
    /// Retention days for expired clipboard items.
    /// `None` means skip retention cleanup.
    /// `Some(0)` is valid but a no-op (nothing expires at day 0).
    /// `Some(days)` with `days > 0` deletes non-favorite items older than N days.
    pub retention_days: Option<u32>,
    /// Scan for and delete stale file/path items whose source is gone.
    pub clean_stale_items: bool,
}

// ── Result types ────────────────────────────────────────────────────────

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
    /// Number of stale (missing source) items removed.
    pub stale_items: u32,
    /// Deleted item IDs that had custom hotkeys and need unregistering.
    pub deleted_hotkey_item_ids: Vec<i64>,
    /// Source paths from deleted file items whose transfer associations
    /// must be refreshed on the GPUI thread.
    pub deleted_file_paths: Vec<String>,
    /// Whether cleanup wrote sync tombstones and should trigger a sync push.
    pub sync_dirty: bool,
}

impl CleanupStats {
    pub fn is_empty(&self) -> bool {
        self.orphan_images == 0
            && self.unreferenced_icons == 0
            && self.expired_tombstones == 0
            && self.expired_items == 0
            && self.stale_items == 0
    }
}

/// A candidate record from the database for stale-item scanning.
#[derive(Debug, Clone)]
pub struct StaleItemCandidate {
    pub id: i64,
    pub content_hash: u64,
    pub updated_at: DateTime<Utc>,
    pub content_type: crate::core::types::ContentType,
    pub full_text: String,
    pub image_path: String,
    pub file_data: String,
    pub meta_type: String,
    pub source_app_name: String,
    pub source_app_icon: String,
    pub is_favorite: bool,
}

/// A confirmed-stale item ready for deletion, carrying only the identity
/// fields needed for the final in-transaction re-check.
#[derive(Debug, Clone)]
pub struct ConfirmedStaleItem {
    pub id: i64,
    pub content_hash: u64,
    pub expected_updated_at: DateTime<Utc>,
}

/// Result from a stale-item deletion transaction.
#[derive(Debug, Default, Clone)]
pub struct DeleteItemsResult {
    pub deleted_items: u32,
    pub deleted_hotkey_item_ids: Vec<i64>,
    pub deleted_file_paths: Vec<String>,
    pub tombstones_written: u32,
}

/// Result from the clear-clipboard-history database transaction.
#[derive(Debug, Default, Clone)]
pub struct ClearClipboardResult {
    pub deleted_items: u32,
    pub deleted_favorites: u32,
    pub deleted_hotkey_item_ids: Vec<i64>,
    pub deleted_file_paths: Vec<String>,
    pub tombstones_written: u32,
}

/// Aggregate stats returned to the UI after a clear-clipboard operation
/// (includes the post-clear cache maintenance phase).
#[derive(Debug, Default, Clone)]
pub struct ClearClipboardStats {
    pub deleted_items: u32,
    pub deleted_favorites: u32,
    pub deleted_hotkey_item_ids: Vec<i64>,
    pub deleted_file_paths: Vec<String>,
    pub orphan_images: u32,
    pub unreferenced_icons: u32,
    pub sync_dirty: bool,
}

/// Sync settings needed when cleanup deletes live clipboard items.
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

/// Remove icon cache files that are no longer referenced by any clipboard
/// item in the database.
fn clean_unreferenced_icons(db: &Database) -> u32 {
    let images_dir = crate::core::paths::images_dir();
    let icon_dirs = [images_dir.join("icons"), images_dir.join("file_icons")];

    let referenced: HashSet<String> = match db.get_all_referenced_icon_keys() {
        Ok(keys) => keys
            .into_iter()
            .map(|k| {
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
) -> (u32, bool, Vec<i64>, Vec<String>) {
    match db.prune_expired_items_with_sync_scope(retention_days, sync_scope) {
        Ok((items, tombstones_written)) => {
            let hotkey_item_ids = items
                .iter()
                .filter(|item| !item.custom_hotkey.is_empty())
                .map(|item| item.id)
                .collect();
            let deleted_file_paths = items
                .iter()
                .filter(|item| item.content_type == crate::core::types::ContentType::File)
                .flat_map(|item| {
                    crate::core::types::FileData::from_json(&item.file_data)
                        .files
                        .into_iter()
                        .map(|file| file.path)
                })
                .collect();
            (
                items.len() as u32,
                tombstones_written > 0,
                hotkey_item_ids,
                deleted_file_paths,
            )
        }
        Err(e) => {
            log::error!("clean_expired_clipboard_items: {e}");
            (0, false, Vec::new(), Vec::new())
        }
    }
}

/// Query the database for file/path item candidates that might be stale.
pub fn find_stale_item_candidates(db: &Database) -> Vec<StaleItemCandidate> {
    match db.find_stale_item_candidates() {
        Ok(candidates) => candidates,
        Err(e) => {
            log::error!("find_stale_item_candidates: {e}");
            Vec::new()
        }
    }
}

/// Perform filesystem verification on a list of candidates and return only
/// the items that are confirmed to have missing source files.
///
/// This function runs fresh filesystem checks (no caching) and distinguishes
/// "definitely missing" from "inaccessible / permission-denied / offline".
pub fn verify_stale_candidates(candidates: &[StaleItemCandidate]) -> Vec<ConfirmedStaleItem> {
    let mut confirmed: Vec<ConfirmedStaleItem> = Vec::new();

    for candidate in candidates {
        if candidate.is_favorite {
            continue; // Never auto-delete favorites.
        }

        let is_stale = check_candidate_stale(candidate);

        if is_stale {
            confirmed.push(ConfirmedStaleItem {
                id: candidate.id,
                content_hash: candidate.content_hash,
                expected_updated_at: candidate.updated_at,
            });
        }
    }

    confirmed
}

/// Check whether a single candidate's source files/paths are all missing.
pub(crate) fn check_candidate_stale(candidate: &StaleItemCandidate) -> bool {
    use crate::core::types::{ContentType, FileData};

    match candidate.content_type {
        ContentType::File => {
            let file_data = FileData::from_json(&candidate.file_data);
            if file_data.is_transfer() {
                return false;
            }
            let paths = file_data
                .files
                .into_iter()
                .map(|file| file.path)
                .collect::<Vec<_>>();

            if paths.is_empty() {
                return false;
            }

            // All paths must be missing for the item to be stale.
            paths.iter().all(|p| is_path_definitely_missing(p))
        }
        ContentType::Image => {
            // Synced images do not retain local source-app metadata. A missing
            // managed image in that state may still be downloaded later.
            if candidate.source_app_name.is_empty() && candidate.source_app_icon.is_empty() {
                return false;
            }
            if candidate.image_path.is_empty()
                || !std::path::Path::new(&candidate.image_path)
                    .starts_with(crate::core::paths::images_dir())
            {
                return false;
            }
            is_path_definitely_missing(&candidate.image_path)
        }
        _ => {
            // For path-type items, check the text as a path.
            if candidate.meta_type == "path" && !candidate.full_text.is_empty() {
                if !crate::core::types::path_is_native(&candidate.full_text) {
                    return false;
                }
                is_path_definitely_missing(&candidate.full_text)
            } else {
                false
            }
        }
    }
}

/// Returns `true` when the path refers to a location on a reachable local
/// volume but the file/directory itself no longer exists.
///
/// Returns `false` for:
/// - Paths that still exist.
/// - Network / UNC paths.
/// - Paths on unmounted or inaccessible volumes.
/// - Permission errors (we can't distinguish "no access" from "missing").
fn is_path_definitely_missing(path_str: &str) -> bool {
    let path = std::path::Path::new(path_str);

    if crate::platform::remote_path::remote_host_label(path_str).is_some()
        || !path.is_absolute()
        || !crate::core::types::path_is_native(path_str)
    {
        return false;
    }

    let Some(anchor) = local_volume_anchor(path_str) else {
        return false;
    };
    if std::fs::metadata(&anchor).is_err() {
        return false;
    }

    match std::fs::metadata(path) {
        Ok(_) => false,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => {
            log::debug!("path status unknown for {path_str}: {error}");
            false
        }
    }
}

/// Detect whether a path belongs to a different platform (e.g. macOS path
/// on Windows, or Windows path on macOS).
fn local_volume_anchor(path_str: &str) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let bytes = path_str.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/')
        {
            use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;

            let root = format!("{}\\", &path_str[..2]);
            let root_wide = root
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            let drive_type = unsafe { GetDriveTypeW(root_wide.as_ptr()) };
            // GetDriveTypeW: 0 = unknown, 1 = no root, 4 = remote.
            if drive_type <= 1 || drive_type == 4 {
                return None;
            }
            return Some(std::path::PathBuf::from(root));
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        let path = std::path::Path::new(path_str);
        if let Ok(relative) = path.strip_prefix("/Volumes") {
            let volume = relative.components().next()?.as_os_str();
            return Some(std::path::Path::new("/Volumes").join(volume));
        }
        Some(std::path::PathBuf::from("/"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = path_str;
        Some(std::path::PathBuf::from("/"))
    }
}

/// Delete confirmed-stale items in batches, writing sync tombstones.
/// Returns aggregated stats. Performs in-transaction re-check before each
/// batch delete.
pub fn clean_stale_items(
    db: &Database,
    confirmed: &[ConfirmedStaleItem],
    sync_scope: Option<&CleanupSyncScope>,
) -> DeleteItemsResult {
    if confirmed.is_empty() {
        return DeleteItemsResult::default();
    }

    match db.delete_stale_items(confirmed, sync_scope) {
        Ok(result) => result,
        Err(e) => {
            log::error!("clean_stale_items: {e}");
            DeleteItemsResult::default()
        }
    }
}

// ── Unified entry points ──────────────────────────────────────────────

/// Run cleanup with explicit options. This is the preferred entry point.
pub fn run_cleanup_with_options(
    db: &Database,
    options: &CleanupOptions,
    sync_scope: Option<&CleanupSyncScope>,
) -> CleanupStats {
    let mut stats = CleanupStats::default();

    // Phase 1: Orphan cache cleanup.
    if options.clean_orphan_cache {
        stats.orphan_images = clean_orphan_images(db);
        stats.unreferenced_icons = clean_unreferenced_icons(db);
    }

    // Phase 2: Expired tombstones.
    if options.clean_expired_tombstones {
        stats.expired_tombstones = clean_expired_tombstones(db);
    }

    // Phase 3: Retention-based expiry.
    if let Some(retention_days) = options.retention_days {
        if retention_days > 0 {
            let (expired, sync_dirty, hotkey_ids, deleted_file_paths) =
                clean_expired_clipboard_items(db, retention_days, sync_scope);
            stats.expired_items = expired;
            stats.sync_dirty = stats.sync_dirty || sync_dirty;
            stats.deleted_hotkey_item_ids.extend(hotkey_ids);
            stats.deleted_file_paths.extend(deleted_file_paths);
        }
    }

    // Phase 4: Stale item cleanup.
    if options.clean_stale_items {
        let candidates = find_stale_item_candidates(db);
        if !candidates.is_empty() {
            let confirmed = verify_stale_candidates(&candidates);
            if !confirmed.is_empty() {
                let result = clean_stale_items(db, &confirmed, sync_scope);
                stats.stale_items = result.deleted_items;
                stats.sync_dirty = stats.sync_dirty || result.tombstones_written > 0;
                stats
                    .deleted_hotkey_item_ids
                    .extend(result.deleted_hotkey_item_ids);
                stats.deleted_file_paths.extend(result.deleted_file_paths);
            }
        }
    }

    if !stats.is_empty() {
        log::info!(
            "run_cleanup_with_options: {} orphan images, {} unreferenced icons, {} expired tombstones, {} expired items, {} stale items",
            stats.orphan_images,
            stats.unreferenced_icons,
            stats.expired_tombstones,
            stats.expired_items,
            stats.stale_items,
        );
    }

    stats
}

/// Run a cache-maintenance-only phase (orphan images, icons, expired tombstones)
/// without retention or stale-item cleanup. Used after clear-clipboard to remove
/// now-orphaned cache files.
pub fn run_cache_maintenance(db: &Database) -> (u32, u32, u32) {
    let orphan_images = clean_orphan_images(db);
    let unreferenced_icons = clean_unreferenced_icons(db);
    let expired_tombstones = clean_expired_tombstones(db);
    (orphan_images, unreferenced_icons, expired_tombstones)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{ContentType, FileData, FileInfo};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "clippi-cleanup-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file_candidate(paths: &[std::path::PathBuf]) -> StaleItemCandidate {
        let files = paths
            .iter()
            .map(|path| FileInfo {
                name: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path: path.to_string_lossy().into_owned(),
                is_dir: false,
            })
            .collect();
        StaleItemCandidate {
            id: 1,
            content_hash: 1,
            updated_at: Utc::now(),
            content_type: ContentType::File,
            full_text: String::new(),
            image_path: String::new(),
            file_data: FileData {
                files,
                transfer: false,
                remote_hash: String::new(),
            }
            .to_json(),
            meta_type: String::new(),
            source_app_name: String::new(),
            source_app_icon: String::new(),
            is_favorite: false,
        }
    }

    #[test]
    fn file_data_paths_are_used_for_stale_detection() {
        let dir = temp_dir("file-data");
        let missing = dir.join("missing.txt");
        let candidate = file_candidate(std::slice::from_ref(&missing));

        assert!(check_candidate_stale(&candidate));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn multi_file_item_is_kept_when_any_source_exists() {
        let dir = temp_dir("multi");
        let existing = dir.join("existing.txt");
        let missing = dir.join("missing.txt");
        std::fs::write(&existing, b"test").unwrap();
        let candidate = file_candidate(&[missing, existing]);

        assert!(!check_candidate_stale(&candidate));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_local_image_cache_is_stale_but_synced_image_is_not() {
        let missing_image = crate::core::paths::images_dir().join(format!(
            "missing-local-image-{}-{}.png",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        assert!(!missing_image.exists());

        let mut candidate = StaleItemCandidate {
            id: 1,
            content_hash: 1,
            updated_at: Utc::now(),
            content_type: ContentType::Image,
            full_text: String::new(),
            image_path: missing_image.to_string_lossy().into_owned(),
            file_data: String::new(),
            meta_type: String::new(),
            source_app_name: "Local App".to_string(),
            source_app_icon: String::new(),
            is_favorite: false,
        };

        assert!(check_candidate_stale(&candidate));

        candidate.source_app_name.clear();
        assert!(!check_candidate_stale(&candidate));
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn inaccessible_or_unmounted_anchor_is_not_confirmed_stale() {
        #[cfg(target_os = "windows")]
        let path = {
            let Some(path) = (b'D'..=b'Z')
                .rev()
                .map(|drive| format!("{}:\\clippi-test\\missing.txt", drive as char))
                .find(|candidate| {
                    local_volume_anchor(candidate)
                        .is_some_and(|anchor| std::fs::metadata(anchor).is_err())
                })
            else {
                return;
            };
            path
        };
        #[cfg(not(target_os = "windows"))]
        let path = format!(
            "/Volumes/clippi-volume-that-does-not-exist-{}/missing.txt",
            std::process::id()
        );

        assert!(!is_path_definitely_missing(&path));
    }

    #[test]
    fn unc_network_path_is_never_confirmed_stale() {
        assert!(!is_path_definitely_missing(
            r"\\nas.example\share\missing.txt"
        ));
    }
}
