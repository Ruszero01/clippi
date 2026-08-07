//! --- Cache cleanup — removes orphaned image files, expired icon caches, ---
//! --- expired sync tombstones, expired clipboard items, and stale items. ---
//!
//! Each cleanup task is a standalone function. `run_cleanup_with_options`
//! orchestrates all of them; callers go through that single entry point.
//!
//! Stale-item cleanup follows the four-state design (docs/stale-item-cleanup-design.md):
//! paths are classified as Present / DefinitelyMissing / Unknown / Protected
//! instead of a `bool`, missing observations are persisted per item, and only
//! items with two consecutive missing observations beyond a grace period are
//! eligible for deletion. No existence evidence at capture time means an item
//! is never auto-deleted.

use crate::core::db::Database;
use crate::core::types::{PathObjectKind, PathStatus, PathStatusReason};
use chrono::{DateTime, Duration, Utc};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// 30-day expiry for sync tombstones.
const TOMBSTONE_EXPIRY_DAYS: i64 = 30;

/// Minimum consecutive missing observations before stale deletion.
const STALE_MIN_OBSERVATIONS: u32 = 2;

/// Grace period between the first missing observation and deletion eligibility.
const STALE_GRACE_PERIOD: Duration = Duration::hours(24);

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
    /// Number of candidates scanned in the stale phase.
    pub stale_scanned: u32,
    /// Candidates whose paths are all present.
    pub stale_present: u32,
    /// Candidates observed missing for the first time this cycle.
    pub stale_first_missing: u32,
    /// Candidates observed missing but not yet eligible (observation window).
    pub stale_pending_confirmation: u32,
    /// Candidates eligible for deletion this cycle (sent to the delete step).
    pub stale_eligible: u32,
    /// Candidates kept because of a protection reason (favorite, sync, remote,
    /// removable volume, unknown origin, ...).
    pub stale_protected: u32,
    /// Candidates kept because the path could not be verified.
    pub stale_unknown: u32,
    /// Candidate rows skipped because item metadata was unparseable.
    pub invalid_metadata: u32,
    /// Cache files that failed to remove (locked, permission, transient I/O).
    pub cache_remove_failed: u32,
    /// True when every requested maintenance phase completed without failure.
    /// Only then may the success markers (cleanup_last_date, ...) advance.
    pub scan_complete: bool,
    /// Whether the retention deletion phase failed.
    pub retention_failed: bool,
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
    pub is_favorite: bool,
    /// RFC3339 timestamp of the first capture-time existence observation.
    /// Empty means no evidence — the item can never be auto-deleted.
    pub existence_observed_at: String,
    /// Whether the image row is sync-owned with a blob that may not be
    /// downloaded yet (set by sync merge, cleared once the blob is present).
    pub sync_pending: bool,
    /// Persisted missing-observation state for this item, if any.
    pub observation: Option<StaleObservation>,
}

/// Aggregate classification of a whole candidate item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    /// At least one path exists, or the item carries no path at all.
    Present,
    /// Every path is definitely missing and all gates passed.
    DefinitelyMissing,
    /// Currently unverifiable — never deleted.
    Unknown { reason: PathStatusReason },
    /// Known safe reason to keep the item.
    Protected { reason: PathStatusReason },
}

/// Persisted per-item missing-observation state (local only, never synced).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleObservation {
    pub item_id: i64,
    pub content_hash: u64,
    pub item_updated_at: DateTime<Utc>,
    pub first_missing_at: DateTime<Utc>,
    pub last_checked_at: DateTime<Utc>,
    pub consecutive_missing_count: u32,
    pub last_status: String,
    pub last_reason: String,
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
    /// Cache files that failed to remove during post-clear maintenance.
    pub cache_remove_failed: u32,
    /// True when the post-clear cache maintenance completed without failure.
    pub scan_complete: bool,
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
fn clean_orphan_images(db: &Database, stats: &mut CleanupStats) {
    clean_orphan_images_in(db, &crate::core::paths::images_dir(), stats)
}

/// Same as `clean_orphan_images` but against an explicit cache directory
/// (used by tests to avoid touching the real images directory).
fn clean_orphan_images_in(db: &Database, images_dir: &Path, stats: &mut CleanupStats) {
    if !images_dir.exists() {
        return; // Nothing to scan — complete.
    }

    // A failed reference query must not delete anything and must not let the
    // success marker advance.
    let referenced: HashSet<String> = match db.get_all_image_hashes() {
        Ok(hashes) => hashes.into_iter().collect(),
        Err(error) => {
            log::error!("clean_orphan_images: failed to query image hashes: {error}");
            stats.scan_complete = false;
            return;
        }
    };

    let entries = match fs::read_dir(images_dir) {
        Ok(entries) => entries,
        Err(error) => {
            log::error!(
                "clean_orphan_images: failed to read {}: {error}",
                images_dir.display()
            );
            stats.scan_complete = false;
            return;
        }
    };

    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(error) => {
                log::warn!("clean_orphan_images: skipped directory entry: {error}");
                stats.scan_complete = false;
                continue;
            }
        };
        if path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // Strict naming: only managed `<hash>.png` and `thumb_<hash>.png`
        // files are eligible. In-flight `thumb_<hash>.<unique>.tmp.png` files
        // and unrelated PNGs are never touched (design §12.1).
        let Some(hash) = image_hash_from_file_name(name) else {
            continue;
        };
        if !referenced.contains(&hash) {
            match fs::remove_file(&path) {
                Ok(()) => stats.orphan_images += 1,
                Err(error) => {
                    log::warn!("clean_orphan_images: failed to remove {name}: {error}");
                    stats.cache_remove_failed += 1;
                }
            }
        }
    }
}

/// Extract the content hash from a strictly-formed cache file name.
///
/// Managed originals may be `png`/`jpg`/`jpeg` (sync downloads support all
/// three); thumbnails are always `thumb_<hash>.png`. Everything else —
/// in-flight `thumb_<hash>.<unique>.tmp.png` files, unrelated PNGs — is
/// never touched (design §12.1).
fn image_hash_from_file_name(name: &str) -> Option<String> {
    let (core, ext) = name.rsplit_once('.')?;
    if let Some(core) = core.strip_prefix("thumb_") {
        // Thumbnails are always `thumb_<hash>.png`; JPEG thumbnails are
        // not managed cache files.
        if ext != "png" {
            return None;
        }
        is_lower_hex_hash(core).then(|| core.to_string())
    } else {
        // Managed originals may be png / jpg / jpeg (sync downloads support
        // all three).
        if !matches!(ext, "png" | "jpg" | "jpeg") {
            return None;
        }
        is_lower_hex_hash(core).then(|| core.to_string())
    }
}

fn is_lower_hex_hash(core: &str) -> bool {
    core.len() == 16
        && core
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Remove icon cache files that are no longer referenced by any clipboard
/// item in the database.
fn clean_unreferenced_icons(db: &Database, stats: &mut CleanupStats) {
    clean_unreferenced_icons_in(db, &crate::core::paths::images_dir(), stats)
}

/// Same as `clean_unreferenced_icons` but against an explicit cache directory.
fn clean_unreferenced_icons_in(db: &Database, images_dir: &Path, stats: &mut CleanupStats) {
    let icon_dirs = [images_dir.join("icons"), images_dir.join("file_icons")];

    let referenced: HashSet<String> = match db.get_all_referenced_icon_keys() {
        Ok(keys) => keys
            .into_iter()
            .map(|key| {
                if let Some(rest) = key.strip_prefix("icons/") {
                    rest.to_string()
                } else if let Some(rest) = key.strip_prefix("file_icons/") {
                    rest.to_string()
                } else {
                    key
                }
            })
            .collect(),
        Err(error) => {
            log::error!("clean_unreferenced_icons: failed to query icon keys: {error}");
            stats.scan_complete = false;
            return;
        }
    };

    for icons_dir in icon_dirs.iter().filter(|dir| dir.exists()) {
        let entries = match fs::read_dir(icons_dir) {
            Ok(entries) => entries,
            Err(error) => {
                log::error!(
                    "clean_unreferenced_icons: failed to read {}: {error}",
                    icons_dir.display()
                );
                stats.scan_complete = false;
                continue;
            }
        };
        for entry in entries {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(error) => {
                    log::warn!("clean_unreferenced_icons: skipped directory entry: {error}");
                    stats.scan_complete = false;
                    continue;
                }
            };
            if !path.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            let stem = name.strip_suffix(".png").unwrap_or(name);
            if !referenced.contains(stem) {
                match fs::remove_file(&path) {
                    Ok(()) => stats.unreferenced_icons += 1,
                    Err(error) => {
                        log::warn!("clean_unreferenced_icons: failed to remove {name}: {error}");
                        stats.cache_remove_failed += 1;
                    }
                }
            }
        }
    }
}

/// Remove sync tombstones older than `TOMBSTONE_EXPIRY_DAYS` days.
fn clean_expired_tombstones(db: &Database, stats: &mut CleanupStats) {
    match db.cleanup_old_tombstones(TOMBSTONE_EXPIRY_DAYS) {
        Ok(deleted) => stats.expired_tombstones = deleted,
        Err(error) => {
            log::error!("clean_expired_tombstones: {error}");
            stats.scan_complete = false;
        }
    }
}

/// Remove non-favorite clipboard items older than `retention_days`.
fn clean_expired_clipboard_items(
    db: &Database,
    retention_days: u32,
    sync_scope: Option<&CleanupSyncScope>,
    stats: &mut CleanupStats,
) {
    match db.prune_expired_items_with_sync_scope(retention_days, sync_scope) {
        Ok((items, tombstones_written)) => {
            let hotkey_item_ids: Vec<i64> = items
                .iter()
                .filter(|item| !item.custom_hotkey.is_empty())
                .map(|item| item.id)
                .collect();
            let deleted_file_paths: Vec<String> = items
                .iter()
                .filter(|item| item.content_type == crate::core::types::ContentType::File)
                .flat_map(|item| {
                    crate::core::types::FileData::from_json(&item.file_data)
                        .files
                        .into_iter()
                        .map(|file| file.path)
                })
                .collect();
            stats.expired_items = items.len() as u32;
            stats.sync_dirty = stats.sync_dirty || tombstones_written > 0;
            stats.deleted_hotkey_item_ids.extend(hotkey_item_ids);
            stats.deleted_file_paths.extend(deleted_file_paths);
        }
        Err(error) => {
            log::error!("clean_expired_clipboard_items: {error}");
            stats.retention_failed = true;
        }
    }
}

// ── Four-state path classification ────────────────────────────────────

/// Probe a single filesystem path and return its four-state status.
///
/// Guards, in order:
/// 1. Remote / network paths are never probed.
/// 2. Non-absolute or non-native paths cannot be verified.
/// 3. The volume must be a stable, reachable local volume (Windows: fixed
///    drives only; removable / optical / RAM-disk volumes are protected).
/// 4. `symlink_metadata` — a symlink whose target is unavailable still counts
///    as existing (the link object itself is a valid filesystem object).
/// 5. Only `NotFound` counts as definitely missing; permission and other
///    errors are Unknown.
pub(crate) fn probe_path_status(path_str: &str) -> PathStatus {
    let path = Path::new(path_str);

    if crate::platform::remote_path::remote_host_label(path_str).is_some() {
        return PathStatus::Protected {
            reason: PathStatusReason::RemotePath,
        };
    }
    if !path.is_absolute() {
        return PathStatus::Unknown {
            reason: PathStatusReason::InvalidMetadata,
        };
    }
    if !crate::core::types::path_is_native(path_str) {
        return PathStatus::Protected {
            reason: PathStatusReason::ForeignPlatform,
        };
    }

    // Volume-level guards come before the target probe (design §11 stage C).
    if let Err(reason) = local_volume_status(path_str) {
        return PathStatus::Protected { reason };
    }

    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            let kind = if metadata.is_dir() {
                PathObjectKind::Directory
            } else if metadata.file_type().is_symlink() {
                PathObjectKind::Symlink
            } else {
                PathObjectKind::File
            };
            PathStatus::Present { kind }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // `NotFound` is only trustworthy when the parent directory is
            // reachable. A missing or unreachable parent (cloud provider
            // subtree temporarily gone, offline sync root, unmounted
            // directory, ...) must stay Unknown (design §10.1.7 / §17.1.5).
            match path.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => {
                    match fs::symlink_metadata(parent) {
                        Ok(metadata) if is_link_like(&metadata) => {
                            // The parent is a symlink / junction / mount
                            // point / cloud reparse point. The object itself
                            // existing is not enough: a broken link or an
                            // offline provider root must not certify the
                            // target as missing.
                            if parent_link_is_reachable(parent) {
                                PathStatus::DefinitelyMissing
                            } else {
                                PathStatus::Unknown {
                                    reason: PathStatusReason::ParentUnavailable,
                                }
                            }
                        }
                        Ok(_) => PathStatus::DefinitelyMissing,
                        Err(_) => PathStatus::Unknown {
                            reason: PathStatusReason::ParentUnavailable,
                        },
                    }
                }
                _ => PathStatus::DefinitelyMissing,
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => PathStatus::Unknown {
            reason: PathStatusReason::PermissionDenied,
        },
        Err(_) => PathStatus::Unknown {
            reason: PathStatusReason::IoError,
        },
    }
}

/// Whether a directory entry is a link-like object whose target must be
/// followed before its contents can be trusted.
///
/// On Windows this explicitly checks `FILE_ATTRIBUTE_REPARSE_POINT` (covers
/// symlinks, junctions, mount points and Cloud Files placeholders) instead of
/// relying on `FileType::is_symlink`, which is not guaranteed to cover every
/// reparse type.
fn is_link_like(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// Whether a link-like parent directory's target is reachable enough to
/// trust a child `NotFound`.
///
/// On Windows the reparse tag is checked first: only symlinks and mount
/// points (junctions) are followed. Cloud placeholders and unknown reparse
/// tags are never trusted to certify a child as missing — provider
/// availability cannot be verified through metadata alone (design §10.1.6).
/// A failed tag read is treated conservatively as unreachable.
fn parent_link_is_reachable(parent: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        // IO_REPARSE_TAG_SYMLINK = 0xA000000C, IO_REPARSE_TAG_MOUNT_POINT = 0xA0000003.
        const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
        const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
        match windows_reparse_tag(parent) {
            Some(tag) if tag == IO_REPARSE_TAG_SYMLINK || tag == IO_REPARSE_TAG_MOUNT_POINT => {}
            _ => return false,
        }
    }
    fs::metadata(parent).is_ok()
}

/// Read the reparse tag of a link/reparse object without following it.
#[cfg(target_os = "windows")]
fn windows_reparse_tag(path: &Path) -> Option<u32> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileAttributeTagInfo, GetFileInformationByHandleEx, FILE_ATTRIBUTE_TAG_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    // Open without following the reparse point so the tag of the object
    // itself is reported, not its target's. `access_mode(0)` requests a
    // query-only handle (no read/write/append access) — the standard
    // library would otherwise reject the open with InvalidInput.
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .ok()?;
    let handle = file.as_raw_handle();
    let mut info: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileAttributeTagInfo,
            &mut info as *mut _ as *mut _,
            std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    (ok != 0).then_some(info.ReparseTag)
}

/// Verify that the volume containing `path_str` is a stable, reachable local
/// volume. Returns `Err(reason)` when the volume cannot be trusted for
/// automatic deletion.
fn local_volume_status(path_str: &str) -> Result<(), PathStatusReason> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;

        let bytes = path_str.as_bytes();
        if !(bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'))
        {
            // Extended-prefix / device / other non-drive-letter forms cannot
            // be verified as stable local volumes.
            return Err(PathStatusReason::InvalidMetadata);
        }
        let root = format!("{}\\", &path_str[..2]);
        let root_wide = root
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let drive_type = unsafe { GetDriveTypeW(root_wide.as_ptr()) };
        // 0 = DRIVE_UNKNOWN, 1 = DRIVE_NO_ROOT_DIR, 2 = DRIVE_REMOVABLE,
        // 3 = DRIVE_FIXED, 4 = DRIVE_REMOTE, 5 = DRIVE_CDROM, 6 = DRIVE_RAMDISK.
        match drive_type {
            0 | 1 => return Err(PathStatusReason::VolumeOffline),
            // Removable media, optical drives and RAM disks have no stable
            // volume identity — auto-deletion stays disabled (design §10.1).
            2 | 5 | 6 => return Err(PathStatusReason::RemovableVolume),
            4 => return Err(PathStatusReason::RemotePath),
            _ => {} // DRIVE_FIXED (3) — the only allowed type by default.
        }
        if std::fs::metadata(&root).is_err() {
            return Err(PathStatusReason::VolumeOffline);
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let path = Path::new(path_str);
        if let Ok(relative) = path.strip_prefix("/Volumes") {
            let volume = relative
                .components()
                .next()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .unwrap_or_default();
            if volume.is_empty() {
                return Err(PathStatusReason::InvalidMetadata);
            }
            let anchor = Path::new("/Volumes").join(volume);
            if std::fs::metadata(&anchor).is_err() {
                return Err(PathStatusReason::VolumeOffline);
            }
            return Ok(());
        }
        if std::fs::metadata("/").is_err() {
            return Err(PathStatusReason::VolumeOffline);
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        // Linux platform implementation is a stub; `classify_item_status`
        // keeps auto-deletion disabled there before any probe runs.
        let _ = path_str;
        Ok(())
    }
}

/// Convert a single-path probe result to the item-level status.
fn item_status_from_path(status: PathStatus) -> ItemStatus {
    match status {
        PathStatus::Present { .. } => ItemStatus::Present,
        PathStatus::DefinitelyMissing => ItemStatus::DefinitelyMissing,
        PathStatus::Unknown { reason } => ItemStatus::Unknown { reason },
        PathStatus::Protected { reason } => ItemStatus::Protected { reason },
    }
}

/// Aggregate multiple path statuses (design §9.3):
/// any Present → Present; otherwise any Unknown/Protected → first blocking
/// status; only all DefinitelyMissing → DefinitelyMissing.
fn aggregate_path_statuses(paths: &[String]) -> ItemStatus {
    let mut blocking: Option<ItemStatus> = None;
    for path in paths {
        match probe_path_status(path) {
            PathStatus::Present { .. } => return ItemStatus::Present,
            PathStatus::DefinitelyMissing => {}
            PathStatus::Unknown { reason } => {
                blocking.get_or_insert(ItemStatus::Unknown { reason });
            }
            PathStatus::Protected { reason } => {
                blocking.get_or_insert(ItemStatus::Protected { reason });
            }
        }
    }
    blocking.unwrap_or(ItemStatus::DefinitelyMissing)
}

/// Classify a whole candidate item. This is the single classifier used by the
/// stale scan, the final in-transaction re-check and (in later phases) the UI.
pub(crate) fn classify_item_status(candidate: &StaleItemCandidate) -> ItemStatus {
    use crate::core::types::{ContentType, FileData};

    if candidate.is_favorite {
        return ItemStatus::Protected {
            reason: PathStatusReason::Favorite,
        };
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        // Linux platform support is still a stub (design §10.3): keep
        // auto-deletion disabled until mount / device-identity checks exist.
        let _ = candidate;
        return ItemStatus::Protected {
            reason: PathStatusReason::UnknownPlatform,
        };
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    match candidate.content_type {
        ContentType::File => {
            let file_data = FileData::from_json(&candidate.file_data);
            if file_data.is_transfer() {
                return ItemStatus::Protected {
                    reason: PathStatusReason::TransferItem,
                };
            }
            let paths: Vec<String> = file_data.files.into_iter().map(|file| file.path).collect();
            if paths.is_empty() {
                return ItemStatus::Unknown {
                    reason: PathStatusReason::InvalidMetadata,
                };
            }
            // No capture-time existence evidence → legacy/unknown origin.
            if candidate.existence_observed_at.is_empty() {
                return ItemStatus::Protected {
                    reason: PathStatusReason::OriginUnknown,
                };
            }
            aggregate_path_statuses(&paths)
        }
        ContentType::Image => {
            if candidate.image_path.is_empty() {
                return ItemStatus::Unknown {
                    reason: PathStatusReason::InvalidMetadata,
                };
            }
            if Path::new(&candidate.image_path).starts_with(crate::core::paths::images_dir()) {
                // Managed image: Clippi wrote the cache file itself, so no
                // capture-time evidence gate applies.
                if candidate.sync_pending {
                    // Sync-owned image: the blob may still be downloaded by a
                    // later sync pass. A present file means the download has
                    // completed; anything else stays protected.
                    return match probe_path_status(&candidate.image_path) {
                        PathStatus::Present { .. } => ItemStatus::Present,
                        _ => ItemStatus::Protected {
                            reason: PathStatusReason::PendingSync,
                        },
                    };
                }
                item_status_from_path(probe_path_status(&candidate.image_path))
            } else {
                // External image path (e.g. screenshot tool temp file).
                if candidate.existence_observed_at.is_empty() {
                    return ItemStatus::Protected {
                        reason: PathStatusReason::OriginUnknown,
                    };
                }
                item_status_from_path(probe_path_status(&candidate.image_path))
            }
        }
        _ => {
            if candidate.meta_type == "path" && !candidate.full_text.is_empty() {
                if candidate.existence_observed_at.is_empty() {
                    return ItemStatus::Protected {
                        reason: PathStatusReason::NeverObservedExisting,
                    };
                }
                item_status_from_path(probe_path_status(&candidate.full_text))
            } else {
                ItemStatus::Unknown {
                    reason: PathStatusReason::InvalidMetadata,
                }
            }
        }
    }
}

// ── Observation state machine ─────────────────────────────────────────

/// Update the persisted observation for a DefinitelyMissing candidate and
/// return the refreshed observation. A changed item identity (hash or
/// updated_at) restarts the observation from scratch.
fn apply_missing_observation(
    candidate: &StaleItemCandidate,
    now: DateTime<Utc>,
) -> StaleObservation {
    let existing = candidate.observation.as_ref();
    let identity_changed = existing.is_none_or(|observation| {
        observation.content_hash != candidate.content_hash
            || observation.item_updated_at != candidate.updated_at
    });
    if identity_changed {
        StaleObservation {
            item_id: candidate.id,
            content_hash: candidate.content_hash,
            item_updated_at: candidate.updated_at,
            first_missing_at: now,
            last_checked_at: now,
            consecutive_missing_count: 1,
            last_status: "missing".to_string(),
            last_reason: String::new(),
        }
    } else if let Some(mut observation) = existing.cloned() {
        observation.consecutive_missing_count =
            observation.consecutive_missing_count.saturating_add(1);
        observation.last_checked_at = now;
        observation.last_status = "missing".to_string();
        observation.last_reason = String::new();
        observation
    } else {
        StaleObservation {
            item_id: candidate.id,
            content_hash: candidate.content_hash,
            item_updated_at: candidate.updated_at,
            first_missing_at: now,
            last_checked_at: now,
            consecutive_missing_count: 1,
            last_status: "missing".to_string(),
            last_reason: String::new(),
        }
    }
}

fn status_labels(status: &ItemStatus) -> (String, String) {
    match status {
        ItemStatus::Present => ("present".to_string(), String::new()),
        ItemStatus::DefinitelyMissing => ("missing".to_string(), String::new()),
        ItemStatus::Unknown { reason } => ("unknown".to_string(), reason_label(*reason)),
        ItemStatus::Protected { reason } => ("protected".to_string(), reason_label(*reason)),
    }
}

/// Persist the reason behind an Unknown/Protected result for observability.
/// Missing counters are never touched, and an identity change clears the
/// stale observation. Returns `false` when a database write failed so the
/// caller can mark the round incomplete.
fn persist_observation_reason(
    db: &Database,
    candidate: &StaleItemCandidate,
    status: &ItemStatus,
    now: DateTime<Utc>,
) -> bool {
    let Some(mut observation) = candidate.observation.clone() else {
        return true; // No missing history — nothing to update.
    };
    if observation.content_hash != candidate.content_hash
        || observation.item_updated_at != candidate.updated_at
    {
        // Identity changed: the observation belongs to an earlier version.
        return db.clear_stale_observation(candidate.id).is_ok();
    }
    observation.last_checked_at = now;
    (observation.last_status, observation.last_reason) = status_labels(status);
    db.save_stale_observation(&observation).is_ok()
}

fn reason_label(reason: PathStatusReason) -> String {
    format!("{reason:?}")
}

// ── Stale-item scan ───────────────────────────────────────────────────

/// Run one stale-item scan: classify candidates, update the persisted
/// observation state, and delete only items that survived the double
/// confirmation and grace period. Writes sync tombstones in the same
/// transaction as each delete.
pub(crate) fn run_stale_scan(
    db: &Database,
    now: DateTime<Utc>,
    sync_scope: Option<&CleanupSyncScope>,
    stats: &mut CleanupStats,
) {
    let (candidates, skipped) = match db.find_stale_item_candidates() {
        Ok(result) => result,
        Err(error) => {
            log::error!("run_stale_scan: failed to query candidates: {error}");
            stats.scan_complete = false;
            return;
        }
    };
    stats.invalid_metadata += skipped;
    stats.stale_scanned = candidates.len() as u32;

    let mut confirmed: Vec<ConfirmedStaleItem> = Vec::new();
    for candidate in &candidates {
        let status = classify_item_status(candidate);
        match status {
            ItemStatus::Present => {
                stats.stale_present += 1;
                if let Err(error) = db.clear_stale_observation(candidate.id) {
                    log::warn!(
                        "run_stale_scan: failed to clear observation for item {}: {error}",
                        candidate.id
                    );
                    stats.scan_complete = false;
                }
            }
            ItemStatus::DefinitelyMissing => {
                let observation = apply_missing_observation(candidate, now);
                if let Err(error) = db.save_stale_observation(&observation) {
                    log::warn!(
                        "run_stale_scan: failed to save observation for item {}: {error}",
                        candidate.id
                    );
                    stats.scan_complete = false;
                    continue;
                }
                let elapsed = now.signed_duration_since(observation.first_missing_at);
                let eligible = observation.consecutive_missing_count >= STALE_MIN_OBSERVATIONS
                    && elapsed >= STALE_GRACE_PERIOD;
                if eligible {
                    stats.stale_eligible += 1;
                    confirmed.push(ConfirmedStaleItem {
                        id: candidate.id,
                        content_hash: candidate.content_hash,
                        expected_updated_at: candidate.updated_at,
                    });
                } else {
                    stats.stale_pending_confirmation += 1;
                    if observation.consecutive_missing_count == 1 {
                        stats.stale_first_missing += 1;
                    }
                }
            }
            ItemStatus::Unknown { .. } => {
                stats.stale_unknown += 1;
                if !persist_observation_reason(db, candidate, &status, now) {
                    stats.scan_complete = false;
                }
            }
            ItemStatus::Protected { .. } => {
                stats.stale_protected += 1;
                if !persist_observation_reason(db, candidate, &status, now) {
                    stats.scan_complete = false;
                }
            }
        }
    }

    if confirmed.is_empty() {
        return;
    }
    // Delete confirmed items; a database failure here must also mark the
    // round incomplete so the success markers do not advance.
    match db.delete_stale_items(&confirmed, sync_scope) {
        Ok(result) => {
            stats.stale_items = result.deleted_items;
            stats.sync_dirty = stats.sync_dirty || result.tombstones_written > 0;
            stats
                .deleted_hotkey_item_ids
                .extend(result.deleted_hotkey_item_ids);
            stats.deleted_file_paths.extend(result.deleted_file_paths);
        }
        Err(error) => {
            log::error!("run_stale_scan: delete_stale_items failed: {error}");
            stats.scan_complete = false;
        }
    }
}

// ── Unified entry points ──────────────────────────────────────────────

/// Run cleanup with explicit options. This is the preferred entry point.
///
/// Phase order matters (design §11 stage F): retention and stale database
/// deletions commit *before* orphan cache reclamation, so this round's
/// deleted items are reclaimed in the same pass.
pub fn run_cleanup_with_options(
    db: &Database,
    options: &CleanupOptions,
    sync_scope: Option<&CleanupSyncScope>,
) -> CleanupStats {
    let mut stats = CleanupStats {
        scan_complete: true,
        ..CleanupStats::default()
    };

    // Phase 1: Expired tombstones.
    if options.clean_expired_tombstones {
        clean_expired_tombstones(db, &mut stats);
    }

    // Phase 2: Retention-based expiry.
    if let Some(retention_days) = options.retention_days {
        if retention_days > 0 {
            clean_expired_clipboard_items(db, retention_days, sync_scope, &mut stats);
        }
    }

    // Phase 3: Stale item cleanup (observation-gated; deletes write tombstones).
    if options.clean_stale_items {
        run_stale_scan(db, Utc::now(), sync_scope, &mut stats);
    }

    // Phase 4: Orphan cache cleanup — AFTER database deletions.
    if options.clean_orphan_cache {
        clean_orphan_images(db, &mut stats);
        clean_unreferenced_icons(db, &mut stats);
    }

    if !stats.is_empty() {
        log::info!(
            "run_cleanup_with_options: {} orphan images, {} unreferenced icons, {} expired tombstones, {} expired items, {} stale items ({} scanned, {} pending, {} protected, {} unknown, {} invalid metadata)",
            stats.orphan_images,
            stats.unreferenced_icons,
            stats.expired_tombstones,
            stats.expired_items,
            stats.stale_items,
            stats.stale_scanned,
            stats.stale_pending_confirmation,
            stats.stale_protected,
            stats.stale_unknown,
            stats.invalid_metadata,
        );
    }

    stats
}

/// Run a cache-maintenance-only phase (orphan images, icons, expired tombstones)
/// without retention or stale-item cleanup. Used after clear-clipboard to remove
/// now-orphaned cache files.
pub fn run_cache_maintenance(db: &Database) -> CleanupStats {
    let mut stats = CleanupStats {
        scan_complete: true,
        ..CleanupStats::default()
    };
    clean_orphan_images(db, &mut stats);
    clean_unreferenced_icons(db, &mut stats);
    clean_expired_tombstones(db, &mut stats);
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db::Database;
    use crate::core::types::{ClipboardItem, ContentType, FileData, FileInfo};

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

    /// Insert an image item through the public upsert path with a controlled
    /// capture-time evidence timestamp and updated_at.
    fn insert_image_item(
        db: &Database,
        hash: u64,
        image_path: &str,
        source_app: &str,
        observed_at: &str,
        updated_at: &str,
    ) {
        let mut item = ClipboardItem::new_image(0, image_path, hash, 0, 0, None);
        item.source_app_name = source_app.to_string();
        item.existence_observed_at = observed_at.to_string();
        item.updated_at = updated_at.parse().unwrap();
        item.created_at = item.updated_at;
        db.upsert(&item).unwrap();
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
            is_favorite: false,
            existence_observed_at: Utc::now().to_rfc3339(),
            sync_pending: false,
            observation: None,
        }
    }

    fn image_candidate(image_path: &str, _source_app: &str) -> StaleItemCandidate {
        StaleItemCandidate {
            id: 2,
            content_hash: 2,
            updated_at: Utc::now(),
            content_type: ContentType::Image,
            full_text: String::new(),
            image_path: image_path.to_string(),
            file_data: String::new(),
            meta_type: String::new(),
            is_favorite: false,
            existence_observed_at: Utc::now().to_rfc3339(),
            sync_pending: false,
            observation: None,
        }
    }

    #[test]
    fn file_data_paths_are_used_for_stale_detection() {
        let dir = temp_dir("file-data");
        let missing = dir.join("missing.txt");
        let mut candidate = file_candidate(std::slice::from_ref(&missing));
        candidate.existence_observed_at = Utc::now().to_rfc3339();

        assert_eq!(
            classify_item_status(&candidate),
            ItemStatus::DefinitelyMissing
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn multi_file_item_is_kept_when_any_source_exists() {
        let dir = temp_dir("multi");
        let existing = dir.join("existing.txt");
        let missing = dir.join("missing.txt");
        std::fs::write(&existing, b"test").unwrap();
        let candidate = file_candidate(&[missing, existing]);

        assert_eq!(classify_item_status(&candidate), ItemStatus::Present);

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn file_without_capture_evidence_is_never_stale() {
        let dir = temp_dir("no-evidence");
        let missing = dir.join("missing.txt");
        let mut candidate = file_candidate(std::slice::from_ref(&missing));
        candidate.existence_observed_at.clear();

        assert_eq!(
            classify_item_status(&candidate),
            ItemStatus::Protected {
                reason: PathStatusReason::OriginUnknown
            }
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_managed_image_is_stale_but_synced_image_is_not() {
        let missing_image = crate::core::paths::images_dir().join(format!(
            "missing-local-image-{}-{}.png",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        assert!(!missing_image.exists());

        // Local managed image → definitely missing.
        let mut candidate = image_candidate(&missing_image.to_string_lossy(), "Local App");
        assert_eq!(
            classify_item_status(&candidate),
            ItemStatus::DefinitelyMissing
        );

        // Sync-owned image whose blob was never downloaded → protected,
        // regardless of the (possibly carried-over) local source metadata.
        candidate.sync_pending = true;
        assert_eq!(
            classify_item_status(&candidate),
            ItemStatus::Protected {
                reason: PathStatusReason::PendingSync
            }
        );

        // A local image without source metadata (capture-time app unknown)
        // is still a local managed image — missing means missing.
        candidate.sync_pending = false;
        assert_eq!(
            classify_item_status(&candidate),
            ItemStatus::DefinitelyMissing
        );
    }

    #[test]
    fn synced_image_with_present_blob_is_present_until_deleted() {
        // A sync-owned managed image whose blob is present (download done).
        // A random hash is used so the file can never collide with a real
        // referenced cache file; it is removed again below.
        let images_dir = crate::core::paths::images_dir();
        let hash = format!(
            "{:016x}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default() as u64
        );
        let image = images_dir.join(format!("{hash}.jpg"));
        std::fs::write(&image, b"jpg").unwrap();
        let mut candidate = image_candidate(&image.to_string_lossy(), "Local App");
        candidate.sync_pending = true;
        assert_eq!(classify_item_status(&candidate), ItemStatus::Present);

        // Blob is deleted later → still sync-owned, must not be deleted
        // locally (it may be re-downloaded by a sync pass).
        std::fs::remove_file(&image).unwrap();
        assert_eq!(
            classify_item_status(&candidate),
            ItemStatus::Protected {
                reason: PathStatusReason::PendingSync
            }
        );

        // Once the sync pass confirms the blob and clears the flag, the
        // image follows normal managed rules again.
        candidate.sync_pending = false;
        assert_eq!(
            classify_item_status(&candidate),
            ItemStatus::DefinitelyMissing
        );
    }

    #[test]
    fn external_image_requires_capture_evidence() {
        let dir = temp_dir("external-image");
        let missing = dir.join("screenshot-temp.png");
        let path = missing.to_string_lossy().into_owned();

        // Legacy record without evidence → protected, never auto-deleted.
        let mut candidate = image_candidate(&path, "SnippingTool");
        candidate.existence_observed_at.clear();
        assert_eq!(
            classify_item_status(&candidate),
            ItemStatus::Protected {
                reason: PathStatusReason::OriginUnknown
            }
        );

        // New capture with evidence → can enter the observation flow.
        candidate.existence_observed_at = Utc::now().to_rfc3339();
        assert_eq!(
            classify_item_status(&candidate),
            ItemStatus::DefinitelyMissing
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn text_path_without_evidence_is_never_deleted() {
        let dir = temp_dir("text-path");
        let missing = dir.join("future-output.txt");
        let path = missing.to_string_lossy().into_owned();
        let mut candidate = StaleItemCandidate {
            id: 3,
            content_hash: 3,
            updated_at: Utc::now(),
            content_type: ContentType::PlainText,
            full_text: path.clone(),
            image_path: String::new(),
            file_data: String::new(),
            meta_type: "path".to_string(),
            is_favorite: false,
            existence_observed_at: String::new(),
            sync_pending: false,
            observation: None,
        };

        // Never observed existing → never auto-deleted.
        assert_eq!(
            classify_item_status(&candidate),
            ItemStatus::Protected {
                reason: PathStatusReason::NeverObservedExisting
            }
        );

        // Observed existing at capture → enters the observation flow.
        candidate.existence_observed_at = Utc::now().to_rfc3339();
        assert_eq!(
            classify_item_status(&candidate),
            ItemStatus::DefinitelyMissing
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn broken_junction_parent_is_unknown_on_windows() {
        let dir = temp_dir("junction-parent");
        let link = dir.join("broken-junction");
        // Junctions (mklink /J) do not require symlink privileges; skip
        // silently if the environment cannot create one. Note: `Path::exists`
        // follows the junction target, so a *broken* junction must be checked
        // with `symlink_metadata` (the object itself).
        let broken_created = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(dir.join("gone-target"))
            .status()
            .is_ok_and(|s| s.success())
            && std::fs::symlink_metadata(&link).is_ok();
        if !broken_created {
            eprintln!("skip: junction creation failed");
            std::fs::remove_dir_all(dir).unwrap();
            return;
        }

        // A junction pointing at a non-existent target must not certify the
        // child as missing.
        let child = link.join("file.txt");
        assert_eq!(
            probe_path_status(&child.to_string_lossy()),
            PathStatus::Unknown {
                reason: PathStatusReason::ParentUnavailable
            }
        );

        // A working junction resolves → NotFound is trustworthy.
        let real = dir.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let working = dir.join("working-junction");
        let working_created = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&working)
            .arg(&real)
            .status()
            .is_ok_and(|s| s.success())
            && std::fs::symlink_metadata(&working).is_ok();
        if working_created {
            let child = working.join("missing.txt");
            assert_eq!(
                probe_path_status(&child.to_string_lossy()),
                PathStatus::DefinitelyMissing
            );
        }

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn broken_directory_symlink_parent_is_unknown_on_windows() {
        let dir = temp_dir("reparse-parent");
        let link = dir.join("broken-link");
        // Creating symlinks requires developer mode / elevated privileges;
        // skip silently when the environment forbids it.
        if std::os::windows::fs::symlink_dir(dir.join("gone-target"), &link).is_err() {
            std::fs::remove_dir_all(dir).unwrap();
            return;
        }

        // A broken directory symlink (reparse point) as parent must not
        // certify the child as missing.
        let child = link.join("file.txt");
        assert_eq!(
            probe_path_status(&child.to_string_lossy()),
            PathStatus::Unknown {
                reason: PathStatusReason::ParentUnavailable
            }
        );

        // A working directory symlink resolves → NotFound is trustworthy.
        let real = dir.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let working = dir.join("working-link");
        if std::os::windows::fs::symlink_dir(&real, &working).is_ok() {
            let child = working.join("missing.txt");
            assert_eq!(
                probe_path_status(&child.to_string_lossy()),
                PathStatus::DefinitelyMissing
            );
        }

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn broken_symlink_parent_is_unknown_but_working_symlink_parent_is_not() {
        let dir = temp_dir("symlink-parent");
        let real = dir.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let broken_link = dir.join("broken");
        std::os::unix::fs::symlink(dir.join("gone-target"), &broken_link).unwrap();
        let working_link = dir.join("working");
        std::os::unix::fs::symlink(&real, &working_link).unwrap();

        // Parent symlink target is gone → the child NotFound is not
        // trustworthy.
        let child = broken_link.join("file.txt");
        assert_eq!(
            probe_path_status(&child.to_string_lossy()),
            PathStatus::Unknown {
                reason: PathStatusReason::ParentUnavailable
            }
        );

        // Parent symlink resolves → NotFound for the child is trustworthy.
        let child = working_link.join("missing.txt");
        assert_eq!(
            probe_path_status(&child.to_string_lossy()),
            PathStatus::DefinitelyMissing
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn broken_symlink_object_itself_counts_as_present() {
        let dir = temp_dir("symlink");
        let target = dir.join("gone.txt");
        let link = dir.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(!target.exists());

        assert!(matches!(
            probe_path_status(&link.to_string_lossy()),
            PathStatus::Present {
                kind: PathObjectKind::Symlink
            }
        ));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn inaccessible_or_unmounted_anchor_is_not_confirmed_stale() {
        #[cfg(target_os = "windows")]
        let path = {
            // A drive letter with no root (not fixed, not removable) must be
            // protected, never reported as definitely missing.
            let path = "Z:\\clippi-test\\missing.txt";
            if !matches!(
                local_volume_status(path),
                Err(PathStatusReason::VolumeOffline | PathStatusReason::RemovableVolume)
            ) {
                // Z: may exist on some machines; skip rather than fail.
                return;
            }
            path.to_string()
        };
        #[cfg(not(target_os = "windows"))]
        let path = format!(
            "/Volumes/clippi-volume-that-does-not-exist-{}/missing.txt",
            std::process::id()
        );

        assert!(!matches!(
            probe_path_status(&path),
            PathStatus::DefinitelyMissing
        ));
    }

    #[test]
    fn unc_network_path_is_never_confirmed_stale() {
        assert!(matches!(
            probe_path_status(r"\\nas.example\share\missing.txt"),
            PathStatus::Protected {
                reason: PathStatusReason::RemotePath
            }
        ));
    }

    #[test]
    fn strict_cache_file_name_patterns() {
        assert_eq!(
            image_hash_from_file_name("0123456789abcdef.png"),
            Some("0123456789abcdef".to_string())
        );
        assert_eq!(
            image_hash_from_file_name("thumb_0123456789abcdef.png"),
            Some("0123456789abcdef".to_string())
        );
        // Sync downloads also produce .jpg / .jpeg blobs.
        assert_eq!(
            image_hash_from_file_name("0123456789abcdef.jpg"),
            Some("0123456789abcdef".to_string())
        );
        assert_eq!(
            image_hash_from_file_name("0123456789abcdef.jpeg"),
            Some("0123456789abcdef".to_string())
        );
        // In-flight thumbnail temp files never match.
        assert_eq!(
            image_hash_from_file_name("thumb_0123456789abcdef.123.tmp.png"),
            None
        );
        // Thumbnails are strictly PNG: JPEG thumbnails are not managed cache.
        assert_eq!(
            image_hash_from_file_name("thumb_0123456789abcdef.jpg"),
            None
        );
        assert_eq!(
            image_hash_from_file_name("thumb_0123456789abcdef.jpeg"),
            None
        );
        // Wrong length / uppercase / non-hex / unrelated files never match.
        assert_eq!(image_hash_from_file_name("abc.png"), None);
        assert_eq!(image_hash_from_file_name("0123456789ABCDEF.png"), None);
        assert_eq!(image_hash_from_file_name("0123456789abcdefg.png"), None);
        assert_eq!(image_hash_from_file_name("notes.png"), None);
        assert_eq!(image_hash_from_file_name("0123456789abcdef.gif"), None);
        assert_eq!(image_hash_from_file_name("0123456789abcdef.png.tmp"), None);
    }

    #[test]
    fn orphan_scan_removes_unreferenced_cache_but_keeps_tmp_and_unrelated_files() {
        let dir = temp_dir("orphan-strict");
        let images = dir.join("images");
        std::fs::create_dir_all(&images).unwrap();

        let (_db_path, db) = temp_database(&dir);
        // Referenced hash stays; unreferenced hashes are removed (png + jpg).
        std::fs::write(images.join("1111111111111111.png"), b"a").unwrap();
        std::fs::write(images.join("thumb_1111111111111111.png"), b"b").unwrap();
        std::fs::write(images.join("2222222222222222.png"), b"c").unwrap();
        std::fs::write(images.join("thumb_2222222222222222.png"), b"d").unwrap();
        std::fs::write(images.join("3333333333333333.jpg"), b"g").unwrap();
        std::fs::write(images.join("4444444444444444.jpeg"), b"h").unwrap();
        // In-flight tmp file and an unrelated PNG must survive.
        std::fs::write(images.join("thumb_2222222222222222.5.tmp.png"), b"e").unwrap();
        std::fs::write(images.join("notes.png"), b"f").unwrap();

        db.upsert(&{
            let mut item = ClipboardItem::new_image(
                0,
                &images.join("1111111111111111.png").to_string_lossy(),
                0x1111_1111_1111_1111,
                0,
                0,
                None,
            );
            item.existence_observed_at = Utc::now().to_rfc3339();
            item
        })
        .unwrap();

        let mut stats = CleanupStats {
            scan_complete: true,
            ..CleanupStats::default()
        };
        clean_orphan_images_in(&db, &images, &mut stats);

        assert_eq!(stats.orphan_images, 4);
        assert_eq!(stats.cache_remove_failed, 0);
        assert!(images.join("1111111111111111.png").exists());
        assert!(images.join("thumb_1111111111111111.png").exists());
        assert!(!images.join("2222222222222222.png").exists());
        assert!(!images.join("thumb_2222222222222222.png").exists());
        assert!(!images.join("3333333333333333.jpg").exists());
        assert!(!images.join("4444444444444444.jpeg").exists());
        assert!(images.join("thumb_2222222222222222.5.tmp.png").exists());
        assert!(images.join("notes.png").exists());

        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stale_requires_two_observations_and_grace_period() {
        let dir = temp_dir("stale-gate");
        let images = dir.join("images");
        std::fs::create_dir_all(&images).unwrap();
        let (_db_path, db) = temp_database(&dir);

        // External image: path missing, capture evidence present.
        let missing = dir.join("external.png");
        insert_image_item(
            &db,
            501,
            &missing.to_string_lossy(),
            "SnippingTool",
            "2026-07-28T00:00:00Z",
            "2026-07-28T00:00:00Z",
        );

        let t0 = "2026-07-28T00:00:00Z".parse::<DateTime<Utc>>().unwrap();

        // First observation: first missing, not eligible.
        let mut stats = CleanupStats::default();
        run_stale_scan(&db, t0, None, &mut stats);
        assert_eq!(stats.stale_scanned, 1);
        assert_eq!(stats.stale_first_missing, 1);
        assert_eq!(stats.stale_pending_confirmation, 1);
        assert_eq!(stats.stale_items, 0);
        assert_eq!(db.count_all_items_for_test(), 1);

        // Second observation within the grace period: pending, not eligible.
        let mut stats = CleanupStats::default();
        run_stale_scan(&db, t0 + Duration::hours(1), None, &mut stats);
        assert_eq!(stats.stale_first_missing, 0);
        assert_eq!(stats.stale_pending_confirmation, 1);
        assert_eq!(stats.stale_items, 0);

        // Third observation beyond the grace period: eligible and deleted.
        let mut stats = CleanupStats::default();
        run_stale_scan(&db, t0 + Duration::hours(25), None, &mut stats);
        assert_eq!(stats.stale_eligible, 1);
        assert_eq!(stats.stale_items, 1);
        assert_eq!(db.count_all_items_for_test(), 0);

        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn present_observation_resets_missing_history() {
        let dir = temp_dir("stale-reset");
        let (_db_path, db) = temp_database(&dir);

        // Image that exists during the first scan, then disappears.
        // (The temp path is external to the real images dir, so capture
        // evidence is required — the item carries it below.)
        let images = dir.join("images");
        std::fs::create_dir_all(&images).unwrap();
        let image = images.join("present-then-gone.png");
        std::fs::write(&image, b"png").unwrap();
        insert_image_item(
            &db,
            502,
            &image.to_string_lossy(),
            "Local App",
            "2026-07-28T00:00:00Z",
            "2026-07-28T00:00:00Z",
        );

        let t0 = "2026-07-28T00:00:00Z".parse::<DateTime<Utc>>().unwrap();

        let mut stats = CleanupStats::default();
        run_stale_scan(&db, t0, None, &mut stats);
        assert_eq!(stats.stale_present, 1);
        assert_eq!(stats.stale_items, 0);

        // File disappears → first missing observation.
        std::fs::remove_file(&image).unwrap();
        let mut stats = CleanupStats::default();
        run_stale_scan(&db, t0 + Duration::hours(25), None, &mut stats);
        assert_eq!(stats.stale_first_missing, 1);
        assert_eq!(stats.stale_items, 0);

        // File returns → observation cleared.
        std::fs::write(&image, b"png").unwrap();
        let mut stats = CleanupStats::default();
        run_stale_scan(&db, t0 + Duration::hours(26), None, &mut stats);
        assert_eq!(stats.stale_present, 1);

        // File disappears again → counting restarts from zero.
        std::fs::remove_file(&image).unwrap();
        let mut stats = CleanupStats::default();
        run_stale_scan(&db, t0 + Duration::hours(50), None, &mut stats);
        assert_eq!(stats.stale_first_missing, 1);
        assert_eq!(stats.stale_items, 0);

        let mut stats = CleanupStats::default();
        run_stale_scan(&db, t0 + Duration::hours(75), None, &mut stats);
        assert_eq!(stats.stale_eligible, 1);
        assert_eq!(stats.stale_items, 1);

        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn item_update_restarts_missing_observation() {
        let dir = temp_dir("stale-update");
        let (_db_path, db) = temp_database(&dir);

        let missing = dir.join("external.png");
        insert_image_item(
            &db,
            503,
            &missing.to_string_lossy(),
            "SnippingTool",
            "2026-07-28T00:00:00Z",
            "2026-07-28T00:00:00Z",
        );

        let t0 = "2026-07-28T00:00:00Z".parse::<DateTime<Utc>>().unwrap();

        let mut stats = CleanupStats::default();
        run_stale_scan(&db, t0, None, &mut stats);
        assert_eq!(stats.stale_first_missing, 1);

        // Re-capture (updated_at changes) → observation restarts.
        insert_image_item(
            &db,
            503,
            &missing.to_string_lossy(),
            "SnippingTool",
            "2026-07-28T00:00:00Z",
            "2026-07-28T01:00:00Z",
        );

        let mut stats = CleanupStats::default();
        run_stale_scan(&db, t0 + Duration::hours(25), None, &mut stats);
        assert_eq!(stats.stale_first_missing, 1);
        assert_eq!(stats.stale_eligible, 0);
        assert_eq!(stats.stale_items, 0);

        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stale_deletion_reclaims_cache_in_same_round() {
        let dir = temp_dir("stale-cache-round");
        let images = dir.join("images");
        std::fs::create_dir_all(&images).unwrap();
        let (_db_path, db) = temp_database(&dir);

        // External image whose thumbnail exists in the cache directory.
        let missing = dir.join("external.png");
        let hash = "a1b2c3d4e5f60718";
        std::fs::write(images.join(format!("thumb_{hash}.png")), b"thumb").unwrap();
        insert_image_item(
            &db,
            u64::from_str_radix(hash, 16).unwrap(),
            &missing.to_string_lossy(),
            "SnippingTool",
            "2026-07-28T00:00:00Z",
            "2026-07-28T00:00:00Z",
        );

        let t0 = "2026-07-28T00:00:00Z".parse::<DateTime<Utc>>().unwrap();

        // Observation phase 1: item stays, thumbnail stays (still referenced).
        let mut stats = CleanupStats::default();
        run_stale_scan(&db, t0, None, &mut stats);
        assert!(images.join(format!("thumb_{hash}.png")).exists());

        // Observation phase 2 beyond grace: item deleted in the same pass,
        // then orphan reclamation removes the now-unreferenced thumbnail.
        let mut stats = CleanupStats::default();
        run_stale_scan(&db, t0 + Duration::hours(25), None, &mut stats);
        assert_eq!(stats.stale_items, 1);
        assert_eq!(db.count_all_items_for_test(), 0);
        clean_orphan_images_in(&db, &images, &mut stats);
        assert_eq!(stats.orphan_images, 1);
        assert!(!images.join(format!("thumb_{hash}.png")).exists());

        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_target_with_unreachable_parent_is_unknown() {
        let dir = temp_dir("parent-gone");
        let parent = dir.join("sub");
        let target = parent.join("file.txt");
        std::fs::create_dir_all(&parent).unwrap();

        // Parent exists: NotFound for the target is trustworthy.
        assert_eq!(
            probe_path_status(&target.to_string_lossy()),
            PathStatus::DefinitelyMissing
        );

        // Parent disappears (e.g. cloud subtree temporarily gone): the
        // target must stay Unknown, never DefinitelyMissing.
        std::fs::remove_dir_all(&parent).unwrap();
        assert_eq!(
            probe_path_status(&target.to_string_lossy()),
            PathStatus::Unknown {
                reason: PathStatusReason::ParentUnavailable
            }
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stale_database_failures_mark_scan_incomplete() {
        let dir = temp_dir("stale-db-fail");
        let (_db_path, db) = temp_database(&dir);

        let missing = dir.join("external.png");
        insert_image_item(
            &db,
            505,
            &missing.to_string_lossy(),
            "SnippingTool",
            "2026-07-28T00:00:00Z",
            "2026-07-28T00:00:00Z",
        );
        let t0 = "2026-07-28T00:00:00Z".parse::<DateTime<Utc>>().unwrap();

        // Every observation write fails → the round must be incomplete.
        db.execute_batch_for_test(
            "CREATE TRIGGER reject_observation \
             BEFORE INSERT ON stale_item_observations \
             BEGIN SELECT RAISE(ABORT, 'reject'); END;",
        )
        .unwrap();
        let mut stats = CleanupStats {
            scan_complete: true,
            ..CleanupStats::default()
        };
        run_stale_scan(&db, t0, None, &mut stats);
        assert!(!stats.scan_complete);
        assert_eq!(stats.stale_items, 0);

        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stale_delete_failure_marks_scan_incomplete() {
        let dir = temp_dir("stale-delete-fail");
        let (_db_path, db) = temp_database(&dir);

        let missing = dir.join("external.png");
        insert_image_item(
            &db,
            506,
            &missing.to_string_lossy(),
            "SnippingTool",
            "2026-07-28T00:00:00Z",
            "2026-07-28T00:00:00Z",
        );
        let t0 = "2026-07-28T00:00:00Z".parse::<DateTime<Utc>>().unwrap();

        // First observation succeeds normally.
        let mut stats = CleanupStats {
            scan_complete: true,
            ..CleanupStats::default()
        };
        run_stale_scan(&db, t0, None, &mut stats);
        assert!(stats.scan_complete);

        // The final delete fails → the round must be incomplete.
        db.execute_batch_for_test(
            "CREATE TRIGGER reject_stale_delete \
             BEFORE DELETE ON clipboard_items \
             BEGIN SELECT RAISE(ABORT, 'reject'); END;",
        )
        .unwrap();
        let mut stats = CleanupStats {
            scan_complete: true,
            ..CleanupStats::default()
        };
        run_stale_scan(&db, t0 + Duration::hours(25), None, &mut stats);
        assert!(!stats.scan_complete);
        assert_eq!(stats.stale_eligible, 1);
        assert_eq!(stats.stale_items, 0);
        assert_eq!(db.count_all_items_for_test(), 1);

        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn retention_failure_flags_scan_incomplete() {
        let dir = temp_dir("retention-fail");
        let (_db_path, db) = temp_database(&dir);

        // A trigger that aborts every retention delete.
        db.execute_batch_for_test(
            "CREATE TRIGGER reject_retention \
             BEFORE DELETE ON clipboard_items \
             BEGIN SELECT RAISE(ABORT, 'reject'); END;",
        )
        .unwrap();
        // An item old enough to be picked up by the retention scan.
        db.upsert(&{
            let mut item =
                ClipboardItem::new_text(0, "old item", ContentType::PlainText, None, None);
            item.updated_at = "2026-01-01T00:00:00Z".parse().unwrap();
            item.created_at = item.updated_at;
            item
        })
        .unwrap();

        let mut stats = CleanupStats {
            scan_complete: true,
            ..CleanupStats::default()
        };
        clean_expired_clipboard_items(&db, 7, None, &mut stats);
        assert!(stats.retention_failed);
        assert!(stats.scan_complete); // retention failure is tracked separately

        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn temp_database(dir: &std::path::Path) -> (std::path::PathBuf, Database) {
        let db_path = dir.join("clippi-test.db");
        let db = Database::open(&db_path.to_string_lossy()).unwrap();
        (db_path, db)
    }
}
