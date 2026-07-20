//! --- Cloud sync data types and merge logic. ---
//!
//! The sync format is a single JSON file (`clippi_sync.json`) placed in a
//! cloud-synced folder (OneDrive, iCloud, Dropbox, etc.). The same format
//! can later be used with a WebDAV backend by swapping the transport layer.

use crate::core::db::Database;
use crate::core::types::TagInfo;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Connection / health status of a backend.
#[derive(Debug, Clone)]
pub enum BackendStatus {
    Online,
    Offline,
    Error(String),
}

/// Transport-agnostic backend trait.
///
/// Implementors handle reading/writing `SyncPayload` to a specific medium
/// (local folder, WebDAV, etc.). The merge logic lives outside the trait
/// and is shared across all backends.
pub trait SyncBackend: Send + Sync {
    fn check_status(&self) -> BackendStatus;
    /// Pull the remote payload. When `bypass_cache` is true, the backend
    /// should skip any mtime/etag optimization and always read the file.
    fn pull(&self, bypass_cache: bool) -> Result<SyncPayload, String>;
    fn push(&self, payload: &SyncPayload) -> Result<(), String>;

    /// Per-backend sync interval in seconds. Returns 0 to use the global default.
    fn sync_interval(&self) -> u64;

    /// Called after a successful push. Backends can override to clean up
    /// temporary or conflict files (e.g., clippi_sync-*.json).
    fn post_push_cleanup(&self) -> Result<(), String> {
        Ok(())
    }

    /// Upload a binary blob to {remote}/images/{hash_hex}.{ext}
    fn upload_blob(&self, _hash_hex: &str, _ext: &str, _data: &[u8]) -> Result<(), String> {
        Err("not supported".into())
    }

    /// Download a binary blob from {remote}/images/{hash_hex}.{ext}
    fn download_blob(&self, _hash_hex: &str, _ext: &str) -> Result<Vec<u8>, String> {
        Err("not supported".into())
    }

    /// List remote blob filenames (e.g. ["a1b2c3.png", "d4e5f6.jpg"])
    fn list_remote_blobs(&self) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
}

/// Top-level sync payload stored as JSON on the cloud folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPayload {
    pub version: u32,
    pub device_name: String,
    pub synced_at: String, // RFC3339
    pub items: Vec<SyncItem>,
    pub tags: Vec<SyncTag>,
    /// Deleted item tombstones.
    #[serde(default)]
    pub deleted_items: Vec<SyncDeletedItem>,
    /// Deleted tag tombstones.
    #[serde(default)]
    pub deleted_tags: Vec<SyncDeletedTag>,
    /// Unfavorite markers.
    #[serde(default)]
    pub unfavorited_items: Vec<SyncUnfavoritedItem>,
}

/// A single clipboard item in the sync payload.
/// Uses `content_hash` as the cross-device merge key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncItem {
    pub content_type: String,
    pub full_text: String,
    pub content_hash: u64,
    pub created_at: String, // RFC3339
    pub updated_at: String, // RFC3339
    pub rich_data: String,
    pub is_favorite: bool,
    pub note: String,
    /// Character count for text types; 0 for other types.
    #[serde(default)]
    pub size: i64,
    /// Tag associations carried on the item.
    #[serde(default)]
    pub tags: Vec<SyncTagRef>,
    /// Plain-text subtype: "" | "email" | "phone" | "link" | "path" | "color".
    #[serde(default)]
    pub meta_type: String,
    /// Image width (only meaningful for image type items).
    #[serde(default)]
    pub image_width: u32,
    /// Image height.
    #[serde(default)]
    pub image_height: u32,
    /// Remote image blob filename (e.g. "a1b2c3d4e5f60789.png" or ".jpg").
    #[serde(default)]
    pub image_blob: String,
}

/// Tag reference embedded in a SyncItem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTagRef {
    #[serde(default)]
    pub uid: String,
    pub name: String,
    pub color: String,
}

/// Global tag definition in the top-level tags array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTag {
    #[serde(default)]
    pub uid: String,
    pub name: String,
    pub color: String,
    /// Last-modified timestamp for tag color conflict resolution.
    #[serde(default)]
    pub updated_at: String,
}

/// A deleted item tombstone — notifies other devices to delete this item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncDeletedItem {
    pub content_hash: u64,
    pub deleted_at: String, // RFC3339
    pub device_name: String,
}

/// A deleted tag tombstone — notifies other devices to delete this tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncDeletedTag {
    #[serde(default)]
    pub uid: String,
    pub name: String,
    pub deleted_at: String, // RFC3339
    pub device_name: String,
}

/// An unfavorite marker — notifies other devices that an item was unfavorited.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncUnfavoritedItem {
    pub content_hash: u64,
    pub unfavorited_at: String, // RFC3339
    pub device_name: String,
}

/// Result of a merge operation reported back to UI.
#[derive(Debug, Clone, Default)]
pub struct MergeStats {
    pub items_added: u32,
    pub items_updated: u32,
    pub items_deleted: u32,
    pub tags_added: u32,
    pub tags_deleted: u32,
}

impl MergeStats {
    pub fn is_empty(&self) -> bool {
        self.items_added == 0
            && self.items_updated == 0
            && self.items_deleted == 0
            && self.tags_added == 0
            && self.tags_deleted == 0
    }
}

// --- ── Snapshot building ── ---

/// Build a full `SyncPayload` from the local database.
/// Excludes file type items. Image items are included when `include_images` is true.
/// When `favorites_only` is true, only favorited items are included.
/// Only tags referenced by the synced items are included.
pub fn build_snapshot(
    db: &Mutex<Database>,
    device_name: &str,
    favorites_only: bool,
    _include_images: bool,
) -> Result<SyncPayload, String> {
    let db = db.lock().map_err(|e| format!("db lock: {e}"))?;
    let _ = db.cleanup_sync_residue().inspect_err(|e| {
        log::warn!("sync: cleanup_sync_residue failed before snapshot: {e}");
    });

    // --- Collect all live synced items ---
    let items = db
        .get_all_sync_items_with_tags(_include_images)
        .map_err(|e| format!("query items: {e}"))?;

    // --- Collect unfavorited hashes up front. Items that are unfavorited ---
    // (is_favorite=false) AND have a tombstone should be excluded from sync_items
    // --- — the tombstone in unfavorited_items is the authoritative signal. ---
    let unfav_hashes: std::collections::HashSet<u64> = db
        .get_unfavorited_recent(30)
        .map_err(|e| format!("query unfavorite markers: {e}"))?
        .into_iter()
        .map(|(hash, _, _)| hash)
        .collect();

    let mut sync_items: Vec<SyncItem> = Vec::with_capacity(items.len());
    let mut used_tag_uids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut used_tag_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    for item in items {
        if favorites_only && !item.is_favorite {
            continue;
        }
        // --- If this item is unfavorited and has a tombstone, the tombstone ---
        // --- communicates the unfavorite — exclude from items[] to avoid ---
        // --- the confusing "both lists" cloud state. ---
        if !item.is_favorite && unfav_hashes.contains(&item.content_hash) {
            continue;
        }

        let tags: Vec<SyncTagRef> = item
            .tags
            .iter()
            .map(|t| {
                used_tag_names.insert(t.name.clone());
                if !t.uid.is_empty() {
                    used_tag_uids.insert(t.uid.clone());
                }
                SyncTagRef {
                    uid: t.uid.clone(),
                    name: t.name.clone(),
                    color: t.color.clone(),
                }
            })
            .collect();

        let full_text = if item.content_type == crate::core::types::ContentType::Image {
            portable_image_text(&item.full_text, &item.image_path)
        } else {
            item.full_text.clone()
        };

        sync_items.push(SyncItem {
            content_type: item.content_type.as_str().to_string(),
            full_text,
            content_hash: item.content_hash,
            created_at: item.created_at.to_rfc3339(),
            updated_at: item.updated_at.to_rfc3339(),
            rich_data: item.rich_data,
            is_favorite: item.is_favorite,
            note: item.note,
            size: item.size,
            tags,
            meta_type: item.meta_type.clone(),
            image_width: item.image_width,
            image_height: item.image_height,
            image_blob: String::new(),
        });
    }

    // --- Only include tags that are referenced by the synced items ---
    let all_tags: Vec<SyncTag> = if used_tag_names.is_empty() {
        Vec::new()
    } else {
        db.get_all_tags()
            .map_err(|e| format!("query all tags: {e}"))?
            .into_iter()
            .filter(|t| used_tag_uids.contains(&t.uid) || used_tag_names.contains(&t.name))
            .map(|t| SyncTag {
                uid: t.uid,
                name: t.name,
                color: t.color,
                updated_at: t.updated_at,
            })
            .collect()
    };

    // --- Collect recent tombstones (30-day window) ---
    let deleted_items: Vec<SyncDeletedItem> = db
        .get_deleted_items_recent(30)
        .map_err(|e| format!("query item tombstones: {e}"))?
        .into_iter()
        .map(|(hash, at, dev)| SyncDeletedItem {
            content_hash: hash,
            deleted_at: at,
            device_name: dev,
        })
        .collect();

    let deleted_tags: Vec<SyncDeletedTag> = db
        .get_deleted_tags_recent(30)
        .map_err(|e| format!("query tag tombstones: {e}"))?
        .into_iter()
        .map(|(uid, name, at, dev)| SyncDeletedTag {
            uid,
            name,
            deleted_at: at,
            device_name: dev,
        })
        .collect();

    let unfavorited_items: Vec<SyncUnfavoritedItem> = db
        .get_unfavorited_recent(30)
        .map_err(|e| format!("query unfavorite markers: {e}"))?
        .into_iter()
        .map(|(hash, at, dev)| SyncUnfavoritedItem {
            content_hash: hash,
            unfavorited_at: at,
            device_name: dev,
        })
        .collect();

    // Sort all arrays deterministically so semantic hashes match across devices.
    // --- SQL queries return rows in undefined order, and HashMap iteration is ---
    // non-deterministic — without sorting, two devices with identical logical
    // --- state produce different hashes and endlessly overwrite each other. ---
    let mut payload = SyncPayload {
        version: crate::core::migration::SYNC_VERSION,
        device_name: device_name.to_string(),
        synced_at: chrono::Utc::now().to_rfc3339(),
        items: sync_items,
        tags: all_tags,
        deleted_items,
        deleted_tags,
        unfavorited_items,
    };
    sanitize_payload(&mut payload);
    Ok(payload)
}

fn portable_image_text(full_text: &str, image_path: &str) -> String {
    portable_path_filename(full_text)
        .or_else(|| portable_path_filename(image_path))
        .unwrap_or_else(|| "Image".to_string())
}

fn portable_path_filename(path: &str) -> Option<String> {
    let trimmed = path.trim().trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return None;
    }
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn tag_key(uid: &str, name: &str) -> String {
    if uid.is_empty() {
        format!("name:{name}")
    } else {
        format!("uid:{uid}")
    }
}

fn resolve_tag_for_ref(db: &Database, tag_ref: &SyncTagRef) -> rusqlite::Result<Option<TagInfo>> {
    if !tag_ref.uid.is_empty() {
        if let Some(tag) = db.get_tag_by_uid(&tag_ref.uid)? {
            return Ok(Some(tag));
        }
    }
    db.get_tag_by_name(&tag_ref.name)
}

fn resolve_tag_for_remote_tag(
    db: &Database,
    remote_tag: &SyncTag,
) -> rusqlite::Result<Option<TagInfo>> {
    if !remote_tag.uid.is_empty() {
        if let Some(tag) = db.get_tag_by_uid(&remote_tag.uid)? {
            return Ok(Some(tag));
        }
    }
    db.get_tag_by_name(&remote_tag.name)
}

fn resolve_tag_for_tombstone(
    db: &Database,
    tombstone: &SyncDeletedTag,
) -> rusqlite::Result<Option<TagInfo>> {
    if tombstone.uid.is_empty() {
        db.get_tag_by_name(&tombstone.name)
    } else {
        db.get_tag_by_uid(&tombstone.uid)
    }
}

fn tag_has_more_stable_refs(a: &SyncItem, b: &SyncItem) -> bool {
    let a_count = a.tags.iter().filter(|tag| !tag.uid.is_empty()).count();
    let b_count = b.tags.iter().filter(|tag| !tag.uid.is_empty()).count();
    a_count > b_count
}

fn sync_tag_preferred(candidate: &SyncTag, existing: &SyncTag) -> bool {
    rfc3339_newer(&candidate.updated_at, &existing.updated_at)
        || (candidate.updated_at == existing.updated_at
            && existing.uid.is_empty()
            && !candidate.uid.is_empty())
}

fn deleted_tag_preferred(candidate: &SyncDeletedTag, existing: &SyncDeletedTag) -> bool {
    rfc3339_newer(&candidate.deleted_at, &existing.deleted_at)
        || (candidate.deleted_at == existing.deleted_at
            && existing.uid.is_empty()
            && !candidate.uid.is_empty())
}

// --- ── Merge logic ── ---

/// Merge remote sync payload into the local database (v1, 4-phase).
///
/// Phases (in order):
/// 0. Clean expired local tombstones
/// 1. Process remote item tombstones — delete local items if tombstone is newer
/// 2. Process remote tag tombstones — delete local tags if tombstone is newer
/// 3. Merge tags — create/update with last-writer-wins color resolution
/// 4. Merge items — create/update with last-writer-wins, skip tombstoned
pub fn merge_remote_into_local(
    db: &Mutex<Database>,
    remote: &mut SyncPayload,
    local_device_name: &str,
) -> Result<MergeStats, String> {
    crate::core::migration::migrate_sync_payload(remote);
    sanitize_payload(remote);
    let mut db = db.lock().map_err(|e| format!("db lock: {e}"))?;
    let mut stats = MergeStats::default();

    // --- Phase 0: Clean expired local tombstones ---
    let _ = db
        .cleanup_old_tombstones(30)
        .inspect_err(|e| log::warn!("sync: cleanup_old_tombstones failed: {e}"));

    // --- Phase 1: Process remote item tombstones ---
    for tombstone in &remote.deleted_items {
        if tombstone.device_name == local_device_name {
            continue; // own deletion, already handled
        }
        if db
            .is_item_tombstoned(tombstone.content_hash)
            .unwrap_or(false)
        {
            // --- Already tombstoned — but the remote tombstone may have a newer ---
            // deleted_at. Replace the local tombstone if remote is newer so
            // --- stale timestamps don't propagate to other devices. ---
            if let Ok(Some(local_at)) = db.get_item_tombstone_deleted_at(tombstone.content_hash) {
                if rfc3339_newer(&tombstone.deleted_at, &local_at) {
                    if let Err(e) = db.remove_item_tombstone(tombstone.content_hash) {
                        log::warn!("sync: remove_item_tombstone (update) failed: {e}");
                    }
                    let _ = db
                        .record_item_deletion(
                            tombstone.content_hash,
                            &tombstone.deleted_at,
                            &tombstone.device_name,
                        )
                        .inspect_err(|e| {
                            log::warn!("sync: record_item_deletion (update) failed: {e}")
                        });
                }
            }
            continue;
        }
        // --- Check local item age before recording the tombstone. ---
        // --- If the local item is newer, the user recreated it after deletion — ---
        // --- skip the tombstone and keep the item. ---
        if let Ok(Some(local_item)) = db.get_by_hash(tombstone.content_hash) {
            let remote_ts = parse_rfc3339(&tombstone.deleted_at);
            if remote_ts.is_some_and(|r| r > local_item.updated_at) {
                // Tombstone is newer — delete and record for propagation.
                if db
                    .delete_item_by_hash(tombstone.content_hash)
                    .unwrap_or(false)
                {
                    stats.items_deleted += 1;
                }
                let _ = db
                    .record_item_deletion(
                        tombstone.content_hash,
                        &tombstone.deleted_at,
                        &tombstone.device_name,
                    )
                    .inspect_err(|e| log::warn!("sync: record_item_deletion (newer) failed: {e}"));
            }
            // else: local item is newer, do nothing (don't record tombstone) ---
        } else {
            // No local item — record tombstone for propagation to other devices.
            let _ = db
                .record_item_deletion(
                    tombstone.content_hash,
                    &tombstone.deleted_at,
                    &tombstone.device_name,
                )
                .inspect_err(|e| log::warn!("sync: record_item_deletion (no local) failed: {e}"));
        }
    }

    // --- Phase 2.5: Process remote unfavorite markers ---
    for uf in &remote.unfavorited_items {
        if uf.device_name == local_device_name {
            continue; // own unfavorite, already handled
        }
        if db.is_item_unfavorited(uf.content_hash).unwrap_or(false) {
            // Already marked — replace if remote marker is newer.
            if let Ok(Some(local_at)) = db.get_unfavorite_deleted_at(uf.content_hash) {
                if rfc3339_newer(&uf.unfavorited_at, &local_at) {
                    if let Err(e) = db.remove_unfavorite(uf.content_hash) {
                        log::warn!("sync: remove_unfavorite (update) failed: {e}");
                    }
                    let _ = db
                        .record_unfavorite(uf.content_hash, &uf.unfavorited_at, &uf.device_name)
                        .inspect_err(|e| {
                            log::warn!("sync: record_unfavorite (update) failed: {e}")
                        });
                }
            }
            continue;
        }
        // --- Check local item age before recording the marker. ---
        // --- If the local item is newer, the user re-favorited it — skip. ---
        if let Ok(Some(local_item)) = db.get_by_hash(uf.content_hash) {
            if local_item.is_favorite {
                let remote_ts = parse_rfc3339(&uf.unfavorited_at);
                if remote_ts.is_some_and(|r| r > local_item.updated_at) {
                    if let Err(e) = db.set_favorite(local_item.id, false) {
                        log::error!("sync: set_favorite failed: {e}");
                    }
                    stats.items_updated += 1;
                    let _ = db
                        .record_unfavorite(uf.content_hash, &uf.unfavorited_at, &uf.device_name)
                        .inspect_err(|e| log::warn!("sync: record_unfavorite (fav) failed: {e}"));
                }
                // else: item was re-favorited after unfavorite, skip
            }
            // else: already not favorited, nothing to do
        } else {
            // No local item — record marker for propagation.
            let _ = db
                .record_unfavorite(uf.content_hash, &uf.unfavorited_at, &uf.device_name)
                .inspect_err(|e| log::warn!("sync: record_unfavorite (no local) failed: {e}"));
        }
    }

    // If a tombstone and a tag share the same stable identity, the remote
    // device recreated the tag and the live tag should take precedence.
    let remote_tag_keys: std::collections::HashSet<String> = remote
        .tags
        .iter()
        .map(|t| tag_key(&t.uid, &t.name))
        .collect();

    // --- Phase 2: Process remote tag tombstones ---
    for tombstone in &remote.deleted_tags {
        if tombstone.device_name == local_device_name {
            continue; // own deletion
        }
        if remote_tag_keys.contains(&tag_key(&tombstone.uid, &tombstone.name)) {
            continue;
        }
        if db
            .is_tag_tombstoned(&tombstone.uid, &tombstone.name)
            .unwrap_or(false)
        {
            // Already tombstoned — replace if remote tombstone is newer.
            if let Ok(Some(local_at)) =
                db.get_tag_tombstone_deleted_at(&tombstone.uid, &tombstone.name)
            {
                if rfc3339_newer(&tombstone.deleted_at, &local_at) {
                    if let Err(e) = db.remove_tag_tombstone(&tombstone.uid, &tombstone.name) {
                        log::warn!("sync: remove_tag_tombstone (update) failed: {e}");
                    }
                    let _ = db
                        .record_tag_deletion(
                            &tombstone.uid,
                            &tombstone.name,
                            &tombstone.deleted_at,
                            &tombstone.device_name,
                        )
                        .inspect_err(|e| {
                            log::warn!("sync: record_tag_deletion (update) failed: {e}")
                        });
                }
            }
            continue;
        }
        // --- Check local tag age before recording the tombstone. ---
        // --- If the local tag is newer, the user recreated it — skip. ---
        if let Ok(Some(local_tag)) = resolve_tag_for_tombstone(&db, tombstone) {
            let remote_ts = parse_rfc3339(&tombstone.deleted_at);
            let local_ts = parse_rfc3339(&local_tag.updated_at);
            if remote_ts.is_some_and(|r| local_ts.is_none_or(|l| r > l)) {
                // Tombstone is newer — delete and record for propagation.
                let deleted = if tombstone.uid.is_empty() {
                    db.delete_tag_by_name(&tombstone.name)
                } else {
                    db.delete_tag_by_uid(&tombstone.uid)
                }
                .unwrap_or(false);
                if deleted {
                    stats.tags_deleted += 1;
                }
                let _ = db
                    .record_tag_deletion(
                        &tombstone.uid,
                        &tombstone.name,
                        &tombstone.deleted_at,
                        &tombstone.device_name,
                    )
                    .inspect_err(|e| log::warn!("sync: record_tag_deletion (newer) failed: {e}"));
            }
            // else: local tag is newer, do nothing (don't record tombstone) ---
        } else {
            // No local tag — record tombstone for propagation.
            let _ = db
                .record_tag_deletion(
                    &tombstone.uid,
                    &tombstone.name,
                    &tombstone.deleted_at,
                    &tombstone.device_name,
                )
                .inspect_err(|e| log::warn!("sync: record_tag_deletion (no local) failed: {e}"));
        }
    }

    // --- Phase 3: Merge tags — create or update with color conflict resolution ---
    for remote_tag in &remote.tags {
        // --- If this tag is locally tombstoned, compare timestamps. The remote ---
        // --- tag may be newer (recreated after deletion) — in that case accept it. ---
        if db
            .is_tag_tombstoned(&remote_tag.uid, &remote_tag.name)
            .unwrap_or(false)
        {
            if remote_tag.updated_at.is_empty() {
                // --- v1 tag without timestamp — fall back to device-based check. ---
                if db
                    .is_tag_tombstoned_by_other_device(
                        &remote_tag.uid,
                        &remote_tag.name,
                        &remote.device_name,
                    )
                    .unwrap_or(false)
                {
                    continue;
                }
            } else if let Ok(Some(deleted_at)) =
                db.get_tag_tombstone_deleted_at(&remote_tag.uid, &remote_tag.name)
            {
                let remote_ts = parse_rfc3339(&remote_tag.updated_at);
                let del_ts = parse_rfc3339(&deleted_at);
                if remote_ts.is_some_and(|r| del_ts.is_some_and(|d| d >= r)) {
                    continue; // tombstone is newer or same age, skip
                }
                // --- Remote tag is newer — fall through to clear tombstone and import. ---
            }
        }
        // --- Clear any tombstone so the tag can be recreated. ---
        if let Err(e) = db.remove_tag_tombstone(&remote_tag.uid, &remote_tag.name) {
            log::warn!("sync: remove_tag_tombstone (merge) failed: {e}");
        }
        match resolve_tag_for_remote_tag(&db, remote_tag).map_err(|e| format!("tag lookup: {e}"))? {
            None => {
                // --- New tag from remote ---
                if remote_tag.updated_at.is_empty() && remote_tag.uid.is_empty() {
                    db.create_tag(&remote_tag.name, &remote_tag.color)
                } else if remote_tag.updated_at.is_empty() {
                    db.create_tag_with_uid_and_timestamp(
                        &remote_tag.uid,
                        &remote_tag.name,
                        &remote_tag.color,
                        &chrono::Utc::now().to_rfc3339(),
                    )
                } else if remote_tag.uid.is_empty() {
                    db.create_tag_with_timestamp(
                        &remote_tag.name,
                        &remote_tag.color,
                        &remote_tag.updated_at,
                    )
                } else {
                    db.create_tag_with_uid_and_timestamp(
                        &remote_tag.uid,
                        &remote_tag.name,
                        &remote_tag.color,
                        &remote_tag.updated_at,
                    )
                }
                .map_err(|e| format!("create tag: {e}"))?;
                stats.tags_added += 1;
            }
            Some(local_tag) => {
                // Tag exists — update color if remote is newer
                if !remote_tag.updated_at.is_empty() {
                    let remote_ts = parse_rfc3339(&remote_tag.updated_at);
                    let local_ts = parse_rfc3339(&local_tag.updated_at);
                    if remote_ts.is_some_and(|r| local_ts.is_none_or(|l| r > l)) {
                        if !remote_tag.uid.is_empty()
                            && local_tag.uid != remote_tag.uid
                            && local_tag.uid == crate::core::db::legacy_tag_uid(&local_tag.name)
                        {
                            db.update_tag_uid_with_timestamp(
                                local_tag.id,
                                &remote_tag.uid,
                                &remote_tag.name,
                                &remote_tag.color,
                                &remote_tag.updated_at,
                            )
                        } else {
                            db.update_tag_with_timestamp(
                                local_tag.id,
                                &remote_tag.name,
                                &remote_tag.color,
                                &remote_tag.updated_at,
                            )
                        }
                        .map_err(|e| format!("update tag: {e}"))?;
                    }
                }
            }
        }
    }

    // --- Phase 4: Merge items by content_hash (last-writer-wins) ---
    for remote_item in &remote.items {
        if db
            .is_item_tombstoned(remote_item.content_hash)
            .unwrap_or(false)
        {
            // --- Locally tombstoned, but the remote version may be newer — ---
            // --- the item could have been recreated after deletion. ---
            let should_import = if let Ok(Some(deleted_at)) =
                db.get_item_tombstone_deleted_at(remote_item.content_hash)
            {
                let remote_ts = parse_rfc3339(&remote_item.updated_at);
                let del_ts = parse_rfc3339(&deleted_at);
                remote_ts.is_some_and(|r| del_ts.is_some_and(|d| r > d))
            } else {
                false
            };
            if should_import {
                if let Err(e) = db.remove_item_tombstone(remote_item.content_hash) {
                    log::warn!("sync: remove_item_tombstone (import) failed: {e}");
                }
                if let Err(e) = db.remove_unfavorite(remote_item.content_hash) {
                    log::warn!("sync: remove_unfavorite (import) failed: {e}");
                }
                // --- fall through to import below ---
            } else {
                continue; // tombstone is newer or same age, skip
            }
        }
        let local = db
            .get_by_hash(remote_item.content_hash)
            .map_err(|e| format!("hash lookup: {e}"))?;

        match local {
            None => {
                // --- New item from remote — insert ---
                let item_id = db
                    .insert_sync_item_raw(remote_item)
                    .map_err(|e| format!("insert item: {e}"))?;
                stats.items_added += 1;

                for tag_ref in &remote_item.tags {
                    if let Ok(Some(tag)) = resolve_tag_for_ref(&db, tag_ref) {
                        if let Err(e) = db.add_item_tag(item_id, tag.id) {
                            log::warn!("sync: add_item_tag (new) failed: {e}");
                        }
                    }
                }

                // --- Restore remote timestamp: add_item_tag may have bumped it. ---
                if let Err(e) = db.set_item_updated_at(item_id, &remote_item.updated_at) {
                    log::error!("sync: set_item_updated_at (new) failed: {e}");
                }
            }
            Some(local_item) => {
                let remote_ts = parse_rfc3339(&remote_item.updated_at);
                let local_ts = local_item.updated_at;

                let should_promote_remote_favorite = remote_item.is_favorite
                    && !local_item.is_favorite
                    && !local_unfavorite_is_at_least_as_new(
                        &db,
                        remote_item.content_hash,
                        &remote_item.updated_at,
                    );

                if should_promote_remote_favorite
                    && remote_ts.is_none_or(|remote| remote <= local_ts)
                {
                    let mut promoted = remote_item.clone();
                    promoted.updated_at = local_ts.to_rfc3339();
                    db.update_sync_item(local_item.id, &promoted)
                        .map_err(|e| format!("promote favorite item: {e}"))?;
                    replace_item_tags(&db, local_item.id, &promoted)?;
                    if let Err(e) = db.set_item_updated_at(local_item.id, &promoted.updated_at) {
                        log::error!("sync: set_item_updated_at (promote favorite) failed: {e}");
                    }
                    if let Err(e) = db.remove_unfavorite(remote_item.content_hash) {
                        log::warn!("sync: remove_unfavorite (promote favorite) failed: {e}");
                    }
                    stats.items_updated += 1;
                } else if remote_ts.is_some_and(|remote| remote > local_ts) {
                    let mut incoming = remote_item.clone();
                    if local_item.is_favorite && !remote_item.is_favorite {
                        incoming.is_favorite = true;
                    }

                    db.update_sync_item(local_item.id, &incoming)
                        .map_err(|e| format!("update item: {e}"))?;
                    replace_item_tags(&db, local_item.id, &incoming)?;

                    // --- Restore remote timestamp: tag operations above may have ---
                    // --- bumped updated_at via touch_item, but the item data is ---
                    // --- semantically identical to what we just pulled from remote. ---
                    if let Err(e) = db.set_item_updated_at(local_item.id, &incoming.updated_at) {
                        log::error!("sync: set_item_updated_at (update) failed: {e}");
                    }
                    if incoming.is_favorite {
                        if let Err(e) = db.remove_unfavorite(remote_item.content_hash) {
                            log::warn!("sync: remove_unfavorite (update favorite) failed: {e}");
                        }
                    }

                    stats.items_updated += 1;
                }
            }
        }
    }

    Ok(stats)
}

/// Compute a semantic content hash of the sync payload.
///
/// Hashes only the data fields (items, tags, tombstones) and ignores
/// metadata that changes on every push (device_name, synced_at).
/// Used to detect whether a local snapshot is semantically identical
/// to a remote payload — if the hashes match, we can skip the push.
pub fn payload_semantic_hash(payload: &SyncPayload) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for item in &payload.items {
        item.content_type.hash(&mut h);
        item.full_text.hash(&mut h);
        item.content_hash.hash(&mut h);
        item.updated_at.hash(&mut h);
        item.rich_data.hash(&mut h);
        item.is_favorite.hash(&mut h);
        item.note.hash(&mut h);
        item.size.hash(&mut h);
        item.meta_type.hash(&mut h);
        item.image_width.hash(&mut h);
        item.image_height.hash(&mut h);
        item.image_blob.hash(&mut h);
        item.tags.len().hash(&mut h);
        for tag in &item.tags {
            tag.uid.hash(&mut h);
            tag.name.hash(&mut h);
            tag.color.hash(&mut h);
        }
    }
    payload.items.len().hash(&mut h);
    for tag in &payload.tags {
        tag.uid.hash(&mut h);
        tag.name.hash(&mut h);
        tag.color.hash(&mut h);
        tag.updated_at.hash(&mut h);
    }
    payload.tags.len().hash(&mut h);
    for d in &payload.deleted_items {
        d.content_hash.hash(&mut h);
        d.deleted_at.hash(&mut h);
    }
    payload.deleted_items.len().hash(&mut h);
    for d in &payload.deleted_tags {
        d.uid.hash(&mut h);
        d.name.hash(&mut h);
        d.deleted_at.hash(&mut h);
    }
    payload.deleted_tags.len().hash(&mut h);
    for u in &payload.unfavorited_items {
        u.content_hash.hash(&mut h);
        u.unfavorited_at.hash(&mut h);
    }
    payload.unfavorited_items.len().hash(&mut h);
    h.finish()
}

/// Normalize legacy or conflicted cloud payloads before merge/hash decisions.
pub fn sanitize_payload(payload: &mut SyncPayload) {
    let mut item_map: std::collections::HashMap<u64, SyncItem> =
        std::collections::HashMap::with_capacity(payload.items.len());
    for mut item in std::mem::take(&mut payload.items) {
        sanitize_sync_item_image_fields(&mut item);
        match item_map.get(&item.content_hash) {
            Some(existing)
                if !(rfc3339_newer(&item.updated_at, &existing.updated_at)
                    || item.updated_at == existing.updated_at
                        && tag_has_more_stable_refs(&item, existing)) => {}
            _ => {
                item_map.insert(item.content_hash, item);
            }
        }
    }

    let mut tag_map: std::collections::HashMap<String, SyncTag> =
        std::collections::HashMap::with_capacity(payload.tags.len());
    for tag in std::mem::take(&mut payload.tags) {
        let key = tag_key(&tag.uid, &tag.name);
        match tag_map.get(&key) {
            Some(existing) if !sync_tag_preferred(&tag, existing) => {}
            _ => {
                tag_map.insert(key, tag);
            }
        }
    }
    let mut tag_name_map: std::collections::HashMap<String, SyncTag> =
        std::collections::HashMap::with_capacity(tag_map.len());
    for tag in tag_map.into_values() {
        match tag_name_map.get(&tag.name) {
            Some(existing) if !sync_tag_preferred(&tag, existing) => {}
            _ => {
                tag_name_map.insert(tag.name.clone(), tag);
            }
        }
    }
    let mut tag_map: std::collections::HashMap<String, SyncTag> = tag_name_map
        .into_values()
        .map(|tag| (tag_key(&tag.uid, &tag.name), tag))
        .collect();

    let mut deleted_item_map: std::collections::HashMap<u64, SyncDeletedItem> =
        std::collections::HashMap::with_capacity(payload.deleted_items.len());
    for tombstone in std::mem::take(&mut payload.deleted_items) {
        match deleted_item_map.get(&tombstone.content_hash) {
            Some(existing) if !rfc3339_newer(&tombstone.deleted_at, &existing.deleted_at) => {}
            _ => {
                deleted_item_map.insert(tombstone.content_hash, tombstone);
            }
        }
    }

    let mut deleted_tag_map: std::collections::HashMap<String, SyncDeletedTag> =
        std::collections::HashMap::with_capacity(payload.deleted_tags.len());
    for tombstone in std::mem::take(&mut payload.deleted_tags) {
        let key = tag_key(&tombstone.uid, &tombstone.name);
        match deleted_tag_map.get(&key) {
            Some(existing) if !deleted_tag_preferred(&tombstone, existing) => {}
            _ => {
                deleted_tag_map.insert(key, tombstone);
            }
        }
    }
    let mut deleted_tag_name_map: std::collections::HashMap<String, SyncDeletedTag> =
        std::collections::HashMap::with_capacity(deleted_tag_map.len());
    for tombstone in deleted_tag_map.into_values() {
        match deleted_tag_name_map.get(&tombstone.name) {
            Some(existing) if !deleted_tag_preferred(&tombstone, existing) => {}
            _ => {
                deleted_tag_name_map.insert(tombstone.name.clone(), tombstone);
            }
        }
    }
    let mut deleted_tag_map: std::collections::HashMap<String, SyncDeletedTag> =
        deleted_tag_name_map
            .into_values()
            .map(|tombstone| (tag_key(&tombstone.uid, &tombstone.name), tombstone))
            .collect();

    let mut unfavorite_map: std::collections::HashMap<u64, SyncUnfavoritedItem> =
        std::collections::HashMap::with_capacity(payload.unfavorited_items.len());
    for marker in std::mem::take(&mut payload.unfavorited_items) {
        match unfavorite_map.get(&marker.content_hash) {
            Some(existing) if !rfc3339_newer(&marker.unfavorited_at, &existing.unfavorited_at) => {}
            _ => {
                unfavorite_map.insert(marker.content_hash, marker);
            }
        }
    }

    item_map.retain(|hash, item| {
        if let Some(tombstone) = deleted_item_map.get(hash) {
            if rfc3339_newer(&item.updated_at, &tombstone.deleted_at) {
                deleted_item_map.remove(hash);
            } else {
                unfavorite_map.remove(hash);
                return false;
            }
        }

        if let Some(marker) = unfavorite_map.get(hash) {
            if rfc3339_newer(&item.updated_at, &marker.unfavorited_at) {
                unfavorite_map.remove(hash);
            } else {
                item.is_favorite = false;
                return false;
            }
        }

        true
    });

    tag_map.retain(|key, tag| {
        if let Some(tombstone) = deleted_tag_map.get(key) {
            if rfc3339_newer(&tag.updated_at, &tombstone.deleted_at) {
                deleted_tag_map.remove(key);
                true
            } else {
                false
            }
        } else {
            true
        }
    });

    for (hash, marker) in unfavorite_map.clone() {
        if deleted_item_map.get(&hash).is_some_and(|deleted| {
            rfc3339_newer_or_equal(&deleted.deleted_at, &marker.unfavorited_at)
        }) {
            unfavorite_map.remove(&hash);
        }
    }

    payload.items = item_map.into_values().collect();
    payload.tags = tag_map.into_values().collect();
    payload.deleted_items = deleted_item_map.into_values().collect();
    payload.deleted_tags = deleted_tag_map.into_values().collect();
    payload.unfavorited_items = unfavorite_map.into_values().collect();

    canonicalize_item_tag_refs(payload);
    sort_payload(payload);
}

fn sanitize_sync_item_image_fields(item: &mut SyncItem) {
    if item.content_type != "image" {
        item.image_blob.clear();
        return;
    }

    if item.image_blob.is_empty() {
        return;
    }

    let filename = item
        .image_blob
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    let ext = filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    let valid_ext = matches!(ext.as_str(), "png" | "jpg" | "jpeg");
    let valid_name = !filename.is_empty()
        && filename == item.image_blob
        && !filename.starts_with('.')
        && filename
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.');

    if valid_name && valid_ext {
        item.image_blob = filename.to_string();
    } else {
        item.image_blob = format!("{:016x}.png", item.content_hash);
    }
}

fn canonicalize_item_tag_refs(payload: &mut SyncPayload) {
    let mut by_uid: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    let mut by_name: std::collections::HashMap<String, (String, String, String)> =
        std::collections::HashMap::new();
    for tag in &payload.tags {
        if !tag.uid.is_empty() {
            by_uid.insert(tag.uid.clone(), (tag.name.clone(), tag.color.clone()));
        }
        by_name.insert(
            tag.name.clone(),
            (tag.uid.clone(), tag.name.clone(), tag.color.clone()),
        );
    }

    for item in &mut payload.items {
        for tag_ref in &mut item.tags {
            if !tag_ref.uid.is_empty() {
                if let Some((name, color)) = by_uid.get(&tag_ref.uid) {
                    tag_ref.name = name.clone();
                    tag_ref.color = color.clone();
                    continue;
                }
            }
            if let Some((uid, name, color)) = by_name.get(&tag_ref.name) {
                tag_ref.uid = uid.clone();
                tag_ref.name = name.clone();
                tag_ref.color = color.clone();
            }
        }
    }
}

fn sort_payload(payload: &mut SyncPayload) {
    for item in &mut payload.items {
        item.tags.sort_by_key(|tag| tag_key(&tag.uid, &tag.name));
    }
    payload.items.sort_by_key(|i| i.content_hash);
    payload.tags.sort_by_key(|tag| tag_key(&tag.uid, &tag.name));
    payload.deleted_items.sort_by_key(|d| d.content_hash);
    payload
        .deleted_tags
        .sort_by_key(|tag| tag_key(&tag.uid, &tag.name));
    payload.unfavorited_items.sort_by_key(|u| u.content_hash);
}

fn local_unfavorite_is_at_least_as_new(
    db: &Database,
    content_hash: u64,
    remote_updated_at: &str,
) -> bool {
    db.get_unfavorite_deleted_at(content_hash)
        .ok()
        .flatten()
        .is_some_and(|unfavorited_at| !rfc3339_newer(remote_updated_at, &unfavorited_at))
}

fn replace_item_tags(db: &Database, item_id: i64, item: &SyncItem) -> Result<(), String> {
    db.clear_item_tags(item_id)
        .map_err(|e| format!("clear tags: {e}"))?;
    for tag_ref in &item.tags {
        if let Ok(Some(tag)) = resolve_tag_for_ref(db, tag_ref) {
            if let Err(e) = db.add_item_tag(item_id, tag.id) {
                log::warn!("sync: add_item_tag (update) failed: {e}");
            }
        }
    }
    Ok(())
}

/// Merge `other` into `base`. Items use content_hash, tags use stable uid when
/// available and fall back to name for legacy payloads.
pub fn merge_payloads(mut base: SyncPayload, other: SyncPayload) -> SyncPayload {
    // Merge items: keep the newer version for each content_hash
    let mut item_map: std::collections::HashMap<u64, SyncItem> =
        std::collections::HashMap::with_capacity(base.items.len() + other.items.len());
    for item in base.items {
        item_map.insert(item.content_hash, item);
    }
    for item in other.items {
        match item_map.get(&item.content_hash) {
            Some(existing) => {
                if rfc3339_newer(&item.updated_at, &existing.updated_at)
                    || (item.updated_at == existing.updated_at
                        && tag_has_more_stable_refs(&item, existing))
                {
                    item_map.insert(item.content_hash, item);
                }
            }
            None => {
                item_map.insert(item.content_hash, item);
            }
        }
    }
    base.items = item_map.into_values().collect();

    // Merge tags: keep the newer version for each stable identity.
    let mut tag_map: std::collections::HashMap<String, SyncTag> =
        std::collections::HashMap::with_capacity(base.tags.len() + other.tags.len());
    for tag in base.tags {
        tag_map.insert(tag_key(&tag.uid, &tag.name), tag);
    }
    for tag in other.tags {
        let key = tag_key(&tag.uid, &tag.name);
        match tag_map.get(&key) {
            Some(existing) => {
                if sync_tag_preferred(&tag, existing) {
                    tag_map.insert(key, tag);
                }
            }
            None => {
                tag_map.insert(key, tag);
            }
        }
    }
    base.tags = tag_map.into_values().collect();

    // Deduplicate tombstones, keeping the newer deleted_at when keys collide.
    {
        let mut seen: std::collections::HashMap<u64, SyncDeletedItem> = base
            .deleted_items
            .into_iter()
            .map(|d| (d.content_hash, d))
            .collect();
        for d in other.deleted_items {
            match seen.get(&d.content_hash) {
                Some(existing) if rfc3339_newer(&d.deleted_at, &existing.deleted_at) => {
                    seen.insert(d.content_hash, d);
                }
                None => {
                    seen.insert(d.content_hash, d);
                }
                _ => {}
            }
        }
        base.deleted_items = seen.into_values().collect();
    }
    {
        let mut seen: std::collections::HashMap<String, SyncDeletedTag> = base
            .deleted_tags
            .into_iter()
            .map(|d| (tag_key(&d.uid, &d.name), d))
            .collect();
        for d in other.deleted_tags {
            let key = tag_key(&d.uid, &d.name);
            match seen.get(&key) {
                Some(existing) if deleted_tag_preferred(&d, existing) => {
                    seen.insert(key, d);
                }
                None => {
                    seen.insert(key, d);
                }
                _ => {}
            }
        }
        base.deleted_tags = seen.into_values().collect();
    }
    {
        let mut seen: std::collections::HashMap<u64, SyncUnfavoritedItem> = base
            .unfavorited_items
            .into_iter()
            .map(|u| (u.content_hash, u))
            .collect();
        for u in other.unfavorited_items {
            match seen.get(&u.content_hash) {
                Some(existing) if rfc3339_newer(&u.unfavorited_at, &existing.unfavorited_at) => {
                    seen.insert(u.content_hash, u);
                }
                None => {
                    seen.insert(u.content_hash, u);
                }
                _ => {}
            }
        }
        base.unfavorited_items = seen.into_values().collect();
    }

    // --- Use the newer synced_at, comparing as DateTime to handle ---
    // --- variable-length RFC3339 representations correctly. ---
    if rfc3339_newer(&other.synced_at, &base.synced_at) {
        base.synced_at = other.synced_at;
    }

    sanitize_payload(&mut base);

    base
}

/// Parse an RFC3339 string to Utc DateTime, returning None on failure.
fn parse_rfc3339(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    s.parse::<chrono::DateTime<chrono::Utc>>().ok()
}

/// Compare two RFC3339 strings as DateTime values.
/// Returns true if `a` is strictly newer than `b`.
/// Falls back to lexical comparison when either string is unparseable.
fn rfc3339_newer(a: &str, b: &str) -> bool {
    match (parse_rfc3339(a), parse_rfc3339(b)) {
        (Some(ta), Some(tb)) => ta > tb,
        _ => {
            log::warn!(
                "[sync] unparseable RFC3339 timestamp in LWW comparison — \
                 falling back to lexical: a={a:?}, b={b:?}"
            );
            a > b
        }
    }
}

fn rfc3339_newer_or_equal(a: &str, b: &str) -> bool {
    match (parse_rfc3339(a), parse_rfc3339(b)) {
        (Some(ta), Some(tb)) => ta >= tb,
        _ => {
            log::warn!(
                "[sync] unparseable RFC3339 timestamp in LWW comparison — \
                 falling back to lexical: a={a:?}, b={b:?}"
            );
            a >= b
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_payload_roundtrip() {
        let payload = SyncPayload {
            version: crate::core::migration::SYNC_VERSION,
            device_name: "test-pc".into(),
            synced_at: "2026-05-14T10:00:00Z".into(),
            items: vec![SyncItem {
                content_type: "plain_text".into(),
                full_text: "hello".into(),
                content_hash: 12345,
                created_at: "2026-05-13T08:00:00Z".into(),
                updated_at: "2026-05-14T09:00:00Z".into(),
                rich_data: String::new(),
                is_favorite: false,
                note: String::new(),
                size: 0,
                tags: vec![SyncTagRef {
                    uid: "tag-work".into(),
                    name: "work".into(),
                    color: "#EF4444".into(),
                }],
                meta_type: String::new(),
                image_width: 0,
                image_height: 0,
                image_blob: String::new(),
            }],
            tags: vec![SyncTag {
                uid: "tag-work".into(),
                name: "work".into(),
                color: "#EF4444".into(),
                updated_at: String::new(),
            }],
            deleted_items: vec![],
            deleted_tags: vec![],
            unfavorited_items: vec![],
        };

        let json = serde_json::to_string_pretty(&payload).unwrap();
        let parsed: SyncPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, crate::core::migration::SYNC_VERSION);
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.items[0].tags[0].name, "work");
        assert!(parsed.deleted_items.is_empty());
        assert!(parsed.deleted_tags.is_empty());
        assert!(parsed.unfavorited_items.is_empty());
    }

    #[test]
    fn test_v1_backward_compat() {
        // --- v1 JSON (no deleted_items, deleted_tags, or tag updated_at) ---
        let v1_json = r##"{
            "version": 1,
            "device_name": "old-pc",
            "synced_at": "2026-01-01T00:00:00Z",
            "items": [],
            "tags": [{"name": "test", "color": "#FF0000"}]
        }"##;
        let parsed: SyncPayload = serde_json::from_str(v1_json).unwrap();
        assert_eq!(parsed.version, 1);
        assert!(parsed.deleted_items.is_empty());
        assert!(parsed.deleted_tags.is_empty());
        assert_eq!(parsed.tags[0].updated_at, "");
    }

    #[test]
    fn test_merge_stats_default() {
        let stats = MergeStats::default();
        assert_eq!(stats.items_added, 0);
        assert_eq!(stats.items_updated, 0);
        assert_eq!(stats.items_deleted, 0);
        assert_eq!(stats.tags_deleted, 0);
    }

    fn make_payload(items: Vec<SyncItem>, tags: Vec<SyncTag>) -> SyncPayload {
        SyncPayload {
            version: crate::core::migration::SYNC_VERSION,
            device_name: "test".into(),
            synced_at: "2026-05-14T10:00:00Z".into(),
            items,
            tags,
            deleted_items: vec![],
            deleted_tags: vec![],
            unfavorited_items: vec![],
        }
    }

    fn make_item(hash: u64, updated_at: &str, text: &str) -> SyncItem {
        SyncItem {
            content_type: "plain_text".into(),
            full_text: text.into(),
            content_hash: hash,
            created_at: "2026-05-13T08:00:00Z".into(),
            updated_at: updated_at.into(),
            rich_data: String::new(),
            is_favorite: false,
            note: String::new(),
            size: 0,
            tags: vec![],
            meta_type: String::new(),
            image_width: 0,
            image_height: 0,
            image_blob: String::new(),
        }
    }

    #[test]
    fn sanitize_payload_rewrites_image_blob_path_segments() {
        let mut item = make_item(0xABCD, "2026-05-14T09:00:00Z", "image");
        item.content_type = "image".into();
        item.image_blob = "..\\outside.png".into();

        let mut payload = make_payload(vec![item], vec![]);

        sanitize_payload(&mut payload);

        assert_eq!(payload.items[0].image_blob, "000000000000abcd.png");
    }

    #[test]
    fn legacy_thumb_data_is_dropped_when_payload_is_rewritten() {
        let old_json = r##"{
            "version": 4,
            "device_name": "old-device",
            "synced_at": "2026-07-20T08:00:00Z",
            "items": [{
                "content_type": "image",
                "full_text": "shot.png",
                "content_hash": 4660,
                "created_at": "2026-07-20T08:00:00Z",
                "updated_at": "2026-07-20T08:00:00Z",
                "rich_data": "",
                "is_favorite": false,
                "note": "",
                "size": 0,
                "tags": [],
                "meta_type": "",
                "image_width": 24,
                "image_height": 16,
                "image_blob": "0000000000001234.png",
                "thumb_data": "legacy-base64"
            }],
            "tags": [],
            "deleted_items": [],
            "deleted_tags": [],
            "unfavorited_items": []
        }"##;
        let mut payload: SyncPayload =
            serde_json::from_str(old_json).expect("parse legacy payload");

        crate::core::migration::migrate_sync_payload(&mut payload);
        sanitize_payload(&mut payload);
        let rewritten = serde_json::to_string(&payload).expect("serialize migrated payload");

        assert_eq!(payload.version, crate::core::migration::SYNC_VERSION);
        assert!(!rewritten.contains("thumb_data"));
        assert_eq!(payload.items[0].image_blob, "0000000000001234.png");
    }

    #[test]
    fn image_snapshot_exports_filename_instead_of_absolute_path() {
        let db = Database::open(":memory:").expect("open :memory:");
        let item = crate::core::types::ClipboardItem::new_image(
            0,
            r"C:\Users\123\AppData\Local\PixPin\Temp\PixPin_20260720.png",
            0x1234,
            24,
            16,
            None,
        );
        db.upsert(&item).expect("insert image");
        let db = std::sync::Mutex::new(db);

        let payload = build_snapshot(&db, "device-a", false, true).expect("build snapshot");

        assert_eq!(payload.items.len(), 1);
        assert_eq!(payload.items[0].full_text, "PixPin_20260720.png");

        let json = serde_json::to_string(&payload).expect("serialize payload");
        assert!(!json.contains(r"C:\Users\123"));
        assert!(!json.contains("thumb_data"));
    }

    #[test]
    fn image_snapshot_exports_posix_filename_instead_of_absolute_path() {
        let db = Database::open(":memory:").expect("open :memory:");
        let item = crate::core::types::ClipboardItem::new_image(
            0,
            "/Users/alice/Library/Application Support/PixPin/Temp/capture.png",
            0x1235,
            24,
            16,
            None,
        );
        db.upsert(&item).expect("insert image");
        let db = std::sync::Mutex::new(db);

        let payload = build_snapshot(&db, "device-b", false, true).expect("build snapshot");

        assert_eq!(payload.items.len(), 1);
        assert_eq!(payload.items[0].full_text, "capture.png");

        let json = serde_json::to_string(&payload).expect("serialize payload");
        assert!(!json.contains("/Users/alice"));
        assert!(!json.contains("thumb_data"));
    }

    #[test]
    fn payload_semantic_hash_includes_item_text() {
        let mut a = make_payload(
            vec![make_item(0x1111, "2026-05-14T09:00:00Z", "old")],
            vec![],
        );
        let mut b = make_payload(
            vec![make_item(0x1111, "2026-05-14T09:00:00Z", "new")],
            vec![],
        );
        sanitize_payload(&mut a);
        sanitize_payload(&mut b);

        assert_ne!(payload_semantic_hash(&a), payload_semantic_hash(&b));
    }

    fn make_tag(name: &str, color: &str, updated_at: &str) -> SyncTag {
        SyncTag {
            uid: String::new(),
            name: name.into(),
            color: color.into(),
            updated_at: updated_at.into(),
        }
    }

    fn make_tag_with_uid(uid: &str, name: &str, color: &str, updated_at: &str) -> SyncTag {
        SyncTag {
            uid: uid.into(),
            name: name.into(),
            color: color.into(),
            updated_at: updated_at.into(),
        }
    }

    #[test]
    fn test_merge_payloads_newer_item_wins() {
        let base = make_payload(vec![make_item(1, "2026-05-14T09:00:00Z", "hello")], vec![]);
        let other = make_payload(
            vec![make_item(1, "2026-05-14T10:00:00Z", "hello updated")],
            vec![],
        );
        let merged = merge_payloads(base, other);
        assert_eq!(merged.items.len(), 1);
        assert_eq!(merged.items[0].full_text, "hello updated");
        assert_eq!(merged.items[0].updated_at, "2026-05-14T10:00:00Z");
    }

    #[test]
    fn test_merge_payloads_older_item_ignored() {
        let base = make_payload(vec![make_item(1, "2026-05-14T10:00:00Z", "hello")], vec![]);
        let other = make_payload(
            vec![make_item(1, "2026-05-14T09:00:00Z", "hello old")],
            vec![],
        );
        let merged = merge_payloads(base, other);
        assert_eq!(merged.items.len(), 1);
        // --- Base is newer, should be kept ---
        assert_eq!(merged.items[0].full_text, "hello");
    }

    #[test]
    fn test_merge_payloads_different_items_combined() {
        let base = make_payload(vec![make_item(1, "2026-05-14T09:00:00Z", "hello")], vec![]);
        let other = make_payload(vec![make_item(2, "2026-05-14T10:00:00Z", "world")], vec![]);
        let merged = merge_payloads(base, other);
        assert_eq!(merged.items.len(), 2);
    }

    #[test]
    fn test_merge_payloads_tags_deduplicated() {
        let base = make_payload(
            vec![],
            vec![make_tag("work", "#FF0000", "2026-05-14T09:00:00Z")],
        );
        let other = make_payload(
            vec![],
            vec![make_tag("work", "#00FF00", "2026-05-14T10:00:00Z")],
        );
        let merged = merge_payloads(base, other);
        assert_eq!(merged.tags.len(), 1);
        assert_eq!(merged.tags[0].color, "#00FF00");
        assert_eq!(merged.tags[0].updated_at, "2026-05-14T10:00:00Z");
    }

    #[test]
    fn test_merge_payloads_canonicalizes_tag_rename_without_item_timestamp_bump() {
        let mut old_item = make_item(1, "2026-05-14T09:00:00Z", "hello");
        old_item.tags = vec![SyncTagRef {
            uid: "tag-1".into(),
            name: "old".into(),
            color: "#FF0000".into(),
        }];
        let base = make_payload(
            vec![old_item],
            vec![make_tag_with_uid(
                "tag-1",
                "old",
                "#FF0000",
                "2026-05-14T09:00:00Z",
            )],
        );

        let mut renamed_item = make_item(1, "2026-05-14T09:00:00Z", "hello");
        renamed_item.tags = vec![SyncTagRef {
            uid: "tag-1".into(),
            name: "new".into(),
            color: "#00FF00".into(),
        }];
        let other = make_payload(
            vec![renamed_item],
            vec![make_tag_with_uid(
                "tag-1",
                "new",
                "#00FF00",
                "2026-05-14T10:00:00Z",
            )],
        );

        let merged = merge_payloads(base, other);
        assert_eq!(merged.tags.len(), 1);
        assert_eq!(merged.tags[0].name, "new");
        assert_eq!(merged.items.len(), 1);
        assert_eq!(merged.items[0].updated_at, "2026-05-14T09:00:00Z");
        assert_eq!(merged.items[0].tags[0].uid, "tag-1");
        assert_eq!(merged.items[0].tags[0].name, "new");
        assert_eq!(merged.items[0].tags[0].color, "#00FF00");
    }

    #[test]
    fn test_merge_remote_renames_tag_by_uid() {
        let db = Database::open(":memory:").expect("open :memory:");
        db.create_tag_with_uid_and_timestamp("tag-1", "old", "#FF0000", "2026-05-14T09:00:00Z")
            .expect("create tag");
        let db = std::sync::Mutex::new(db);
        let mut remote = make_payload(
            vec![],
            vec![make_tag_with_uid(
                "tag-1",
                "new",
                "#00FF00",
                "2026-05-14T10:00:00Z",
            )],
        );

        let stats = merge_remote_into_local(&db, &mut remote, "local").expect("merge");
        assert_eq!(stats.tags_added, 0);
        let db = db.lock().unwrap();
        let tag = db
            .get_tag_by_uid("tag-1")
            .expect("lookup")
            .expect("tag exists");
        assert_eq!(tag.name, "new");
        assert_eq!(tag.color, "#00FF00");
    }

    #[test]
    fn test_merge_payloads_tombstones_deduplicated() {
        let mut base = make_payload(vec![], vec![]);
        base.deleted_items = vec![SyncDeletedItem {
            content_hash: 1,
            deleted_at: "2026-05-14T09:00:00Z".into(),
            device_name: "a".into(),
        }];
        let mut other = make_payload(vec![], vec![]);
        other.deleted_items = vec![
            SyncDeletedItem {
                content_hash: 1,
                deleted_at: "2026-05-14T10:00:00Z".into(),
                device_name: "b".into(),
            },
            SyncDeletedItem {
                content_hash: 2,
                deleted_at: "2026-05-14T10:00:00Z".into(),
                device_name: "b".into(),
            },
        ];
        let merged = merge_payloads(base, other);
        // --- Hash 1 should be deduplicated, hash 2 is new ---
        assert_eq!(merged.deleted_items.len(), 2);
        assert!(merged.deleted_items.iter().any(|d| d.content_hash == 1));
        assert!(merged.deleted_items.iter().any(|d| d.content_hash == 2));
    }

    // --- ── build_snapshot: unfavorite filtering ── ---

    /// Helper: create an in-memory Database with schema and a single item.
    fn setup_db() -> (std::sync::Mutex<Database>, i64, u64) {
        let db = Database::open(":memory:").expect("open :memory:");
        let now = chrono::Utc::now().to_rfc3339();
        let item = SyncItem {
            content_type: "plain_text".into(),
            full_text: "hello".into(),
            content_hash: 0xABCD,
            created_at: now.clone(),
            updated_at: now.clone(),
            rich_data: String::new(),
            is_favorite: false,
            note: String::new(),
            size: 5,
            tags: vec![],
            meta_type: String::new(),
            image_width: 0,
            image_height: 0,
            image_blob: String::new(),
        };
        let id = db.insert_sync_item_raw(&item).expect("insert");
        (std::sync::Mutex::new(db), id, 0xABCD)
    }

    /// Helper: insert a sync item with given params into DB, return (db, id, hash).
    fn insert_item(
        db: &Database,
        text: &str,
        hash: u64,
        is_favorite: bool,
        updated_at: &str,
    ) -> i64 {
        let item = SyncItem {
            content_type: "plain_text".into(),
            full_text: text.into(),
            content_hash: hash,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: updated_at.into(),
            rich_data: String::new(),
            is_favorite,
            note: String::new(),
            size: text.len() as i64,
            tags: vec![],
            meta_type: String::new(),
            image_width: 0,
            image_height: 0,
            image_blob: String::new(),
        };
        db.insert_sync_item_raw(&item).expect("insert")
    }

    fn days_ago(days: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339()
    }

    fn seconds_ago(seconds: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::seconds(seconds)).to_rfc3339()
    }

    #[test]
    fn test_snapshot_unfav_with_tombstone_excluded_from_items() {
        let (db_mutex, _id, hash) = setup_db();
        {
            let db = db_mutex.lock().unwrap();
            // --- Simulate: item was favorited, then unfavorited ---
            db.set_favorite(_id, true).unwrap(); // was favorited
            db.record_unfavorite(hash, &seconds_ago(1), "test-device")
                .unwrap();
            // --- Now unfavorite it (simulating toggle_favorite was_fav=true branch) ---
            db.set_favorite(_id, false).unwrap();
        }

        // --- Build snapshot with favorites_only=false ---
        let payload = build_snapshot(&db_mutex, "test-device", false, false).unwrap();

        // --- Item should NOT be in items[] (tombstone communicates unfavorite) ---
        assert!(
            payload
                .items
                .iter()
                .find(|i| i.content_hash == hash)
                .is_none(),
            "unfavorited item with tombstone should be excluded from items[]"
        );
        // --- Tombstone SHOULD be in unfavorited_items[] ---
        assert_eq!(payload.unfavorited_items.len(), 1);
        assert_eq!(payload.unfavorited_items[0].content_hash, hash);
    }

    #[test]
    fn test_snapshot_unfav_no_tombstone_stays_in_items() {
        let (db_mutex, _id, hash) = setup_db();
        // --- Item has is_favorite=false, no tombstone (never favorited) ---

        let payload = build_snapshot(&db_mutex, "test-device", false, false).unwrap();

        // --- Item SHOULD be in items[] — it's just a normal unfavorited item ---
        let found = payload.items.iter().find(|i| i.content_hash == hash);
        assert!(
            found.is_some(),
            "unfavorited item without tombstone should be in items[]"
        );
        assert!(!found.unwrap().is_favorite);
        // --- No tombstone ---
        assert!(payload.unfavorited_items.is_empty());
    }

    #[test]
    fn test_snapshot_favorited_always_included() {
        let (db_mutex, _id, hash) = setup_db();
        {
            let db = db_mutex.lock().unwrap();
            db.set_favorite(_id, true).unwrap();
        }

        let payload = build_snapshot(&db_mutex, "test-device", false, false).unwrap();

        let found = payload.items.iter().find(|i| i.content_hash == hash);
        assert!(
            found.is_some(),
            "favorited item should always be in items[]"
        );
        assert!(found.unwrap().is_favorite);
    }

    #[test]
    fn test_snapshot_favorites_only_excludes_unfavorited() {
        let (db_mutex, _id, hash) = setup_db();
        // --- Item is_favorite=false, no tombstone ---

        let payload = build_snapshot(&db_mutex, "test-device", true, false).unwrap();

        // --- With favorites_only=true, unfavorited items should be excluded ---
        assert!(payload
            .items
            .iter()
            .find(|i| i.content_hash == hash)
            .is_none());
    }

    #[test]
    fn test_snapshot_refavorite_clears_tombstone_restores_item() {
        let (db_mutex, _id, hash) = setup_db();
        {
            let db = db_mutex.lock().unwrap();
            // --- Simulate: favorite → unfavorite → refavorite ---
            db.set_favorite(_id, true).unwrap();
            db.record_unfavorite(hash, "2026-06-06T10:00:00Z", "test-device")
                .unwrap();
            db.set_favorite(_id, false).unwrap();
            // --- Now refavorite: remove tombstone ---
            db.remove_unfavorite(hash).unwrap();
            db.set_favorite(_id, true).unwrap();
        }

        let payload = build_snapshot(&db_mutex, "test-device", false, false).unwrap();

        // --- Item should be back in items[] as favorited ---
        let found = payload.items.iter().find(|i| i.content_hash == hash);
        assert!(found.is_some(), "re-favorited item should be in items[]");
        assert!(found.unwrap().is_favorite);
        // --- No tombstone ---
        assert!(payload.unfavorited_items.is_empty());
    }

    #[test]
    fn test_snapshot_mixed_states() {
        let db = Database::open(":memory:").expect("open :memory:");
        let db = std::sync::Mutex::new(db);

        // --- Item A: favorited (hash=1) ---
        let recent = days_ago(1);
        insert_item(&db.lock().unwrap(), "item a", 1, true, &recent);
        // --- Item B: unfavorited with tombstone (hash=2) ---
        insert_item(&db.lock().unwrap(), "item b", 2, false, &recent);
        // --- Item C: unfavorited without tombstone (hash=3) ---
        insert_item(&db.lock().unwrap(), "item c", 3, false, &recent);

        {
            let db = db.lock().unwrap();
            db.record_unfavorite(2, &recent, "test-device").unwrap();
        }

        let payload = build_snapshot(&db, "test-device", false, false).unwrap();

        // --- Item A: in items[] ---
        assert!(payload.items.iter().any(|i| i.content_hash == 1));
        // --- Item B: NOT in items[], in tombstone ---
        assert!(payload.items.iter().find(|i| i.content_hash == 2).is_none());
        assert!(payload
            .unfavorited_items
            .iter()
            .any(|u| u.content_hash == 2));
        // --- Item C: in items[] ---
        assert!(payload.items.iter().any(|i| i.content_hash == 3));
        assert_eq!(payload.items.len(), 2); // A + C
        assert_eq!(payload.unfavorited_items.len(), 1); // B
    }

    #[test]
    fn test_snapshot_tombstone_without_item_still_present() {
        // --- Edge case: tombstone exists but item doesn't (item was deleted) ---
        let db = Database::open(":memory:").expect("open :memory:");
        {
            let db = &db;
            db.record_unfavorite(0xDEAD, &seconds_ago(1), "test-device")
                .unwrap();
        }
        let db = std::sync::Mutex::new(db);

        let payload = build_snapshot(&db, "test-device", false, false).unwrap();

        // --- Tombstone still propagates even without the item ---
        assert_eq!(payload.unfavorited_items.len(), 1);
        assert_eq!(payload.unfavorited_items[0].content_hash, 0xDEAD);
        assert!(payload.items.is_empty());
    }

    #[test]
    fn test_snapshot_favorited_with_tombstone_included() {
        // --- Edge case: item is favorited but also has a tombstone ---
        // (shouldn't happen normally, but if it does, is_favorite wins)
        let (db_mutex, _id, hash) = setup_db();
        {
            let db = db_mutex.lock().unwrap();
            db.set_favorite(_id, true).unwrap();
            db.record_unfavorite(hash, "2026-06-06T10:00:00Z", "test-device")
                .unwrap();
        }

        let payload = build_snapshot(&db_mutex, "test-device", false, false).unwrap();

        // Favorited item should be in items[] even if a stale tombstone exists
        let found = payload.items.iter().find(|i| i.content_hash == hash);
        assert!(
            found.is_some(),
            "favorited item should be in items[] despite tombstone"
        );
        assert!(found.unwrap().is_favorite);
    }

    // --- ── merge_remote_into_local tests ── ---

    fn make_remote_payload(
        items: Vec<SyncItem>,
        unfavorited_items: Vec<SyncUnfavoritedItem>,
    ) -> SyncPayload {
        SyncPayload {
            version: crate::core::migration::SYNC_VERSION,
            device_name: "remote-device".into(),
            synced_at: "2026-05-15T10:00:00Z".into(),
            items,
            tags: vec![],
            deleted_items: vec![],
            deleted_tags: vec![],
            unfavorited_items,
        }
    }

    #[test]
    fn test_merge_unfavorite_applied_to_favorited_item() {
        let db = Database::open(":memory:").expect("open :memory:");
        // --- Local: favorited item with hash=100, updated_at T1 (older) ---
        insert_item(&db, "favorited", 100, true, "2026-05-01T10:00:00Z");
        let db = std::sync::Mutex::new(db);

        // --- Remote: unfavorite marker at T2 (newer than local item) ---
        let mut remote = make_remote_payload(
            vec![],
            vec![SyncUnfavoritedItem {
                content_hash: 100,
                unfavorited_at: "2026-06-06T10:00:00Z".into(),
                device_name: "remote-device".into(),
            }],
        );

        let stats = merge_remote_into_local(&db, &mut remote, "local-device").unwrap();

        assert_eq!(stats.items_updated, 1);
        // --- Local item should now be unfavorited ---
        let db = db.lock().unwrap();
        let item = db.get_by_hash(100).unwrap().unwrap();
        assert!(!item.is_favorite, "item should be unfavorited after merge");
        // --- Tombstone should be recorded locally ---
        assert!(db.is_item_unfavorited(100).unwrap());
    }

    #[test]
    fn test_merge_unfavorite_ignored_when_already_unfavorited() {
        let db = Database::open(":memory:").expect("open :memory:");
        // --- Local: already unfavorited item ---
        insert_item(&db, "unfavorited", 100, false, "2026-06-06T10:00:00Z");
        let db = std::sync::Mutex::new(db);

        let mut remote = make_remote_payload(
            vec![],
            vec![SyncUnfavoritedItem {
                content_hash: 100,
                unfavorited_at: "2026-05-02T10:00:00Z".into(),
                device_name: "remote-device".into(),
            }],
        );

        let stats = merge_remote_into_local(&db, &mut remote, "local-device").unwrap();

        // --- No update needed — already unfavorited ---
        assert_eq!(stats.items_updated, 0);
    }

    #[test]
    fn test_merge_unfavorite_older_timestamp_ignored() {
        let db = Database::open(":memory:").expect("open :memory:");
        // --- Local: favorited item at T2 (newer than remote — user re-favorited) ---
        insert_item(&db, "favorited", 100, true, "2026-06-06T10:00:00Z");
        let db = std::sync::Mutex::new(db);

        // --- Remote: unfavorite marker at T1 (older than local item) ---
        let mut remote = make_remote_payload(
            vec![],
            vec![SyncUnfavoritedItem {
                content_hash: 100,
                unfavorited_at: "2026-05-01T10:00:00Z".into(), // older
                device_name: "remote-device".into(),
            }],
        );

        let stats = merge_remote_into_local(&db, &mut remote, "local-device").unwrap();

        // --- Should NOT unfavorite — local is newer (user re-favorited) ---
        assert_eq!(stats.items_updated, 0);
        let db = db.lock().unwrap();
        let item = db.get_by_hash(100).unwrap().unwrap();
        assert!(item.is_favorite, "item should stay favorited (local newer)");
    }

    #[test]
    fn test_merge_own_unfavorite_ignored() {
        let db = Database::open(":memory:").expect("open :memory:");
        insert_item(&db, "favorited", 100, true, "2026-06-06T10:00:00Z");
        let db = std::sync::Mutex::new(db);

        // --- Remote tombstone from the SAME device ---
        let mut remote = make_remote_payload(
            vec![],
            vec![SyncUnfavoritedItem {
                content_hash: 100,
                unfavorited_at: "2026-05-02T10:00:00Z".into(),
                device_name: "local-device".into(), // same device!
            }],
        );

        let stats = merge_remote_into_local(&db, &mut remote, "local-device").unwrap();

        // --- Should ignore own tombstone ---
        assert_eq!(stats.items_updated, 0);
    }

    #[test]
    fn test_merge_remote_favorite_promotes_newer_local_duplicate() {
        let db = Database::open(":memory:").expect("open :memory:");
        insert_item(&db, "same content", 100, false, "2026-06-06T10:00:00Z");
        let db = std::sync::Mutex::new(db);

        let mut remote = make_remote_payload(
            vec![SyncItem {
                content_type: "plain_text".into(),
                full_text: "same content".into(),
                content_hash: 100,
                created_at: "2026-05-01T08:00:00Z".into(),
                updated_at: "2026-05-01T10:00:00Z".into(),
                rich_data: String::new(),
                is_favorite: true,
                note: "saved on device A".into(),
                size: 12,
                tags: vec![],
                meta_type: String::new(),
                image_width: 0,
                image_height: 0,
                image_blob: String::new(),
            }],
            vec![],
        );

        let stats = merge_remote_into_local(&db, &mut remote, "local-device").unwrap();

        assert_eq!(stats.items_updated, 1);
        let db = db.lock().unwrap();
        let item = db.get_by_hash(100).unwrap().unwrap();
        assert!(item.is_favorite);
        assert_eq!(item.note, "saved on device A");
        assert_eq!(
            item.updated_at,
            "2026-06-06T10:00:00Z"
                .parse::<chrono::DateTime<chrono::Utc>>()
                .unwrap()
        );
    }

    #[test]
    fn test_merge_remote_favorite_respects_newer_local_unfavorite() {
        let db = Database::open(":memory:").expect("open :memory:");
        let local_updated_at = seconds_ago(1);
        let local_unfavorited_at = seconds_ago(2);
        let remote_updated_at = days_ago(1);
        insert_item(&db, "same content", 100, false, &local_updated_at);
        db.record_unfavorite(100, &local_unfavorited_at, "local-device")
            .unwrap();
        let db = std::sync::Mutex::new(db);

        let mut remote = make_remote_payload(
            vec![SyncItem {
                content_type: "plain_text".into(),
                full_text: "same content".into(),
                content_hash: 100,
                created_at: "2026-05-01T08:00:00Z".into(),
                updated_at: remote_updated_at,
                rich_data: String::new(),
                is_favorite: true,
                note: "saved on device A".into(),
                size: 12,
                tags: vec![],
                meta_type: String::new(),
                image_width: 0,
                image_height: 0,
                image_blob: String::new(),
            }],
            vec![],
        );

        let stats = merge_remote_into_local(&db, &mut remote, "local-device").unwrap();

        assert_eq!(stats.items_updated, 0);
        let db = db.lock().unwrap();
        let item = db.get_by_hash(100).unwrap().unwrap();
        assert!(!item.is_favorite);
        assert!(item.note.is_empty());
    }

    #[test]
    fn test_merge_unfavorite_no_local_item_records_tombstone() {
        // Remote tombstone for item that doesn't exist locally
        let db = Database::open(":memory:").expect("open :memory:");
        let db = std::sync::Mutex::new(db);

        let mut remote = make_remote_payload(
            vec![],
            vec![SyncUnfavoritedItem {
                content_hash: 999,
                unfavorited_at: "2026-05-02T10:00:00Z".into(),
                device_name: "remote-device".into(),
            }],
        );

        let stats = merge_remote_into_local(&db, &mut remote, "local-device").unwrap();

        // Should record tombstone for propagation even without the item
        assert_eq!(stats.items_updated, 0);
        let db = db.lock().unwrap();
        assert!(db.is_item_unfavorited(999).unwrap());
    }

    #[test]
    fn test_merge_item_fav_false_with_tombstone() {
        // Simulates the "both lists" scenario: remote has item with is_favorite=false
        // AND a tombstone. Phase 2.5 should apply the unfavorite, Phase 4 should update.
        let db = Database::open(":memory:").expect("open :memory:");
        // --- Local: favorited item at T1 (older) ---
        insert_item(&db, "favorited", 100, true, "2026-05-01T10:00:00Z");
        let db = std::sync::Mutex::new(db);

        let mut remote = make_remote_payload(
            vec![SyncItem {
                content_type: "plain_text".into(),
                full_text: "favorited".into(),
                content_hash: 100,
                created_at: "2026-05-01T08:00:00Z".into(),
                updated_at: "2026-06-06T10:00:00Z".into(), // T2 > T1 (newer)
                rich_data: String::new(),
                is_favorite: false,
                note: String::new(),
                size: 9,
                tags: vec![],
                meta_type: String::new(),
                image_width: 0,
                image_height: 0,
                image_blob: String::new(),
            }],
            vec![SyncUnfavoritedItem {
                content_hash: 100,
                unfavorited_at: "2026-06-06T10:00:00Z".into(),
                device_name: "remote-device".into(),
            }],
        );

        let stats = merge_remote_into_local(&db, &mut remote, "local-device").unwrap();

        // --- Phase 2.5 unfavorites, Phase 4 updates — both count ---
        assert!(stats.items_updated >= 1);
        let db = db.lock().unwrap();
        let item = db.get_by_hash(100).unwrap().unwrap();
        assert!(!item.is_favorite, "item should be unfavorited after merge");
        // Tombstone should be recorded for propagation
        assert!(db.is_item_unfavorited(100).unwrap());
    }
}
