//! Cloud sync data types and merge logic.
//!
//! The sync format is a single JSON file (`clippi_sync.json`) placed in a
//! cloud-synced folder (OneDrive, iCloud, Dropbox, etc.). The same format
//! can later be used with a WebDAV backend by swapping the transport layer.

use crate::core::db::Database;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Backend transport type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BackendType {
    LocalFolder,
}

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
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn backend_type(&self) -> BackendType;
    fn check_status(&self) -> BackendStatus;
    fn pull(&self) -> Result<SyncPayload, String>;
    fn push(&self, payload: &SyncPayload) -> Result<(), String>;
}

/// Top-level sync payload stored as JSON on the cloud folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPayload {
    pub version: u32,
    pub device_name: String,
    pub synced_at: String, // RFC3339
    pub items: Vec<SyncItem>,
    pub tags: Vec<SyncTag>,
    /// Deleted item tombstones (v2+).
    #[serde(default)]
    pub deleted_items: Vec<SyncDeletedItem>,
    /// Deleted tag tombstones (v2+).
    #[serde(default)]
    pub deleted_tags: Vec<SyncDeletedTag>,
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
    /// Tag associations carried on the item.
    #[serde(default)]
    pub tags: Vec<SyncTagRef>,
}

/// Tag reference embedded in a SyncItem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTagRef {
    pub name: String,
    pub color: String,
}

/// Global tag definition in the top-level tags array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTag {
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
    pub name: String,
    pub deleted_at: String, // RFC3339
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

// ── Snapshot building ──

/// Build a full `SyncPayload` from the local database.
/// Excludes image and file type items.
/// When `favorites_only` is true, only favorited items are included.
/// Only tags referenced by the synced items are included.
pub fn build_snapshot(db: &Mutex<Database>, device_name: &str, favorites_only: bool) -> Result<SyncPayload, String> {
    let db = db.lock().map_err(|e| format!("db lock: {e}"))?;

    // Collect all live synced items
    let items = db
        .get_all_sync_items_with_tags()
        .map_err(|e| format!("query items: {e}"))?;

    let mut sync_items: Vec<SyncItem> = Vec::with_capacity(items.len());
    let mut used_tag_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    for item in items {
        if favorites_only && !item.is_favorite {
            continue;
        }

        let tags: Vec<SyncTagRef> = item
            .tags
            .iter()
            .map(|t| {
                used_tag_names.insert(t.name.clone());
                SyncTagRef {
                    name: t.name.clone(),
                    color: t.color.clone(),
                }
            })
            .collect();

        sync_items.push(SyncItem {
            content_type: item.content_type.as_str().to_string(),
            full_text: item.full_text,
            content_hash: item.content_hash,
            created_at: item.created_at.to_rfc3339(),
            updated_at: item.updated_at.to_rfc3339(),
            rich_data: item.rich_data,
            is_favorite: item.is_favorite,
            note: item.note,
            tags,
        });
    }

    // Only include tags that are referenced by the synced items
    let all_tags: Vec<SyncTag> = if used_tag_names.is_empty() {
        Vec::new()
    } else {
        db.get_all_tags()
            .map_err(|e| format!("query all tags: {e}"))?
            .into_iter()
            .filter(|t| used_tag_names.contains(&t.name))
            .map(|t| SyncTag {
                name: t.name,
                color: t.color,
                updated_at: t.updated_at,
            })
            .collect()
    };

    // Collect recent tombstones (30-day window)
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
        .map(|(name, at, dev)| SyncDeletedTag {
            name,
            deleted_at: at,
            device_name: dev,
        })
        .collect();

    Ok(SyncPayload {
        version: 2,
        device_name: device_name.to_string(),
        synced_at: chrono::Utc::now().to_rfc3339(),
        items: sync_items,
        tags: all_tags,
        deleted_items,
        deleted_tags,
    })
}

// ── Merge logic ──

/// Merge remote sync payload into the local database (v2, 4-phase).
///
/// Phases (in order):
/// 0. Clean expired local tombstones
/// 1. Process remote item tombstones — delete local items if tombstone is newer
/// 2. Process remote tag tombstones — delete local tags if tombstone is newer
/// 3. Merge tags — create/update with last-writer-wins color resolution
/// 4. Merge items — create/update with last-writer-wins, skip tombstoned
pub fn merge_remote_into_local(
    db: &Mutex<Database>,
    remote: &SyncPayload,
    local_device_name: &str,
) -> Result<MergeStats, String> {
    let mut db = db.lock().map_err(|e| format!("db lock: {e}"))?;
    let mut stats = MergeStats::default();

    // Phase 0: Clean expired local tombstones
    let _ = db.cleanup_old_tombstones(30);

    // Phase 1: Process remote item tombstones
    for tombstone in &remote.deleted_items {
        if tombstone.device_name == local_device_name {
            continue; // own deletion, already handled
        }
        if db
            .is_item_tombstoned(tombstone.content_hash)
            .unwrap_or(false)
        {
            continue; // already tombstoned
        }
        // Record tombstone locally for propagation
        let _ = db.record_item_deletion(
            tombstone.content_hash,
            &tombstone.deleted_at,
            &tombstone.device_name,
        );
        // Delete local item if exists and is older than the tombstone
        if let Ok(Some(local_item)) = db.get_by_hash(tombstone.content_hash) {
            let remote_ts = parse_rfc3339(&tombstone.deleted_at);
            if remote_ts.is_some_and(|r| r > local_item.updated_at)
                && db
                    .delete_item_by_hash(tombstone.content_hash)
                    .unwrap_or(false)
            {
                stats.items_deleted += 1;
            }
        }
    }

    // Phase 2: Process remote tag tombstones
    for tombstone in &remote.deleted_tags {
        if tombstone.device_name == local_device_name {
            continue; // own deletion
        }
        if db
            .is_tag_tombstoned(&tombstone.name)
            .unwrap_or(false)
        {
            continue; // already tombstoned
        }
        let _ = db.record_tag_deletion(&tombstone.name, &tombstone.deleted_at, &tombstone.device_name);
        // Delete local tag if exists and is older than the tombstone
        if let Ok(Some(local_tag)) = db.get_tag_by_name(&tombstone.name) {
            let remote_ts = parse_rfc3339(&tombstone.deleted_at);
            let local_ts = parse_rfc3339(&local_tag.updated_at);
            if remote_ts.is_some_and(|r| local_ts.is_none_or(|l| r > l)) {
                // Drop immutable ref before mutable delete
                drop(local_tag);
                if db.delete_tag_by_name(&tombstone.name).unwrap_or(false) {
                    stats.tags_deleted += 1;
                }
            }
        }
    }

    // Phase 3: Merge tags — create or update with color conflict resolution
    for remote_tag in &remote.tags {
        if db
            .is_tag_tombstoned(&remote_tag.name)
            .unwrap_or(false)
        {
            continue; // tag was deleted
        }
        match db
            .get_tag_by_name(&remote_tag.name)
            .map_err(|e| format!("tag lookup: {e}"))?
        {
            None => {
                // New tag from remote
                if remote_tag.updated_at.is_empty() {
                    db.create_tag(&remote_tag.name, &remote_tag.color)
                } else {
                    db.create_tag_with_timestamp(
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
                        db.update_tag_with_timestamp(
                            local_tag.id,
                            &remote_tag.name,
                            &remote_tag.color,
                            &remote_tag.updated_at,
                        )
                        .map_err(|e| format!("update tag: {e}"))?;
                    }
                }
            }
        }
    }

    // Phase 4: Merge items by content_hash (last-writer-wins, skip tombstoned)
    for remote_item in &remote.items {
        if db
            .is_item_tombstoned(remote_item.content_hash)
            .unwrap_or(false)
        {
            continue; // locally tombstoned, don't re-import
        }
        let local = db
            .get_by_hash(remote_item.content_hash)
            .map_err(|e| format!("hash lookup: {e}"))?;

        match local {
            None => {
                // New item from remote — insert
                let item_id = db.insert_sync_item_raw(remote_item)
                    .map_err(|e| format!("insert item: {e}"))?;
                stats.items_added += 1;

                for tag_ref in &remote_item.tags {
                    if let Ok(Some(tag)) = db.get_tag_by_name(&tag_ref.name) {
                        let _ = db.add_item_tag(item_id, tag.id);
                    }
                }
            }
            Some(local_item) => {
                let remote_ts = parse_rfc3339(&remote_item.updated_at);
                let local_ts = Some(local_item.updated_at);

                if remote_ts > local_ts {
                    db.update_sync_item(local_item.id, remote_item)
                        .map_err(|e| format!("update item: {e}"))?;

                    db.clear_item_tags(local_item.id)
                        .map_err(|e| format!("clear tags: {e}"))?;
                    for tag_ref in &remote_item.tags {
                        if let Ok(Some(tag)) = db.get_tag_by_name(&tag_ref.name) {
                            let _ = db.add_item_tag(local_item.id, tag.id);
                        }
                    }

                    stats.items_updated += 1;
                }
            }
        }
    }

    Ok(stats)
}

/// Parse an RFC3339 string to Utc DateTime, returning None on failure.
fn parse_rfc3339(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    s.parse::<chrono::DateTime<chrono::Utc>>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_payload_roundtrip() {
        let payload = SyncPayload {
            version: 2,
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
                tags: vec![SyncTagRef {
                    name: "work".into(),
                    color: "#EF4444".into(),
                }],
            }],
            tags: vec![SyncTag {
                name: "work".into(),
                color: "#EF4444".into(),
                updated_at: String::new(),
            }],
            deleted_items: vec![],
            deleted_tags: vec![],
        };

        let json = serde_json::to_string_pretty(&payload).unwrap();
        let parsed: SyncPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, 2);
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.items[0].tags[0].name, "work");
        assert!(parsed.deleted_items.is_empty());
        assert!(parsed.deleted_tags.is_empty());
    }

    #[test]
    fn test_v1_backward_compat() {
        // v1 JSON (no deleted_items, deleted_tags, or tag updated_at)
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
}
