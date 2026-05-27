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
    /// Pull the remote payload. When `bypass_cache` is true, the backend
    /// should skip any mtime/etag optimization and always read the file.
    fn pull(&self, bypass_cache: bool) -> Result<SyncPayload, String>;
    fn push(&self, payload: &SyncPayload) -> Result<(), String>;

    /// Called after a successful push. Backends can override to clean up
    /// temporary or conflict files (e.g., clippi_sync-*.json).
    fn post_push_cleanup(&self) -> Result<(), String> {
        Ok(())
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
    /// Plain-text subtype: "" | "email" | "phone".
    #[serde(default)]
    pub meta_type: String,
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

// ── Snapshot building ──

/// Build a full `SyncPayload` from the local database.
/// Excludes image and file type items.
/// When `favorites_only` is true, only favorited items are included.
/// Only tags referenced by the synced items are included.
pub fn build_snapshot(
    db: &Mutex<Database>,
    device_name: &str,
    favorites_only: bool,
) -> Result<SyncPayload, String> {
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
            full_text: item.full_text.clone(),
            content_hash: item.content_hash,
            created_at: item.created_at.to_rfc3339(),
            updated_at: item.updated_at.to_rfc3339(),
            rich_data: item.rich_data,
            is_favorite: item.is_favorite,
            note: item.note,
            size: item.size,
            tags,
            meta_type: item.meta_type.clone(),
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

    Ok(SyncPayload {
        version: crate::core::migration::SYNC_VERSION,
        device_name: device_name.to_string(),
        synced_at: chrono::Utc::now().to_rfc3339(),
        items: sync_items,
        tags: all_tags,
        deleted_items,
        deleted_tags,
        unfavorited_items,
    })
}

// ── Merge logic ──

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

    // Phase 2.5: Process remote unfavorite markers
    for uf in &remote.unfavorited_items {
        if uf.device_name == local_device_name {
            continue; // own unfavorite, already handled
        }
        if db.is_item_unfavorited(uf.content_hash).unwrap_or(false) {
            continue; // already marked
        }
        // Record locally for propagation
        let _ = db.record_unfavorite(uf.content_hash, &uf.unfavorited_at, &uf.device_name);
        // Unfavorite local item if it exists and is still favorited
        if let Ok(Some(local_item)) = db.get_by_hash(uf.content_hash) {
            if local_item.is_favorite {
                let remote_ts = parse_rfc3339(&uf.unfavorited_at);
                if remote_ts.is_some_and(|r| r > local_item.updated_at) {
                    let _ = db.set_favorite(local_item.id, false);
                }
            }
        }
    }

    // Collect remote tag names — if a tombstone and a tag share the same name,
    // the remote device recreated the tag and the tag should take precedence.
    let remote_tag_names: std::collections::HashSet<&str> =
        remote.tags.iter().map(|t| t.name.as_str()).collect();

    // Phase 2: Process remote tag tombstones
    for tombstone in &remote.deleted_tags {
        if tombstone.device_name == local_device_name {
            continue; // own deletion
        }
        // Skip tombstone if the remote payload also includes a tag with the same
        // name — the sender recreated this tag after deleting it.
        if remote_tag_names.contains(tombstone.name.as_str()) {
            continue;
        }
        if db.is_tag_tombstoned(&tombstone.name).unwrap_or(false) {
            continue; // already tombstoned
        }
        let _ = db.record_tag_deletion(
            &tombstone.name,
            &tombstone.deleted_at,
            &tombstone.device_name,
        );
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
        // If a different device deleted this tag, respect the deletion.
        // If the tombstone is from the SAME device that sent this payload,
        // the sender recreated the tag — clear the tombstone and proceed.
        if db
            .is_tag_tombstoned_by_other_device(&remote_tag.name, &remote.device_name)
            .unwrap_or(false)
        {
            continue;
        }
        // Clear any tombstone from the sender so the tag can be recreated.
        let _ = db.remove_tag_tombstone(&remote_tag.name);
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
                let item_id = db
                    .insert_sync_item_raw(remote_item)
                    .map_err(|e| format!("insert item: {e}"))?;
                stats.items_added += 1;

                for tag_ref in &remote_item.tags {
                    if let Ok(Some(tag)) = db.get_tag_by_name(&tag_ref.name) {
                        let _ = db.add_item_tag(item_id, tag.id);
                    }
                }

                // Restore remote timestamp: add_item_tag may have bumped it.
                let _ = db.set_item_updated_at(item_id, &remote_item.updated_at);
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

                    // Restore remote timestamp: tag operations above may have
                    // bumped updated_at via touch_item, but the item data is
                    // semantically identical to what we just pulled from remote.
                    let _ = db.set_item_updated_at(local_item.id, &remote_item.updated_at);

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
        item.content_hash.hash(&mut h);
        item.updated_at.hash(&mut h);
        item.is_favorite.hash(&mut h);
        item.note.hash(&mut h);
        item.tags.len().hash(&mut h);
        for tag in &item.tags {
            tag.name.hash(&mut h);
            tag.color.hash(&mut h);
        }
    }
    payload.items.len().hash(&mut h);
    for tag in &payload.tags {
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

/// Merge `other` into `base`. For items (by content_hash) and tags (by name),
/// the version with the newer updated_at wins. Tombstones and unfavorite markers
/// are deduplicated by their natural keys.
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
                if item.updated_at.as_str() > existing.updated_at.as_str() {
                    item_map.insert(item.content_hash, item);
                }
            }
            None => {
                item_map.insert(item.content_hash, item);
            }
        }
    }
    base.items = item_map.into_values().collect();

    // Merge tags: keep the newer color for each name
    let mut tag_map: std::collections::HashMap<String, SyncTag> =
        std::collections::HashMap::with_capacity(base.tags.len() + other.tags.len());
    for tag in base.tags {
        tag_map.insert(tag.name.clone(), tag);
    }
    for tag in other.tags {
        match tag_map.get(&tag.name) {
            Some(existing) => {
                if tag.updated_at.as_str() > existing.updated_at.as_str() {
                    tag_map.insert(tag.name.clone(), tag);
                }
            }
            None => {
                tag_map.insert(tag.name.clone(), tag);
            }
        }
    }
    base.tags = tag_map.into_values().collect();

    // Deduplicate tombstones
    {
        let mut seen: std::collections::HashSet<u64> =
            base.deleted_items.iter().map(|d| d.content_hash).collect();
        for d in other.deleted_items {
            if seen.insert(d.content_hash) {
                base.deleted_items.push(d);
            }
        }
    }
    {
        let mut seen: std::collections::HashSet<String> =
            base.deleted_tags.iter().map(|d| d.name.clone()).collect();
        for d in other.deleted_tags {
            if seen.insert(d.name.clone()) {
                base.deleted_tags.push(d);
            }
        }
    }
    {
        let mut seen: std::collections::HashSet<u64> = base
            .unfavorited_items
            .iter()
            .map(|u| u.content_hash)
            .collect();
        for u in other.unfavorited_items {
            if seen.insert(u.content_hash) {
                base.unfavorited_items.push(u);
            }
        }
    }

    // Use the newer synced_at and device_name from whichever payload has it
    if other.synced_at > base.synced_at {
        base.synced_at = other.synced_at;
    }

    base
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
                    name: "work".into(),
                    color: "#EF4444".into(),
                }],
                meta_type: String::new(),
            }],
            tags: vec![SyncTag {
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
        }
    }

    fn make_tag(name: &str, color: &str, updated_at: &str) -> SyncTag {
        SyncTag {
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
        // Base is newer, should be kept
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
        // Hash 1 should be deduplicated, hash 2 is new
        assert_eq!(merged.deleted_items.len(), 2);
        assert!(merged.deleted_items.iter().any(|d| d.content_hash == 1));
        assert!(merged.deleted_items.iter().any(|d| d.content_hash == 2));
    }
}
