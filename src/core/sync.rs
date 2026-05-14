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
    pub source_app_name: String,
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
}

/// Result of a merge operation reported back to UI.
#[derive(Debug, Clone, Default)]
pub struct MergeStats {
    pub items_added: u32,
    pub items_updated: u32,
    pub tags_added: u32,
}

// ── Snapshot building ──

/// Build a full `SyncPayload` from the local database.
/// Excludes image and file type items (images/files not synced in v1).
pub fn build_snapshot(db: &Mutex<Database>, device_name: &str) -> Result<SyncPayload, String> {
    let db = db.lock().map_err(|e| format!("db lock: {e}"))?;
    let items = db
        .get_all_sync_items_with_tags()
        .map_err(|e| format!("query items: {e}"))?;

    let mut sync_items: Vec<SyncItem> = Vec::with_capacity(items.len());
    let mut all_tags: Vec<SyncTag> = Vec::new();

    for item in items {
        let tags: Vec<SyncTagRef> = item
            .tags
            .iter()
            .map(|t| SyncTagRef {
                name: t.name.clone(),
                color: t.color.clone(),
            })
            .collect();

        // Collect unique global tags
        for t in &item.tags {
            if !all_tags.iter().any(|gt| gt.name == t.name) {
                all_tags.push(SyncTag {
                    name: t.name.clone(),
                    color: t.color.clone(),
                });
            }
        }

        sync_items.push(SyncItem {
            content_type: item.content_type.as_str().to_string(),
            full_text: item.full_text,
            content_hash: item.content_hash,
            created_at: item.created_at.to_rfc3339(),
            updated_at: item.updated_at.to_rfc3339(),
            rich_data: item.rich_data,
            is_favorite: item.is_favorite,
            note: item.note,
            source_app_name: item.source_app_name,
            tags,
        });
    }

    Ok(SyncPayload {
        version: 1,
        device_name: device_name.to_string(),
        synced_at: chrono::Utc::now().to_rfc3339(),
        items: sync_items,
        tags: all_tags,
    })
}

// ── Merge logic ──

/// Merge remote sync payload into the local database.
/// Conflict resolution: latest `updated_at` timestamp wins.
/// Tags are matched by name (unique in the tags table).
pub fn merge_remote_into_local(db: &Mutex<Database>, remote: &SyncPayload) -> Result<MergeStats, String> {
    let db = db.lock().map_err(|e| format!("db lock: {e}"))?;
    let mut stats = MergeStats::default();

    // Phase 1: Ensure all remote tags exist locally (match by name)
    for remote_tag in &remote.tags {
        if db.get_tag_by_name(&remote_tag.name)
            .map_err(|e| format!("tag lookup: {e}"))?
            .is_none()
        {
            db.create_tag(&remote_tag.name, &remote_tag.color)
                .map_err(|e| format!("create tag: {e}"))?;
            stats.tags_added += 1;
        }
    }

    // Phase 2: Merge items by content_hash
    for remote_item in &remote.items {
        let local = db
            .get_by_hash(remote_item.content_hash)
            .map_err(|e| format!("hash lookup: {e}"))?;

        match local {
            None => {
                // New item from remote — insert
                let item_id = insert_sync_item(&db, remote_item)?;
                stats.items_added += 1;

                // Attach tags
                for tag_ref in &remote_item.tags {
                    if let Ok(Some(tag)) = db.get_tag_by_name(&tag_ref.name) {
                        let _ = db.add_item_tag(item_id, tag.id);
                    }
                }
            }
            Some(local_item) => {
                // Parse both timestamps
                let remote_ts = remote_item.updated_at.parse::<chrono::DateTime<chrono::Utc>>().ok();
                let local_ts = Some(local_item.updated_at);

                if remote_ts > local_ts {
                    // Remote is newer — update local
                    db.update_sync_item(
                        local_item.id,
                        &remote_item.full_text,
                        &remote_item.content_type,
                        remote_item.updated_at.clone(),
                        &remote_item.rich_data,
                        remote_item.is_favorite,
                        &remote_item.note,
                        &remote_item.source_app_name,
                    )
                    .map_err(|e| format!("update item: {e}"))?;

                    // Replace tag associations
                    db.clear_item_tags(local_item.id)
                        .map_err(|e| format!("clear tags: {e}"))?;
                    for tag_ref in &remote_item.tags {
                        if let Ok(Some(tag)) = db.get_tag_by_name(&tag_ref.name) {
                            let _ = db.add_item_tag(local_item.id, tag.id);
                        }
                    }

                    stats.items_updated += 1;
                }
                // else: local is newer or equal — keep local
            }
        }
    }

    Ok(stats)
}

/// Insert a SyncItem as a new row in clipboard_items.
/// Returns the new row id.
fn insert_sync_item(db: &Database, item: &SyncItem) -> Result<i64, String> {
    let created_at: chrono::DateTime<chrono::Utc> = item
        .created_at
        .parse()
        .unwrap_or_else(|_| chrono::Utc::now());
    let updated_at: chrono::DateTime<chrono::Utc> = item
        .updated_at
        .parse()
        .unwrap_or_else(|_| chrono::Utc::now());

    db.insert_sync_item_raw(
        &item.full_text,
        &item.content_type,
        item.content_hash,
        created_at,
        updated_at,
        &item.rich_data,
        item.is_favorite,
        &item.note,
        &item.source_app_name,
    )
    .map_err(|e| format!("insert item: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_payload_roundtrip() {
        let payload = SyncPayload {
            version: 1,
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
                source_app_name: "Notepad".into(),
                tags: vec![SyncTagRef {
                    name: "work".into(),
                    color: "#EF4444".into(),
                }],
            }],
            tags: vec![SyncTag {
                name: "work".into(),
                color: "#EF4444".into(),
            }],
        };

        let json = serde_json::to_string_pretty(&payload).unwrap();
        let parsed: SyncPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.items[0].tags[0].name, "work");
    }

    #[test]
    fn test_merge_stats_default() {
        let stats = MergeStats::default();
        assert_eq!(stats.items_added, 0);
        assert_eq!(stats.items_updated, 0);
    }
}
