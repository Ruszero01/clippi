//! Database persistence for clipboard items

use crate::core::filters::ClipboardFilters;
use crate::core::types::{ClipboardItem, ContentType, TagInfo};
use rusqlite::{params, Connection, Result as SqlResult};

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &str) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        // Wait up to 5s if the database is locked (e.g. previous process still exiting).
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // WAL mode improves concurrency and reduces memory pressure.
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA cache_size = -2000;")?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    /// Flush WAL to main database file before copy/migration.
    pub fn checkpoint(&self) -> SqlResult<()> {
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
    }

    fn init_schema(&self) -> SqlResult<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS clipboard_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                content_type TEXT NOT NULL DEFAULT 'text',
                full_text TEXT NOT NULL,
                content_hash INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                image_path TEXT NOT NULL DEFAULT '',
                rich_data TEXT NOT NULL DEFAULT '',
                file_data TEXT NOT NULL DEFAULT '',
                is_favorite INTEGER NOT NULL DEFAULT 0,
                note TEXT NOT NULL DEFAULT '',
                source_app_name TEXT NOT NULL DEFAULT '',
                source_app_icon TEXT NOT NULL DEFAULT '',
                image_width INTEGER NOT NULL DEFAULT 0,
                image_height INTEGER NOT NULL DEFAULT 0,
                size INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_hash ON clipboard_items(content_hash);
            CREATE INDEX IF NOT EXISTS idx_updated ON clipboard_items(updated_at DESC);",
        )?;
        // Tags tables
        let _ = self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tags (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                color TEXT NOT NULL DEFAULT '',
                updated_at TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS item_tags (
                tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                item_id INTEGER NOT NULL REFERENCES clipboard_items(id) ON DELETE CASCADE,
                used_at TEXT NOT NULL,
                PRIMARY KEY (tag_id, item_id)
            );
            CREATE INDEX IF NOT EXISTS idx_item_tags_item ON item_tags(item_id);
            CREATE INDEX IF NOT EXISTS idx_item_tags_tag ON item_tags(tag_id);",
        );
        // Tombstone tables for sync deletion propagation
        let _ = self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS deleted_items (
                content_hash INTEGER NOT NULL,
                deleted_at TEXT NOT NULL,
                device_name TEXT NOT NULL DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_del_items_hash ON deleted_items(content_hash);
            CREATE INDEX IF NOT EXISTS idx_del_items_at ON deleted_items(deleted_at);
            CREATE TABLE IF NOT EXISTS deleted_tags (
                name TEXT NOT NULL,
                deleted_at TEXT NOT NULL,
                device_name TEXT NOT NULL DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_del_tags_name ON deleted_tags(name);
            CREATE INDEX IF NOT EXISTS idx_del_tags_at ON deleted_tags(deleted_at);
            CREATE TABLE IF NOT EXISTS unfavorited_items (
                content_hash INTEGER NOT NULL,
                unfavorited_at TEXT NOT NULL,
                device_name TEXT NOT NULL DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_uf_items_hash ON unfavorited_items(content_hash);
            CREATE INDEX IF NOT EXISTS idx_uf_items_at ON unfavorited_items(unfavorited_at);",
        );

        // Schema migration: add UNIQUE constraints to tombstone tables
        // Without UNIQUE, INSERT OR IGNORE always inserts, causing unbounded growth.
        let version: i64 = self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version < 1 {
            // Deduplicate existing rows (keep earliest entry per key)
            self.conn.execute_batch(
                "DELETE FROM deleted_items WHERE rowid NOT IN (SELECT MIN(rowid) FROM deleted_items GROUP BY content_hash);
                 DELETE FROM deleted_tags WHERE rowid NOT IN (SELECT MIN(rowid) FROM deleted_tags GROUP BY name);
                 DELETE FROM unfavorited_items WHERE rowid NOT IN (SELECT MIN(rowid) FROM unfavorited_items GROUP BY content_hash);",
            )?;
            // Add unique indexes so INSERT OR IGNORE actually ignores duplicates
            self.conn.execute_batch(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_del_items_hash_uq ON deleted_items(content_hash);
                 CREATE UNIQUE INDEX IF NOT EXISTS idx_del_tags_name_uq ON deleted_tags(name);
                 CREATE UNIQUE INDEX IF NOT EXISTS idx_uf_items_hash_uq ON unfavorited_items(content_hash);",
            )?;
            self.conn.pragma_update(None, "user_version", 1)?;
        }
        Ok(())
    }

    pub fn upsert(&self, item: &ClipboardItem) -> SqlResult<()> {
        let changed = self.conn.execute(
            "UPDATE clipboard_items SET updated_at = ?1, image_path = ?3, rich_data = ?4, file_data = ?5, image_width = ?6, image_height = ?7, size = ?8 WHERE content_hash = ?2",
            params![item.updated_at.to_rfc3339(), item.content_hash as i64, item.image_path, item.rich_data, item.file_data, item.image_width as i64, item.image_height as i64, item.size],
        )?;
        if changed == 0 {
            self.conn.execute(
                "INSERT INTO clipboard_items (content_type, full_text, content_hash, created_at, updated_at, image_path, rich_data, file_data, source_app_name, source_app_icon, image_width, image_height, size)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    item.content_type.as_str(),
                    item.full_text,
                    item.content_hash as i64,
                    item.created_at.to_rfc3339(),
                    item.updated_at.to_rfc3339(),
                    item.image_path,
                    item.rich_data,
                    item.file_data,
                    item.source_app_name,
                    item.source_app_icon,
                    item.image_width as i64,
                    item.image_height as i64,
                    item.size,
                ],
            )?;
        }
        Ok(())
    }

    /// Load items with unified filter support.
    /// Uses ClipboardFilters to build WHERE clause with AND logic across all filter dimensions.
    pub fn load_filtered(
        &self,
        filters: &ClipboardFilters,
        limit: usize,
        order_by: &str,
    ) -> SqlResult<Vec<ClipboardItem>> {
        let (where_clause, mut filter_params) = filters.db_where();
        let query = format!(
            "SELECT id, content_type, full_text, content_hash, created_at, updated_at, image_path, rich_data, file_data, is_favorite, note, source_app_name, source_app_icon, image_width, image_height, size
             FROM clipboard_items {} ORDER BY {} DESC LIMIT ?",
            where_clause, order_by
        );
        filter_params.push((limit as i64).into());
        let mut stmt = self.conn.prepare(&query)?;
        let items = stmt.query_map(rusqlite::params_from_iter(filter_params), row_to_item)?;
        items.collect()
    }

    pub fn get_by_id(&self, id: i64) -> SqlResult<Option<ClipboardItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content_type, full_text, content_hash, created_at, updated_at, image_path, rich_data, file_data, is_favorite, note, source_app_name, source_app_icon, image_width, image_height, size
             FROM clipboard_items WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_item(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_by_hash(&self, hash: u64) -> SqlResult<Option<ClipboardItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content_type, full_text, content_hash, created_at, updated_at, image_path, rich_data, file_data, is_favorite, note, source_app_name, source_app_icon, image_width, image_height, size
             FROM clipboard_items WHERE content_hash = ?1",
        )?;
        let mut rows = stmt.query(params![hash as i64])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_item(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn toggle_favorite(&self, id: i64) -> SqlResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE clipboard_items SET is_favorite = CASE WHEN is_favorite = 0 THEN 1 ELSE 0 END, updated_at = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        Ok(())
    }

    /// Set favorite status explicitly (used in sync merge).
    pub fn set_favorite(&self, id: i64, value: bool) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE clipboard_items SET is_favorite = ?2 WHERE id = ?1",
            params![id, value as i32],
        )?;
        Ok(())
    }

    pub fn delete_item(&self, id: i64) -> SqlResult<()> {
        self.conn.execute(
            "DELETE FROM clipboard_items WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn update_note(&self, id: i64, note: &str) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE clipboard_items SET note = ?1 WHERE id = ?2",
            params![note, id],
        )?;
        Ok(())
    }

    pub fn update_content(&self, id: i64, text: &str, content_type: &str) -> SqlResult<()> {
        let hash = {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&text, &mut hasher);
            std::hash::Hasher::finish(&hasher)
        };
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE clipboard_items SET full_text = ?1, content_hash = ?2, content_type = ?3, updated_at = ?4, rich_data = '', image_path = '', file_data = '' WHERE id = ?5",
            params![text, hash as i64, content_type, now, id],
        )?;
        Ok(())
    }

    // ── Tag CRUD ──

    pub fn create_tag(&self, name: &str, color: &str) -> SqlResult<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO tags (name, color, updated_at) VALUES (?1, ?2, ?3)",
            params![name, color, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_tag(&mut self, tag_id: i64) -> SqlResult<()> {
        let tx = self.conn.transaction()?;
        // Touch affected items before removing the associations
        tx.execute(
            "UPDATE clipboard_items SET updated_at = ?1
             WHERE id IN (SELECT item_id FROM item_tags WHERE tag_id = ?2)",
            rusqlite::params![chrono::Utc::now().to_rfc3339(), tag_id],
        )?;
        tx.execute("DELETE FROM item_tags WHERE tag_id = ?1", params![tag_id])?;
        tx.execute("DELETE FROM tags WHERE id = ?1", params![tag_id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn update_tag(&self, tag_id: i64, name: &str, color: &str) -> SqlResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE tags SET name = ?1, color = ?2, updated_at = ?3 WHERE id = ?4",
            params![name, color, now, tag_id],
        )?;
        Ok(())
    }

    pub fn get_all_tags(&self) -> SqlResult<Vec<TagInfo>> {
        let mut stmt = self.conn.prepare("SELECT id, name, color, updated_at FROM tags ORDER BY id DESC")?;
        let rows = stmt.query_map([], |row| {
            let updated_str: String = row.get::<_, String>(3).unwrap_or_default();
            Ok(TagInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
                updated_at: updated_str,
            })
        })?;
        rows.collect()
    }

    pub fn get_tags_for_item(&self, item_id: i64) -> SqlResult<Vec<TagInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.name, t.color, t.updated_at FROM tags t \
             INNER JOIN item_tags it ON t.id = it.tag_id \
             WHERE it.item_id = ?1 ORDER BY it.used_at DESC",
        )?;
        let rows = stmt.query_map(params![item_id], |row| {
            let updated_str: String = row.get::<_, String>(3).unwrap_or_default();
            Ok(TagInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
                updated_at: updated_str,
            })
        })?;
        rows.collect()
    }

    pub fn get_tags_for_items(&self, item_ids: &[i64]) -> SqlResult<std::collections::HashMap<i64, Vec<TagInfo>>> {
        let mut map: std::collections::HashMap<i64, Vec<TagInfo>> = std::collections::HashMap::new();
        if item_ids.is_empty() {
            return Ok(map);
        }
        let placeholders: Vec<String> = item_ids.iter().map(|_| "?".to_string()).collect();
        let query = format!(
            "SELECT it.item_id, t.id, t.name, t.color, t.updated_at FROM tags t \
             INNER JOIN item_tags it ON t.id = it.tag_id \
             WHERE it.item_id IN ({}) ORDER BY it.used_at DESC",
            placeholders.join(",")
        );
        let mut stmt = self.conn.prepare(&query)?;
        let params: Vec<rusqlite::types::Value> = item_ids.iter().map(|&id| (id).into()).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
            let updated_str: String = row.get::<_, String>(4).unwrap_or_default();
            Ok((row.get::<_, i64>(0)?, TagInfo {
                id: row.get(1)?,
                name: row.get(2)?,
                color: row.get(3)?,
                updated_at: updated_str,
            }))
        })?;
        for row in rows {
            let (item_id, tag) = row?;
            map.entry(item_id).or_default().push(tag);
        }
        Ok(map)
    }

    pub fn add_item_tag(&self, item_id: i64, tag_id: i64) -> SqlResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let changed = self.conn.execute(
            "INSERT OR IGNORE INTO item_tags (tag_id, item_id, used_at) VALUES (?1, ?2, ?3)",
            params![tag_id, item_id, now],
        )?;
        if changed > 0 {
            self.touch_item(item_id)?;
        }
        Ok(())
    }

    pub fn remove_item_tag(&self, item_id: i64, tag_id: i64) -> SqlResult<()> {
        let deleted = self.conn.execute(
            "DELETE FROM item_tags WHERE tag_id = ?1 AND item_id = ?2",
            params![tag_id, item_id],
        )?;
        if deleted > 0 {
            self.touch_item(item_id)?;
        }
        Ok(())
    }

    pub fn clear_item_tags(&self, item_id: i64) -> SqlResult<()> {
        let deleted = self.conn.execute(
            "DELETE FROM item_tags WHERE item_id = ?1",
            params![item_id],
        )?;
        if deleted > 0 {
            self.touch_item(item_id)?;
        }
        Ok(())
    }

    pub fn get_tag_by_name(&self, name: &str) -> SqlResult<Option<TagInfo>> {
        let mut stmt = self.conn.prepare("SELECT id, name, color, updated_at FROM tags WHERE name = ?1")?;
        let mut rows = stmt.query(params![name])?;
        if let Some(row) = rows.next()? {
            let updated_str: String = row.get::<_, String>(3).unwrap_or_default();
            Ok(Some(TagInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
                updated_at: updated_str,
            }))
        } else {
            Ok(None)
        }
    }

    /// Load items with tags pre-filled via batch query
    pub fn load_filtered_with_tags(
        &self,
        filters: &ClipboardFilters,
        limit: usize,
        order_by: &str,
    ) -> SqlResult<Vec<ClipboardItem>> {
        let mut items = self.load_filtered(filters, limit, order_by)?;
        if !items.is_empty() {
            let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
            let tag_map = self.get_tags_for_items(&ids)?;
            for item in &mut items {
                item.tags = tag_map.get(&item.id).cloned().unwrap_or_default();
            }
        }
        Ok(items)
    }

    pub fn get_by_id_with_tags(&self, id: i64) -> SqlResult<Option<ClipboardItem>> {
        if let Some(mut item) = self.get_by_id(id)? {
            item.tags = self.get_tags_for_item(id)?;
            Ok(Some(item))
        } else {
            Ok(None)
        }
    }

    // ── Sync helpers ──

    /// Get all items (excluding image and file types) with tags for sync snapshot.
    pub fn get_all_sync_items_with_tags(&self) -> SqlResult<Vec<ClipboardItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content_type, full_text, content_hash, created_at, updated_at,
             image_path, rich_data, file_data, is_favorite, note, source_app_name, source_app_icon, image_width, image_height, size
             FROM clipboard_items
             WHERE content_type NOT IN ('image', 'file')
             ORDER BY updated_at DESC",
        )?;
        let mut items: Vec<ClipboardItem> = stmt
            .query_map([], row_to_item)?
            .collect::<SqlResult<Vec<_>>>()?;
        if !items.is_empty() {
            let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
            let tag_map = self.get_tags_for_items(&ids)?;
            for item in &mut items {
                item.tags = tag_map.get(&item.id).cloned().unwrap_or_default();
            }
        }
        Ok(items)
    }

    /// Insert a full item row (used during sync merge for new remote items).
    pub fn insert_sync_item_raw(
        &self,
        item: &crate::core::sync::SyncItem,
    ) -> SqlResult<i64> {
        let created_at: chrono::DateTime<chrono::Utc> = item
            .created_at
            .parse()
            .unwrap_or_else(|_| chrono::Utc::now());
        let updated_at: chrono::DateTime<chrono::Utc> = item
            .updated_at
            .parse()
            .unwrap_or_else(|_| chrono::Utc::now());

        self.conn.execute(
            "INSERT INTO clipboard_items (content_type, full_text, content_hash, created_at, updated_at,
             rich_data, is_favorite, note, source_app_name, size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                item.content_type,
                item.full_text,
                item.content_hash as i64,
                created_at.to_rfc3339(),
                updated_at.to_rfc3339(),
                item.rich_data,
                item.is_favorite as i32,
                item.note,
                "", // source_app_name not in sync payload
                item.size,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Update an item's fields from a newer remote version (sync merge).
    pub fn update_sync_item(
        &self,
        id: i64,
        item: &crate::core::sync::SyncItem,
    ) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE clipboard_items SET full_text = ?1, content_type = ?2, updated_at = ?3,
             rich_data = ?4, is_favorite = ?5, note = ?6, size = ?7
             WHERE id = ?8",
            rusqlite::params![
                item.full_text,
                item.content_type,
                item.updated_at,
                item.rich_data,
                item.is_favorite as i32,
                item.note,
                item.size,
                id,
            ],
        )?;
        Ok(())
    }

    /// Bump updated_at on an item (called after tag changes to mark dirty for sync).
    pub fn touch_item(&self, item_id: i64) -> SqlResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE clipboard_items SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now, item_id],
        )?;
        Ok(())
    }

    /// Set updated_at to an explicit value (used after sync merge to preserve
    /// remote timestamp when tag re-application has bumped it to now).
    pub fn set_item_updated_at(&self, item_id: i64, timestamp: &str) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE clipboard_items SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![timestamp, item_id],
        )?;
        Ok(())
    }

    // ── Tombstone operations ──

    /// Record a deleted item tombstone for sync propagation.
    pub fn record_item_deletion(&self, content_hash: u64, deleted_at: &str, device_name: &str) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO deleted_items (content_hash, deleted_at, device_name) VALUES (?1, ?2, ?3)",
            params![content_hash as i64, deleted_at, device_name],
        )?;
        Ok(())
    }

    /// Record a deleted tag tombstone for sync propagation.
    pub fn record_tag_deletion(&self, name: &str, deleted_at: &str, device_name: &str) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO deleted_tags (name, deleted_at, device_name) VALUES (?1, ?2, ?3)",
            params![name, deleted_at, device_name],
        )?;
        Ok(())
    }

    /// Get item tombstones newer than N days for sync snapshot.
    pub fn get_deleted_items_recent(&self, days: i64) -> SqlResult<Vec<(u64, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT content_hash, deleted_at, device_name FROM deleted_items WHERE deleted_at >= strftime('%Y-%m-%dT%H:%M:%S', 'now', ?1)",
        )?;
        let rows = stmt.query_map(params![format!("-{days} days")], |row| {
            Ok((row.get::<_, i64>(0)? as u64, row.get(1)?, row.get(2)?))
        })?;
        rows.collect()
    }

    /// Get tag tombstones newer than N days for sync snapshot.
    pub fn get_deleted_tags_recent(&self, days: i64) -> SqlResult<Vec<(String, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, deleted_at, device_name FROM deleted_tags WHERE deleted_at >= strftime('%Y-%m-%dT%H:%M:%S', 'now', ?1)",
        )?;
        let rows = stmt.query_map(params![format!("-{days} days")], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        rows.collect()
    }

    /// Prune oldest non-favorite items when total exceeds max_items.
    /// Returns the ids of deleted items. max_items == 0 means unlimited.
    pub fn prune_excess_non_favorites(&self, max_items: u32) -> SqlResult<Vec<i64>> {
        if max_items == 0 {
            return Ok(Vec::new());
        }
        let non_fav_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM clipboard_items WHERE is_favorite = 0",
            [],
            |row| row.get(0),
        )?;
        if non_fav_count <= max_items as i64 {
            return Ok(Vec::new());
        }
        let excess = (non_fav_count - max_items as i64) as usize;
        let mut stmt = self.conn.prepare(
            "SELECT id FROM clipboard_items WHERE is_favorite = 0 ORDER BY created_at ASC",
        )?;
        let all_ids: Vec<i64> = stmt.query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        let pruned_ids: Vec<i64> = all_ids.iter().take(excess).copied().collect();
        if pruned_ids.is_empty() {
            return Ok(Vec::new());
        }
        // Delete item_tags first (CASCADE may not be enforced without PRAGMA foreign_keys = ON),
        // then delete clipboard_items. Use chunked IN clauses to stay within SQLITE_MAX_VARIABLE_NUMBER.
        let tx = self.conn.unchecked_transaction()?;
        for chunk in pruned_ids.chunks(500) {
            let placeholders: Vec<String> = chunk.iter().map(|_| "?".to_string()).collect();
            let ph = placeholders.join(",");
            let params: Vec<rusqlite::types::Value> = chunk.iter().map(|&id| (id).into()).collect();
            tx.execute(
                &format!("DELETE FROM item_tags WHERE item_id IN ({})", ph),
                rusqlite::params_from_iter(params.iter()),
            )?;
            tx.execute(
                &format!("DELETE FROM clipboard_items WHERE id IN ({})", ph),
                rusqlite::params_from_iter(params.iter()),
            )?;
        }
        tx.commit()?;
        Ok(pruned_ids)
    }

    /// Clean up tombstones older than N days.
    pub fn cleanup_old_tombstones(&self, days: i64) -> SqlResult<()> {
        let cutoff = format!("-{days} days");
        self.conn.execute(
            "DELETE FROM deleted_items WHERE deleted_at < strftime('%Y-%m-%dT%H:%M:%S', 'now', ?1)",
            params![&cutoff],
        )?;
        self.conn.execute(
            "DELETE FROM deleted_tags WHERE deleted_at < strftime('%Y-%m-%dT%H:%M:%S', 'now', ?1)",
            params![&cutoff],
        )?;
        self.conn.execute(
            "DELETE FROM unfavorited_items WHERE unfavorited_at < strftime('%Y-%m-%dT%H:%M:%S', 'now', ?1)",
            params![&cutoff],
        )?;
        Ok(())
    }

    /// Record an unfavorite marker for sync propagation.
    pub fn record_unfavorite(&self, content_hash: u64, unfavorited_at: &str, device_name: &str) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO unfavorited_items (content_hash, unfavorited_at, device_name) VALUES (?1, ?2, ?3)",
            params![content_hash as i64, unfavorited_at, device_name],
        )?;
        Ok(())
    }

    /// Remove an unfavorite marker (item was re-favorited).
    pub fn remove_unfavorite(&self, content_hash: u64) -> SqlResult<()> {
        self.conn.execute(
            "DELETE FROM unfavorited_items WHERE content_hash = ?1",
            params![content_hash as i64],
        )?;
        Ok(())
    }

    /// Get unfavorited items newer than N days for sync snapshot.
    pub fn get_unfavorited_recent(&self, days: i64) -> SqlResult<Vec<(u64, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT content_hash, unfavorited_at, device_name FROM unfavorited_items WHERE unfavorited_at >= strftime('%Y-%m-%dT%H:%M:%S', 'now', ?1)",
        )?;
        let rows = stmt.query_map(params![format!("-{days} days")], |row| {
            Ok((row.get::<_, i64>(0)? as u64, row.get(1)?, row.get(2)?))
        })?;
        rows.collect()
    }

    /// Check if an item has a local tombstone (prevents re-import after delete).
    pub fn is_item_tombstoned(&self, content_hash: u64) -> SqlResult<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM deleted_items WHERE content_hash = ?1",
            params![content_hash as i64],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Check if an item already has an unfavorite marker.
    pub fn is_item_unfavorited(&self, content_hash: u64) -> SqlResult<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM unfavorited_items WHERE content_hash = ?1",
            params![content_hash as i64],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Check if a tag has a local tombstone.
    pub fn is_tag_tombstoned(&self, name: &str) -> SqlResult<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM deleted_tags WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Delete a local item by content_hash (triggered by remote tombstone).
    pub fn delete_item_by_hash(&self, content_hash: u64) -> SqlResult<bool> {
        let affected = self.conn.execute(
            "DELETE FROM clipboard_items WHERE content_hash = ?1",
            params![content_hash as i64],
        )?;
        Ok(affected > 0)
    }

    /// Delete a local tag by name (triggered by remote tombstone).
    /// Also cleans up item_tags associations and touches affected items.
    pub fn delete_tag_by_name(&mut self, name: &str) -> SqlResult<bool> {
        let tx = self.conn.transaction()?;
        // Touch affected items
        tx.execute(
            "UPDATE clipboard_items SET updated_at = ?1
             WHERE id IN (SELECT it.item_id FROM item_tags it
                          INNER JOIN tags t ON t.id = it.tag_id
                          WHERE t.name = ?2)",
            rusqlite::params![chrono::Utc::now().to_rfc3339(), name],
        )?;
        // Delete item_tags associations
        tx.execute(
            "DELETE FROM item_tags WHERE tag_id IN (SELECT id FROM tags WHERE name = ?1)",
            params![name],
        )?;
        // Delete the tag
        let affected = tx.execute("DELETE FROM tags WHERE name = ?1", params![name])?;
        tx.commit()?;
        Ok(affected > 0)
    }

    /// Update tag with explicit timestamp (for sync merge).
    pub fn update_tag_with_timestamp(&self, id: i64, name: &str, color: &str, updated_at: &str) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE tags SET name = ?1, color = ?2, updated_at = ?3 WHERE id = ?4",
            params![name, color, updated_at, id],
        )?;
        Ok(())
    }

    /// Create tag with explicit timestamp (for sync merge).
    /// Caller must ensure the tag does not already exist.
    pub fn create_tag_with_timestamp(&self, name: &str, color: &str, updated_at: &str) -> SqlResult<i64> {
        self.conn.execute(
            "INSERT INTO tags (name, color, updated_at) VALUES (?1, ?2, ?3)",
            params![name, color, updated_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }
}

fn row_to_item(row: &rusqlite::Row<'_>) -> SqlResult<ClipboardItem> {
    let ct_str: String = row.get(1)?;
    // Lazy reclassification: legacy "link" items that are actually file paths → "path"
    let ct_str = if ct_str == "link" {
        let full_text: String = row.get(2)?;
        if crate::core::types::is_path(&full_text) {
            "path".to_string()
        } else {
            ct_str
        }
    } else {
        ct_str
    };
    let created_str: String = row.get(4)?;
    let updated_str: String = row.get(5)?;
    let image_path: String = row.get(6).unwrap_or_default();
    let rich_data: String = row.get(7).unwrap_or_default();
    let file_data: String = row.get(8).unwrap_or_default();
    let is_favorite: i32 = row.get(9).unwrap_or(0);
    let note: String = row.get(10).unwrap_or_default();
    let source_app_name: String = row.get(11).unwrap_or_default();
    let source_app_icon: String = row.get(12).unwrap_or_default();
    let image_width: i32 = row.get(13).unwrap_or(0);
    let image_height: i32 = row.get(14).unwrap_or(0);
    let size: i64 = row.get(15).unwrap_or(0);
    Ok(ClipboardItem {
        id: row.get(0)?,
        content_type: ContentType::from_str(&ct_str),
        full_text: row.get(2)?,
        content_hash: row.get::<_, i64>(3)? as u64,
        created_at: created_str.parse().unwrap_or_default(),
        updated_at: updated_str.parse().unwrap_or_default(),
        image_path,
        image_width: image_width as u32,
        image_height: image_height as u32,
        rich_data,
        file_data,
        is_favorite: is_favorite != 0,
        note,
        source_app_name,
        source_app_icon,
        size,
        tags: Vec::new(), // filled later via batch query
    })
}
