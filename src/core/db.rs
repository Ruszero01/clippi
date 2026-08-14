//! Database persistence for clipboard items

use crate::core::cache_cleanup::{
    CleanupSyncScope, ClearClipboardResult, ConfirmedStaleItem, DeleteItemsResult,
    StaleItemCandidate,
};
use crate::core::filters::ClipboardFilters;
use crate::core::types::{ClipboardItem, ContentType, FileData, TagInfo};
use rusqlite::{params, Connection, OptionalExtension, Result as SqlResult};
use std::path::Path;
use uuid::Uuid;

/// Statistics returned by [`Database::merge_from`].
#[derive(Debug, Default)]
pub struct MergeStats {
    pub items_added: usize,
    pub items_updated: usize,
    pub tags_added: usize,
    pub tags_updated: usize,
}

/// Titlebar filter availability + clearable counts, loaded in a single
/// database round trip (previously two EXISTS + two COUNT queries).
#[derive(Debug, Clone, Copy, Default)]
pub struct TitlebarStats {
    pub has_hotkey_items: bool,
    pub has_favorite_items: bool,
    pub clearable_history_count: u32,
    pub clearable_non_favorite_history_count: u32,
}

pub struct Database {
    conn: Connection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunedClipboardItem {
    pub id: i64,
    pub content_hash: u64,
    pub content_type: ContentType,
    pub is_favorite: bool,
    pub custom_hotkey: String,
    pub file_data: String,
}

const SOURCE_APP_ICON_INLINE_LIMIT: usize = 256 * 1024;
const LIST_FULL_TEXT_LIMIT: usize = 8192;
const LIST_RICH_HTML_LIMIT: usize = 4096;
const LIST_RICH_AUX_LIMIT: usize = 2048;
const LIST_NOTE_LIMIT: usize = 2048;

/// Upper bound for a sane consecutive-missing observation count. Values
/// outside `0..=MAX` (e.g. a corrupt negative i64 cast to u32) are treated
/// as no history at all so they can never satisfy the delete threshold.
const STALE_OBSERVATION_COUNT_MAX: i64 = 1_000;

pub fn legacy_tag_uid(name: &str) -> String {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("clippi-tag:{name}").as_bytes(),
    )
    .to_string()
}

fn new_tag_uid() -> String {
    Uuid::new_v4().to_string()
}

fn source_app_icon_cache_key(app_name: &str) -> Option<String> {
    let safe: String = app_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    (!safe.is_empty()).then_some(format!("icons/{safe}"))
}

fn file_icon_cache_key(file_path: &str, is_dir: bool) -> String {
    let ext_lower = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_else(|| "file".to_string());

    if crate::platform::source::extension_has_embedded_icon(&ext_lower) {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        file_path.hash(&mut hasher);
        format!("file_icons/{ext_lower}_{:016x}", hasher.finish())
    } else if is_dir {
        "file_icons/folder".to_string()
    } else {
        format!("file_icons/{ext_lower}")
    }
}

fn item_select_columns() -> String {
    format!(
        "id, content_type, full_text, content_hash, created_at, updated_at, image_path, rich_data, file_data, is_favorite, note, source_app_name, CASE WHEN length(source_app_icon) <= {SOURCE_APP_ICON_INLINE_LIMIT} THEN source_app_icon ELSE '' END, image_width, image_height, size, meta_type, custom_hotkey, custom_hotkey_format, existence_observed_at"
    )
}

fn list_item_select_columns() -> String {
    format!(
        "id, content_type, substr(full_text, 1, {LIST_FULL_TEXT_LIMIT}), content_hash, created_at, updated_at, image_path,
         CASE
             WHEN rich_data = '' OR NOT json_valid(rich_data) THEN ''
             ELSE json_object(
                 'html', NULLIF(substr(coalesce(json_extract(rich_data, '$.html'), ''), 1, {LIST_RICH_HTML_LIMIT}), ''),
                 'rtf', NULLIF(substr(coalesce(json_extract(rich_data, '$.rtf'), ''), 1, {LIST_RICH_HTML_LIMIT}), ''),
                 'ocr_text', NULLIF(substr(coalesce(json_extract(rich_data, '$.ocr_text'), ''), 1, {LIST_RICH_AUX_LIMIT}), ''),
                 'qr_text', NULLIF(substr(coalesce(json_extract(rich_data, '$.qr_text'), ''), 1, {LIST_RICH_AUX_LIMIT}), ''),
                 'page_title', NULLIF(substr(coalesce(json_extract(rich_data, '$.page_title'), ''), 1, {LIST_RICH_AUX_LIMIT}), ''),
                 'drive_label', NULLIF(substr(coalesce(json_extract(rich_data, '$.drive_label'), ''), 1, {LIST_RICH_AUX_LIMIT}), ''),
                 'remote_host', NULLIF(substr(coalesce(json_extract(rich_data, '$.remote_host'), ''), 1, {LIST_RICH_AUX_LIMIT}), '')
             )
         END,
         file_data, is_favorite, substr(note, 1, {LIST_NOTE_LIMIT}), source_app_name, CASE WHEN length(source_app_icon) <= {SOURCE_APP_ICON_INLINE_LIMIT} THEN source_app_icon ELSE '' END, image_width, image_height, size, meta_type, custom_hotkey, custom_hotkey_format, existence_observed_at"
    )
}

impl Database {
    pub fn open(path: &str) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        // Wait up to 5s if the database is locked (e.g. previous process still exiting).
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // --- WAL mode improves concurrency and reduces memory pressure. ---
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA cache_size = -2000;")?;
        let db = Self { conn };
        db.init_schema()?;
        db.prune_oversized_source_icons()?;
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
            CREATE INDEX IF NOT EXISTS idx_updated ON clipboard_items(updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_created ON clipboard_items(created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_content_type ON clipboard_items(content_type);
            CREATE INDEX IF NOT EXISTS idx_is_favorite ON clipboard_items(is_favorite);",
        )?;
        // --- Tags tables ---
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tags (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                uid TEXT NOT NULL DEFAULT '',
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
        )?;
        // Tombstone tables for sync deletion propagation
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS deleted_items (
                content_hash INTEGER NOT NULL,
                deleted_at TEXT NOT NULL,
                device_name TEXT NOT NULL DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_del_items_hash ON deleted_items(content_hash);
            CREATE INDEX IF NOT EXISTS idx_del_items_at ON deleted_items(deleted_at);
            CREATE TABLE IF NOT EXISTS deleted_tags (
                uid TEXT NOT NULL DEFAULT '',
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
        )?;

        // Local-only stale-item observation state (never synced).
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS stale_item_observations (
                item_id INTEGER PRIMARY KEY REFERENCES clipboard_items(id) ON DELETE CASCADE,
                content_hash INTEGER NOT NULL,
                item_updated_at TEXT NOT NULL,
                first_missing_at TEXT NOT NULL DEFAULT '',
                last_checked_at TEXT NOT NULL DEFAULT '',
                consecutive_missing_count INTEGER NOT NULL DEFAULT 0,
                last_status TEXT NOT NULL DEFAULT '',
                last_reason TEXT NOT NULL DEFAULT ''
            );",
        )?;

        crate::core::migration::run_db_migrations(&self.conn)?;
        Ok(())
    }

    fn prune_oversized_source_icons(&self) -> SqlResult<()> {
        let changed = self.conn.execute(
            "UPDATE clipboard_items SET source_app_icon = ''
             WHERE length(source_app_icon) > ?1",
            params![SOURCE_APP_ICON_INLINE_LIMIT as i64],
        )?;
        if changed > 0 {
            log::info!("Cleared {changed} oversized source app icon(s)");
        }
        Ok(())
    }

    pub fn upsert(&self, item: &ClipboardItem) -> SqlResult<()> {
        // A local clipboard capture always ends any sync-pending state: the
        // item now exists locally with a real captured path. (Sync merge uses
        // insert_sync_item_raw / update_sync_item instead and is unaffected.)
        let changed = self.conn.execute(
            "UPDATE clipboard_items SET updated_at = ?1, content_type = ?3, image_path = ?4, rich_data = ?5, file_data = ?6, image_width = ?7, image_height = ?8, size = ?9, meta_type = ?10, existence_observed_at = CASE WHEN ?11 = '' THEN existence_observed_at ELSE ?11 END, sync_pending = 0 WHERE content_hash = ?2",
            params![item.updated_at.to_rfc3339(), item.content_hash as i64, item.content_type.as_str(), item.image_path, item.rich_data, item.file_data, item.image_width as i64, item.image_height as i64, item.size, item.meta_type, item.existence_observed_at],
        )?;
        if changed == 0 {
            self.conn.execute(
                "INSERT INTO clipboard_items (content_type, full_text, content_hash, created_at, updated_at, image_path, rich_data, file_data, source_app_name, source_app_icon, image_width, image_height, size, meta_type, existence_observed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
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
                    item.meta_type,
                    item.existence_observed_at,
                ],
            )?;
        }
        Ok(())
    }

    /// Update only the rich_data column for a given item.
    pub fn update_rich_data(&self, id: i64, rich_data: &str) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE clipboard_items SET rich_data = ?1, updated_at = ?2 WHERE id = ?3",
            params![rich_data, chrono::Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Whitelist allowed sort columns to prevent SQL injection through `order_by`.
    /// Falls back to `updated_at` for unknown values and logs a warning.
    fn validate_order_by(order_by: &str) -> &str {
        const ALLOWED: &[&str] = &[
            "created_at",
            "updated_at",
            "content_type",
            "is_favorite",
            "image_width",
            "image_height",
            "size",
        ];
        if ALLOWED.contains(&order_by) {
            order_by
        } else {
            log::warn!("[db] unexpected order_by value {order_by:?}, falling back to updated_at");
            "updated_at"
        }
    }

    /// Load items with unified filter support.
    /// Uses ClipboardFilters to build WHERE clause with AND logic across all filter dimensions.
    /// `order_by` must be one of the allowed column names — see `validate_order_by`.
    pub fn load_filtered(
        &self,
        filters: &ClipboardFilters,
        limit: usize,
        order_by: &str,
    ) -> SqlResult<Vec<ClipboardItem>> {
        self.load_filtered_inner(filters, Some(limit), order_by)
    }

    pub fn load_filtered_list(
        &self,
        filters: &ClipboardFilters,
        limit: usize,
        order_by: &str,
    ) -> SqlResult<Vec<ClipboardItem>> {
        self.load_filtered_inner_with_columns(
            filters,
            Some(limit),
            order_by,
            &list_item_select_columns(),
        )
    }

    fn load_filtered_inner(
        &self,
        filters: &ClipboardFilters,
        limit: Option<usize>,
        order_by: &str,
    ) -> SqlResult<Vec<ClipboardItem>> {
        self.load_filtered_inner_with_columns(filters, limit, order_by, &item_select_columns())
    }

    fn load_filtered_inner_with_columns(
        &self,
        filters: &ClipboardFilters,
        limit: Option<usize>,
        order_by: &str,
        columns: &str,
    ) -> SqlResult<Vec<ClipboardItem>> {
        let order_col = Self::validate_order_by(order_by);
        let (where_clause, mut filter_params) = filters.db_where();
        let limit_clause = if limit.is_some() { " LIMIT ?" } else { "" };
        let query = format!(
            "SELECT {columns}
             FROM clipboard_items {} ORDER BY {} DESC{}",
            where_clause, order_col, limit_clause
        );
        if let Some(limit) = limit {
            filter_params.push((limit as i64).into());
        }
        let mut stmt = self.conn.prepare(&query)?;
        let items = stmt.query_map(rusqlite::params_from_iter(filter_params), row_to_item)?;
        items.collect()
    }

    pub fn load_filtered_page(
        &self,
        filters: &ClipboardFilters,
        limit: usize,
        offset: usize,
        order_by: &str,
    ) -> SqlResult<Vec<ClipboardItem>> {
        let order_col = Self::validate_order_by(order_by);
        let (where_clause, mut filter_params) = filters.db_where();
        let columns = item_select_columns();
        let query = format!(
            "SELECT {columns}
             FROM clipboard_items {} ORDER BY {} DESC LIMIT ? OFFSET ?",
            where_clause, order_col
        );
        filter_params.push((limit as i64).into());
        filter_params.push((offset as i64).into());
        let mut stmt = self.conn.prepare(&query)?;
        let items = stmt.query_map(rusqlite::params_from_iter(filter_params), row_to_item)?;
        items.collect()
    }

    pub fn get_by_id(&self, id: i64) -> SqlResult<Option<ClipboardItem>> {
        let query = format!(
            "SELECT {} FROM clipboard_items WHERE id = ?1",
            item_select_columns()
        );
        let mut stmt = self.conn.prepare(&query)?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_item(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_by_hash(&self, hash: u64) -> SqlResult<Option<ClipboardItem>> {
        let query = format!(
            "SELECT {} FROM clipboard_items WHERE content_hash = ?1",
            item_select_columns()
        );
        let mut stmt = self.conn.prepare(&query)?;
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
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM stale_item_observations WHERE item_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM clipboard_items WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn update_note(&self, id: i64, note: &str) -> SqlResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE clipboard_items SET note = ?1, updated_at = ?2 WHERE id = ?3",
            params![note, now, id],
        )?;
        Ok(())
    }

    pub fn set_item_hotkey(&self, id: i64, hotkey: &str, format: &str) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE clipboard_items SET custom_hotkey = ?1, custom_hotkey_format = ?2 WHERE id = ?3",
            params![hotkey, format, id],
        )?;
        Ok(())
    }

    pub fn clear_item_hotkey(&self, id: i64) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE clipboard_items SET custom_hotkey = '', custom_hotkey_format = '' WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn load_titlebar_stats(&self) -> SqlResult<TitlebarStats> {
        self.conn.query_row(
            "SELECT
                COALESCE(MAX(custom_hotkey <> ''), 0),
                COALESCE(MAX(is_favorite = 1), 0),
                COUNT(CASE WHEN meta_type != 'transfer' THEN 1 END),
                COUNT(CASE
                    WHEN meta_type != 'transfer' AND is_favorite = 0 THEN 1
                END)
             FROM clipboard_items",
            [],
            |row| {
                Ok(TitlebarStats {
                    has_hotkey_items: row.get::<_, i64>(0)? != 0,
                    has_favorite_items: row.get::<_, i64>(1)? != 0,
                    clearable_history_count: row.get::<_, i64>(2)? as u32,
                    clearable_non_favorite_history_count: row.get::<_, i64>(3)? as u32,
                })
            },
        )
    }

    pub fn get_all_custom_item_hotkeys(&self) -> SqlResult<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, custom_hotkey FROM clipboard_items
             WHERE custom_hotkey <> ''
             ORDER BY updated_at DESC",
        )?;
        let hotkeys = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect();
        hotkeys
    }

    pub fn update_content_with_rich_data(
        &self,
        id: i64,
        text: &str,
        content_type: &str,
        meta_type: &str,
        rich_data: &str,
    ) -> SqlResult<()> {
        let hash = {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&text, &mut hasher);
            std::hash::Hasher::finish(&hasher)
        };
        let now = chrono::Utc::now().to_rfc3339();
        let size = text.chars().count() as i64;
        self.conn.execute(
            "UPDATE clipboard_items SET full_text = ?1, content_hash = ?2, content_type = ?3, updated_at = ?4, rich_data = ?5, image_path = '', file_data = '', image_width = 0, image_height = 0, size = ?6, meta_type = ?7 WHERE id = ?8",
            params![text, hash as i64, content_type, now, rich_data, size, meta_type, id],
        )?;
        Ok(())
    }

    // --- ── Tag CRUD ── ---

    pub fn create_tag(&self, name: &str, color: &str) -> SqlResult<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        let uid = new_tag_uid();
        // --- Clear any existing tombstone — the user is recreating a tag ---
        // --- that was previously deleted. ---
        let _ = self.remove_tag_tombstone(&uid, name);
        self.conn.execute(
            "INSERT INTO tags (uid, name, color, updated_at) VALUES (?1, ?2, ?3, ?4)",
            params![uid, name, color, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_tag(&mut self, tag_id: i64) -> SqlResult<()> {
        let tx = self.conn.transaction()?;
        // --- Touch affected items before removing the associations ---
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
        let mut stmt = self
            .conn
            .prepare("SELECT id, uid, name, color, updated_at FROM tags ORDER BY id DESC")?;
        let rows = stmt.query_map([], |row| {
            let updated_str: String = row.get::<_, String>(4).unwrap_or_default();
            Ok(TagInfo {
                id: row.get(0)?,
                uid: row.get(1)?,
                name: row.get(2)?,
                color: row.get(3)?,
                updated_at: updated_str,
            })
        })?;
        rows.collect()
    }

    pub fn get_tags_for_item(&self, item_id: i64) -> SqlResult<Vec<TagInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.uid, t.name, t.color, t.updated_at FROM tags t \
             INNER JOIN item_tags it ON t.id = it.tag_id \
             WHERE it.item_id = ?1 ORDER BY it.used_at DESC",
        )?;
        let rows = stmt.query_map(params![item_id], |row| {
            let updated_str: String = row.get::<_, String>(4).unwrap_or_default();
            Ok(TagInfo {
                id: row.get(0)?,
                uid: row.get(1)?,
                name: row.get(2)?,
                color: row.get(3)?,
                updated_at: updated_str,
            })
        })?;
        rows.collect()
    }

    pub fn get_tags_for_items(
        &self,
        item_ids: &[i64],
    ) -> SqlResult<std::collections::HashMap<i64, Vec<TagInfo>>> {
        let mut map: std::collections::HashMap<i64, Vec<TagInfo>> =
            std::collections::HashMap::new();
        if item_ids.is_empty() {
            return Ok(map);
        }
        let placeholders: Vec<String> = item_ids.iter().map(|_| "?".to_string()).collect();
        let query = format!(
            "SELECT it.item_id, t.id, t.uid, t.name, t.color, t.updated_at FROM tags t \
             INNER JOIN item_tags it ON t.id = it.tag_id \
             WHERE it.item_id IN ({}) ORDER BY it.used_at DESC",
            placeholders.join(",")
        );
        let mut stmt = self.conn.prepare(&query)?;
        let params: Vec<rusqlite::types::Value> = item_ids.iter().map(|&id| (id).into()).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
            let updated_str: String = row.get::<_, String>(5).unwrap_or_default();
            Ok((
                row.get::<_, i64>(0)?,
                TagInfo {
                    id: row.get(1)?,
                    uid: row.get(2)?,
                    name: row.get(3)?,
                    color: row.get(4)?,
                    updated_at: updated_str,
                },
            ))
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
        let deleted = self
            .conn
            .execute("DELETE FROM item_tags WHERE item_id = ?1", params![item_id])?;
        if deleted > 0 {
            self.touch_item(item_id)?;
        }
        Ok(())
    }

    pub fn get_tag_by_name(&self, name: &str) -> SqlResult<Option<TagInfo>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, uid, name, color, updated_at FROM tags WHERE name = ?1")?;
        let mut rows = stmt.query(params![name])?;
        if let Some(row) = rows.next()? {
            let updated_str: String = row.get::<_, String>(4).unwrap_or_default();
            Ok(Some(TagInfo {
                id: row.get(0)?,
                uid: row.get(1)?,
                name: row.get(2)?,
                color: row.get(3)?,
                updated_at: updated_str,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn get_tag_by_uid(&self, uid: &str) -> SqlResult<Option<TagInfo>> {
        if uid.is_empty() {
            return Ok(None);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id, uid, name, color, updated_at FROM tags WHERE uid = ?1")?;
        let mut rows = stmt.query(params![uid])?;
        if let Some(row) = rows.next()? {
            let updated_str: String = row.get::<_, String>(4).unwrap_or_default();
            Ok(Some(TagInfo {
                id: row.get(0)?,
                uid: row.get(1)?,
                name: row.get(2)?,
                color: row.get(3)?,
                updated_at: updated_str,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn get_tag_by_id(&self, id: i64) -> SqlResult<Option<TagInfo>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, uid, name, color, updated_at FROM tags WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            let updated_str: String = row.get::<_, String>(4).unwrap_or_default();
            Ok(Some(TagInfo {
                id: row.get(0)?,
                uid: row.get(1)?,
                name: row.get(2)?,
                color: row.get(3)?,
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
        self.fill_tags(&mut items)?;
        Ok(items)
    }

    pub fn load_filtered_page_with_tags(
        &self,
        filters: &ClipboardFilters,
        limit: usize,
        offset: usize,
        order_by: &str,
    ) -> SqlResult<Vec<ClipboardItem>> {
        let mut items = self.load_filtered_page(filters, limit, offset, order_by)?;
        self.fill_tags(&mut items)?;
        Ok(items)
    }

    pub fn load_filtered_list_with_tags(
        &self,
        filters: &ClipboardFilters,
        limit: usize,
        order_by: &str,
    ) -> SqlResult<Vec<ClipboardItem>> {
        let mut items = self.load_filtered_list(filters, limit, order_by)?;
        self.fill_tags(&mut items)?;
        Ok(items)
    }

    fn fill_tags(&self, items: &mut [ClipboardItem]) -> SqlResult<()> {
        if !items.is_empty() {
            let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
            let tag_map = self.get_tags_for_items(&ids)?;
            for item in items {
                item.tags = tag_map.get(&item.id).cloned().unwrap_or_default();
            }
        }
        Ok(())
    }

    pub fn get_by_id_with_tags(&self, id: i64) -> SqlResult<Option<ClipboardItem>> {
        if let Some(mut item) = self.get_by_id(id)? {
            item.tags = self.get_tags_for_item(id)?;
            Ok(Some(item))
        } else {
            Ok(None)
        }
    }

    // --- ── Sync helpers ── ---

    /// Get all items (excluding image and file types) with tags for sync snapshot.
    pub fn get_all_sync_items_with_tags(
        &self,
        include_images: bool,
    ) -> SqlResult<Vec<ClipboardItem>> {
        let exclude = if include_images {
            "'file'"
        } else {
            "'image', 'file'"
        };
        let query = format!(
            "SELECT {}
             FROM clipboard_items
             WHERE content_type NOT IN ({exclude})
             ORDER BY updated_at DESC",
            item_select_columns()
        );
        let mut stmt = self.conn.prepare(&query)?;
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
    /// Normalizes legacy content_type strings from old peers.
    pub fn insert_sync_item_raw(&self, item: &crate::core::sync::SyncItem) -> SqlResult<i64> {
        let created_at: chrono::DateTime<chrono::Utc> = item
            .created_at
            .parse()
            .unwrap_or_else(|_| chrono::Utc::now());
        let updated_at: chrono::DateTime<chrono::Utc> = item
            .updated_at
            .parse()
            .unwrap_or_else(|_| chrono::Utc::now());

        // Normalize legacy content_type from old peers (link/path/color → plain_text).
        let (content_type, meta_type) = match item.content_type.as_str() {
            "link" => (
                "plain_text",
                if item.meta_type.is_empty() {
                    "link"
                } else {
                    item.meta_type.as_str()
                },
            ),
            "path" => (
                "plain_text",
                if item.meta_type.is_empty() {
                    "path"
                } else {
                    item.meta_type.as_str()
                },
            ),
            "color" => (
                "plain_text",
                if item.meta_type.is_empty() {
                    "color"
                } else {
                    item.meta_type.as_str()
                },
            ),
            _ => (item.content_type.as_str(), item.meta_type.as_str()),
        };

        let is_image = content_type == "image";
        let image_path = if is_image && !item.image_blob.is_empty() {
            crate::core::paths::images_dir()
                .join(&item.image_blob)
                .to_string_lossy()
                .to_string()
        } else if is_image {
            // Fallback: use hash-based filename
            crate::core::paths::images_dir()
                .join(format!("{:016x}.png", item.content_hash))
                .to_string_lossy()
                .to_string()
        } else {
            String::new()
        };

        self.conn.execute(
            "INSERT INTO clipboard_items (content_type, full_text, content_hash, created_at, updated_at,
             rich_data, is_favorite, note, source_app_name, size, meta_type,
             image_path, image_width, image_height, file_data, sync_pending)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            rusqlite::params![
                content_type,
                item.full_text,
                item.content_hash as i64,
                created_at.to_rfc3339(),
                updated_at.to_rfc3339(),
                item.rich_data,
                item.is_favorite as i32,
                item.note,
                "", // source_app_name not in sync payload
                item.size,
                meta_type,
                image_path,
                item.image_width,
                item.image_height,
                "", // file_data — not synced yet
                is_image as i32, // sync-owned image: blob may not be downloaded yet
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Update an item's fields from a newer remote version (sync merge).
    pub fn update_sync_item(&self, id: i64, item: &crate::core::sync::SyncItem) -> SqlResult<()> {
        let is_image = item.content_type == "image";
        let image_path = if is_image && !item.image_blob.is_empty() {
            crate::core::paths::images_dir()
                .join(&item.image_blob)
                .to_string_lossy()
                .to_string()
        } else if is_image {
            crate::core::paths::images_dir()
                .join(format!("{:016x}.png", item.content_hash))
                .to_string_lossy()
                .to_string()
        } else {
            String::new()
        };

        self.conn.execute(
            "UPDATE clipboard_items SET full_text = ?1, content_type = ?2, updated_at = ?3,
             rich_data = ?4, is_favorite = ?5, note = ?6, size = ?7, meta_type = ?8,
             image_path = ?9, image_width = ?10, image_height = ?11,
             sync_pending = CASE WHEN ?13 = 'image' THEN 1 ELSE sync_pending END
             WHERE id = ?12",
            rusqlite::params![
                item.full_text,
                item.content_type,
                item.updated_at,
                item.rich_data,
                item.is_favorite as i32,
                item.note,
                item.size,
                item.meta_type,
                image_path,
                item.image_width,
                item.image_height,
                id,
                item.content_type,
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

    /// Bump updated_at for multiple items in one transaction, executing
    /// bounded `UPDATE ... WHERE id IN (...)` statements of at most 500 IDs
    /// each. Returns the number of affected rows; an empty id list is a
    /// no-op. The caller supplies the timestamp so the in-memory refresh
    /// stays consistent with the database.
    pub fn touch_items(&self, item_ids: &[i64], now: &str) -> SqlResult<usize> {
        if item_ids.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut affected = 0usize;
        for chunk in item_ids.chunks(500) {
            let placeholders: Vec<String> = chunk.iter().map(|_| "?".to_string()).collect();
            let sql = format!(
                "UPDATE clipboard_items SET updated_at = ?1 WHERE id IN ({})",
                placeholders.join(",")
            );
            let mut params: Vec<rusqlite::types::Value> =
                vec![rusqlite::types::Value::Text(now.to_string())];
            params.extend(chunk.iter().map(|&id| id.into()));
            affected += tx.execute(&sql, rusqlite::params_from_iter(params))?;
        }
        tx.commit()?;
        Ok(affected)
    }

    /// Test-only hook: reject `UPDATE clipboard_items SET updated_at` so the
    /// usage-touch failure path can be exercised. The trigger is permanent
    /// for this connection (tests use in-memory databases).
    #[cfg(test)]
    pub fn reject_updated_at_updates_for_test(&self) {
        self.conn
            .execute_batch(
                "CREATE TRIGGER reject_touch_for_test BEFORE UPDATE OF updated_at ON clipboard_items \
                 BEGIN SELECT RAISE(ABORT, 'reject'); END;",
            )
            .unwrap();
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

    // --- ── Transfer station queries ── ---

    /// Get all clipboard items with `meta_type = "transfer"`.
    pub fn get_transfer_items(&self) -> SqlResult<Vec<ClipboardItem>> {
        let query = format!(
            "SELECT {} FROM clipboard_items WHERE content_type = 'file' AND meta_type = 'transfer' ORDER BY updated_at DESC",
            item_select_columns()
        );
        let mut stmt = self.conn.prepare(&query)?;
        let items: Vec<ClipboardItem> = stmt
            .query_map([], row_to_item)?
            .collect::<SqlResult<Vec<_>>>()?;
        Ok(items)
    }

    /// Get ordinary file entries whose transfer marker is derived from the
    /// current remote manifest. Transfer backing records are excluded.
    pub fn get_original_file_items(&self) -> SqlResult<Vec<ClipboardItem>> {
        let query = format!(
            "SELECT {} FROM clipboard_items WHERE content_type = 'file' AND meta_type != 'transfer' ORDER BY updated_at DESC",
            item_select_columns()
        );
        let mut stmt = self.conn.prepare(&query)?;
        let items = stmt
            .query_map([], row_to_item)?
            .collect::<SqlResult<Vec<_>>>()?;
        Ok(items)
    }

    /// Store a transfer hash derived by comparing local file contents with the
    /// current manifest. This value is a local cache, not upload state.
    pub fn set_derived_file_transfer_hash(
        &self,
        item_id: i64,
        remote_hash: Option<&str>,
    ) -> SqlResult<bool> {
        let remote_hash = remote_hash.unwrap_or_default();
        let changed = self.conn.execute(
            "UPDATE clipboard_items
             SET file_data = json_set(file_data, '$.remote_hash', ?1)
             WHERE id = ?2
               AND content_type = 'file'
               AND meta_type != 'transfer'
               AND json_valid(file_data)
               AND COALESCE(json_extract(file_data, '$.remote_hash'), '') != ?1",
            params![remote_hash, item_id],
        )?;
        Ok(changed > 0)
    }

    /// Update the local path of an ordinary file row after its transfer blob
    /// has been downloaded again. Identity and user metadata stay on the
    /// original row; only the local file projection is refreshed.
    pub fn restore_original_transfer_file(
        &self,
        item_id: i64,
        file_data: &str,
        size: i64,
    ) -> SqlResult<bool> {
        let changed = self.conn.execute(
            "UPDATE clipboard_items
             SET file_data = ?1, size = ?2, updated_at = ?3,
                 existence_observed_at = '', sync_pending = 0
             WHERE id = ?4 AND content_type = 'file' AND meta_type != 'transfer'",
            params![file_data, size, chrono::Utc::now().to_rfc3339(), item_id],
        )?;
        Ok(changed > 0)
    }

    /// Insert or refresh the hidden local backing row for a transfer entry.
    /// This is deliberately keyed by `remote_hash`, not `content_hash`: an
    /// ordinary history row may represent the same bytes and must not be
    /// converted into a transfer backing row by the generic upsert path.
    pub fn upsert_transfer_backing(&self, item: &ClipboardItem) -> SqlResult<()> {
        let remote_hash = FileData::from_json(&item.file_data).remote_hash;
        let changed = self.conn.execute(
            "UPDATE clipboard_items
             SET updated_at = ?1, full_text = ?2, content_hash = ?3,
                 file_data = ?4, size = ?5, existence_observed_at = '',
                 sync_pending = 0
             WHERE content_type = 'file' AND meta_type = 'transfer'
               AND json_valid(file_data)
               AND json_extract(file_data, '$.remote_hash') = ?6",
            params![
                item.updated_at.to_rfc3339(),
                item.full_text,
                item.content_hash as i64,
                item.file_data,
                item.size,
                remote_hash
            ],
        )?;
        if changed == 0 {
            self.conn.execute(
                "INSERT INTO clipboard_items
                 (content_type, full_text, content_hash, created_at, updated_at,
                  image_path, rich_data, file_data, source_app_name,
                  source_app_icon, image_width, image_height, size, meta_type,
                  existence_observed_at)
                 VALUES ('file', ?1, ?2, ?3, ?4, '', '', ?5, '', '', 0, 0,
                         ?6, 'transfer', '')",
                params![
                    item.full_text,
                    item.content_hash as i64,
                    item.created_at.to_rfc3339(),
                    item.updated_at.to_rfc3339(),
                    item.file_data,
                    item.size
                ],
            )?;
        }
        Ok(())
    }

    /// Delete a transfer item by its remote_hash (stored in file_data JSON).
    /// Returns true if an item was deleted.
    pub fn delete_transfer_by_hash(&self, remote_hash: &str) -> SqlResult<bool> {
        let mut stmt = self.conn.prepare(
            "DELETE FROM clipboard_items
             WHERE content_type = 'file'
               AND meta_type = 'transfer'
               AND json_valid(file_data)
               AND json_extract(file_data, '$.remote_hash') = ?1",
        )?;
        let deleted = stmt.execute(rusqlite::params![remote_hash])?;
        Ok(deleted > 0)
    }

    /// Convert a downloaded transfer backing row into an ordinary local file row.
    pub fn detach_transfer_item(
        &self,
        item_id: i64,
        content_hash: u64,
        file_data: &str,
    ) -> SqlResult<bool> {
        let ordinary_row_exists: bool = self.conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM clipboard_items
                 WHERE id != ?1 AND content_type = 'file' AND meta_type != 'transfer'
                   AND content_hash = ?2
             )",
            params![item_id, content_hash as i64],
            |row| row.get(0),
        )?;
        if ordinary_row_exists {
            let deleted = self.conn.execute(
                "DELETE FROM clipboard_items WHERE id = ?1 AND meta_type = 'transfer'",
                params![item_id],
            )?;
            return Ok(deleted > 0);
        }
        let changed = self.conn.execute(
            "UPDATE clipboard_items
             SET content_hash = ?1, file_data = ?2, meta_type = '', updated_at = ?3
             WHERE id = ?4 AND content_type = 'file' AND meta_type = 'transfer'",
            params![
                content_hash as i64,
                file_data,
                chrono::Utc::now().to_rfc3339(),
                item_id
            ],
        )?;
        Ok(changed > 0)
    }

    // --- ── Tombstone operations ── ---

    /// Record a deleted item tombstone for sync propagation.
    pub fn record_item_deletion(
        &self,
        content_hash: u64,
        deleted_at: &str,
        device_name: &str,
    ) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO deleted_items (content_hash, deleted_at, device_name) VALUES (?1, ?2, ?3)",
            params![content_hash as i64, deleted_at, device_name],
        )?;
        Ok(())
    }

    /// Record a deleted tag tombstone for sync propagation.
    pub fn record_tag_deletion(
        &self,
        uid: &str,
        name: &str,
        deleted_at: &str,
        device_name: &str,
    ) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO deleted_tags (uid, name, deleted_at, device_name)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(uid) WHERE uid != '' DO UPDATE SET
                 name = excluded.name,
                 deleted_at = excluded.deleted_at,
                 device_name = excluded.device_name
             ON CONFLICT(name) WHERE uid = '' DO UPDATE SET
                 deleted_at = excluded.deleted_at,
                 device_name = excluded.device_name",
            params![uid, name, deleted_at, device_name],
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
    pub fn get_deleted_tags_recent(
        &self,
        days: i64,
    ) -> SqlResult<Vec<(String, String, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT uid, name, deleted_at, device_name FROM deleted_tags WHERE deleted_at >= strftime('%Y-%m-%dT%H:%M:%S', 'now', ?1)",
        )?;
        let rows = stmt.query_map(params![format!("-{days} days")], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        rows.collect()
    }

    /// Collect all image content hashes currently referenced in the database.
    pub fn get_all_image_hashes(&self) -> SqlResult<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT content_hash FROM clipboard_items
             WHERE content_type = 'image' AND image_path != ''",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        Ok(rows
            .flatten()
            .map(|hash| format!("{:016x}", hash as u64))
            .collect())
    }

    /// Update image_path for an item identified by content_hash.
    /// Used after a synced image blob has been confirmed present/downloaded:
    /// clears the sync-pending flag so the image returns to normal managed
    /// cleanup rules (design §9.5).
    pub fn set_item_image_path(&self, content_hash: u64, image_path: &str) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE clipboard_items SET image_path = ?1, sync_pending = 0 WHERE content_hash = ?2",
            rusqlite::params![image_path, content_hash as i64],
        )?;
        Ok(())
    }

    /// Test-only helper: total number of clipboard item rows.
    #[cfg(test)]
    pub(crate) fn count_all_items_for_test(&self) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM clipboard_items", [], |row| row.get(0))
            .unwrap_or(0)
    }

    /// Test-only helper: run an arbitrary SQL batch (e.g. trigger setup).
    #[cfg(test)]
    pub(crate) fn execute_batch_for_test(&self, sql: &str) -> rusqlite::Result<()> {
        self.conn.execute_batch(sql)
    }

    /// Collect icon cache filenames referenced by any clipboard item.
    ///
    /// Returns filenames (not full paths) for:
    /// - Source app icons: `{safe_name}.png` in the `icons/` directory.
    /// - File icons: per-file keys (`exe_<hash>.png`) and extension keys
    ///   (`pdf.png`) in the `file_icons/` directory; `folder.png` for dirs.
    /// - Favicons: `favicon_{domain}.png` in the `icons/` directory.
    pub fn get_all_referenced_icon_keys(&self) -> SqlResult<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_app_name, file_data, full_text, meta_type
             FROM clipboard_items
             WHERE source_app_name != ''
                OR file_data != ''
                OR (content_type = 'plain_text' AND meta_type = 'link')",
        )?;

        #[allow(clippy::type_complexity)]
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?, // source_app_name
                row.get::<_, String>(1)?, // file_data
                row.get::<_, String>(2)?, // full_text
                row.get::<_, String>(3)?, // meta_type
            ))
        })?;

        let mut keys = Vec::new();

        for row in rows.flatten() {
            let (app_name, file_data, full_text, meta_type) = row;

            // --- source app icon ---
            if let Some(key) = source_app_icon_cache_key(&app_name) {
                keys.push(key);
            }

            // --- file icon(s) ---
            if !file_data.is_empty() {
                if let Ok(fd) = serde_json::from_str::<crate::core::types::FileData>(&file_data) {
                    for fi in &fd.files {
                        keys.push(file_icon_cache_key(&fi.path, fi.is_dir));
                    }
                }
            }

            // --- favicon for link items ---
            if meta_type == "link" && !full_text.is_empty() {
                let domain = crate::core::types::url_to_domain(&full_text);
                if !domain.is_empty() {
                    // Match sanitize_domain in favicon.rs: strip port, allow
                    // only [a-zA-Z0-9._-].
                    let safe: String = domain
                        .split(':')
                        .next()
                        .unwrap_or(&domain)
                        .chars()
                        .map(|c| {
                            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                                c
                            } else {
                                '_'
                            }
                        })
                        .collect();
                    keys.push(format!("icons/favicon_{safe}"));
                }
            }
        }

        Ok(keys)
    }

    /// Internal helper: delete clipboard_items and their item_tags in chunks.
    /// Uses a transaction so the delete is atomic. Caller must ensure `ids` are valid.
    fn delete_items_in_chunks(&self, ids: &[i64]) -> SqlResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        for chunk in ids.chunks(500) {
            let placeholders: Vec<String> = chunk.iter().map(|_| "?".to_string()).collect();
            let ph = placeholders.join(",");
            let params: Vec<rusqlite::types::Value> = chunk.iter().map(|&id| (id).into()).collect();
            tx.execute(
                &format!(
                    "DELETE FROM stale_item_observations WHERE item_id IN ({})",
                    ph
                ),
                rusqlite::params_from_iter(params.iter()),
            )?;
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
        Ok(())
    }

    /// Prune oldest non-favorite items when total exceeds max_items.
    /// Returns the ids of deleted items. max_items == 0 means unlimited.
    pub fn prune_excess_non_favorites(&self, max_items: u32) -> SqlResult<Vec<i64>> {
        if max_items == 0 {
            return Ok(Vec::new());
        }
        let non_fav_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM clipboard_items WHERE is_favorite = 0 AND meta_type != 'transfer'",
            [],
            |row| row.get(0),
        )?;
        if non_fav_count <= max_items as i64 {
            return Ok(Vec::new());
        }
        let excess = (non_fav_count - max_items as i64) as usize;
        // Read only the IDs that will actually be deleted instead of loading
        // every non-favorite row and truncating in memory.
        let mut stmt = self.conn.prepare(
            "SELECT id FROM clipboard_items WHERE is_favorite = 0 AND meta_type != 'transfer' \
             ORDER BY created_at ASC LIMIT ?1",
        )?;
        let pruned_ids: Vec<i64> = stmt
            .query_map([excess as i64], |row| row.get(0))?
            .collect::<SqlResult<Vec<_>>>()?;
        if pruned_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.delete_items_in_chunks(&pruned_ids)?;
        Ok(pruned_ids)
    }

    /// Prune non-favorite items not updated within retention_days.
    /// Returns the deleted items. retention_days == 0 means no limit.
    /// Uses updated_at so frequently re-captured content stays fresh.
    #[cfg(test)]
    pub fn prune_expired_items(&self, retention_days: u32) -> SqlResult<Vec<PrunedClipboardItem>> {
        self.prune_expired_items_with_sync_scope(retention_days, None)
            .map(|(items, _)| items)
    }

    /// Prune expired items and write any required sync tombstones in the same
    /// transaction as the item/tag deletions.
    pub fn prune_expired_items_with_sync_scope(
        &self,
        retention_days: u32,
        sync_scope: Option<&CleanupSyncScope>,
    ) -> SqlResult<(Vec<PrunedClipboardItem>, u32)> {
        if retention_days == 0 {
            return Ok((Vec::new(), 0));
        }
        let cutoff = format!("-{} days", retention_days);
        let tx = self.conn.unchecked_transaction()?;
        let mut stmt = tx.prepare(
            "SELECT id, content_hash, content_type, is_favorite, custom_hotkey, file_data FROM clipboard_items \
             WHERE is_favorite = 0 AND meta_type != 'transfer' \
               AND julianday(updated_at) < julianday('now', ?1)",
        )?;
        let expired_items: Vec<PrunedClipboardItem> = stmt
            .query_map(params![&cutoff], |row| {
                let content_type: String = row.get(2)?;
                let is_favorite: i32 = row.get(3)?;
                Ok(PrunedClipboardItem {
                    id: row.get(0)?,
                    content_hash: row.get::<_, i64>(1)? as u64,
                    content_type: ContentType::from_str(&content_type),
                    is_favorite: is_favorite != 0,
                    custom_hotkey: row.get(4).unwrap_or_default(),
                    file_data: row.get(5).unwrap_or_default(),
                })
            })?
            .collect::<SqlResult<Vec<_>>>()?;
        drop(stmt);

        let mut tombstones_written = 0;
        if let Some(scope) = sync_scope {
            let now = chrono::Utc::now().to_rfc3339();
            for item in &expired_items {
                if crate::core::sync_scope::item_in_sync_scope(
                    item.content_type,
                    item.is_favorite,
                    scope.include_images,
                    scope.favorites_only,
                ) {
                    tx.execute(
                        "INSERT INTO deleted_items (content_hash, deleted_at, device_name) \
                         VALUES (?1, ?2, ?3) \
                         ON CONFLICT(content_hash) DO UPDATE SET \
                           deleted_at = excluded.deleted_at, \
                           device_name = excluded.device_name",
                        params![item.content_hash as i64, &now, &scope.device_name],
                    )?;
                    tombstones_written += 1;
                }
            }
        }

        let expired_ids: Vec<i64> = expired_items.iter().map(|item| item.id).collect();
        for chunk in expired_ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            tx.execute(
                &format!("DELETE FROM stale_item_observations WHERE item_id IN ({placeholders})"),
                rusqlite::params_from_iter(chunk.iter()),
            )?;
            tx.execute(
                &format!("DELETE FROM item_tags WHERE item_id IN ({placeholders})"),
                rusqlite::params_from_iter(chunk.iter()),
            )?;
            tx.execute(
                &format!("DELETE FROM clipboard_items WHERE id IN ({placeholders})"),
                rusqlite::params_from_iter(chunk.iter()),
            )?;
        }
        tx.commit()?;
        Ok((expired_items, tombstones_written))
    }

    /// Clean up tombstones older than N days.
    pub fn cleanup_old_tombstones(&self, days: i64) -> SqlResult<u32> {
        let cutoff = format!("-{days} days");
        let deleted_items = self.conn.execute(
            "DELETE FROM deleted_items WHERE deleted_at < strftime('%Y-%m-%dT%H:%M:%S', 'now', ?1)",
            params![&cutoff],
        )?;
        let deleted_tags = self.conn.execute(
            "DELETE FROM deleted_tags WHERE deleted_at < strftime('%Y-%m-%dT%H:%M:%S', 'now', ?1)",
            params![&cutoff],
        )?;
        let unfavorited_items = self.conn.execute(
            "DELETE FROM unfavorited_items WHERE unfavorited_at < strftime('%Y-%m-%dT%H:%M:%S', 'now', ?1)",
            params![&cutoff],
        )?;
        Ok((deleted_items + deleted_tags + unfavorited_items) as u32)
    }

    /// Remove stale sync markers that have been superseded by newer local data.
    pub fn cleanup_sync_residue(&self) -> SqlResult<u32> {
        let mut removed = 0;

        let item_tombstones: Vec<(u64, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT content_hash, deleted_at FROM deleted_items")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get::<_, String>(1)?))
            })?;
            rows.collect::<SqlResult<Vec<_>>>()?
        };
        for (hash, deleted_at) in item_tombstones {
            if let Some(item) = self.get_by_hash(hash)? {
                if rfc3339_newer_str(&item.updated_at.to_rfc3339(), &deleted_at) {
                    removed += self.conn.execute(
                        "DELETE FROM deleted_items WHERE content_hash = ?1",
                        params![hash as i64],
                    )?;
                }
            }
        }

        let unfavorite_markers: Vec<(u64, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT content_hash, unfavorited_at FROM unfavorited_items")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get::<_, String>(1)?))
            })?;
            rows.collect::<SqlResult<Vec<_>>>()?
        };
        for (hash, unfavorited_at) in unfavorite_markers {
            let newer_item_refavorite = self.get_by_hash(hash)?.is_some_and(|item| {
                item.is_favorite
                    && rfc3339_newer_str(&item.updated_at.to_rfc3339(), &unfavorited_at)
            });
            let newer_item_delete = self
                .get_item_tombstone_deleted_at(hash)?
                .is_some_and(|deleted_at| rfc3339_newer_or_equal_str(&deleted_at, &unfavorited_at));

            if newer_item_refavorite || newer_item_delete {
                removed += self.conn.execute(
                    "DELETE FROM unfavorited_items WHERE content_hash = ?1",
                    params![hash as i64],
                )?;
            }
        }

        let tag_tombstones: Vec<(String, String, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT uid, name, deleted_at FROM deleted_tags")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<SqlResult<Vec<_>>>()?
        };
        for (uid, name, deleted_at) in tag_tombstones {
            let tag = if uid.is_empty() {
                self.get_tag_by_name(&name)?
            } else {
                self.get_tag_by_uid(&uid)?
            };
            if let Some(tag) = tag {
                if rfc3339_newer_str(&tag.updated_at, &deleted_at) {
                    removed += self.remove_tag_tombstone(&uid, &name)?;
                }
            }
        }

        Ok(removed as u32)
    }

    /// Record an unfavorite marker for sync propagation.
    pub fn record_unfavorite(
        &self,
        content_hash: u64,
        unfavorited_at: &str,
        device_name: &str,
    ) -> SqlResult<()> {
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

    /// Get the most recent unfavorited_at timestamp of an unfavorite marker.
    pub fn get_unfavorite_deleted_at(&self, content_hash: u64) -> SqlResult<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT unfavorited_at FROM unfavorited_items WHERE content_hash = ?1 ORDER BY unfavorited_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![content_hash as i64])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Check if a tag has a local tombstone.
    pub fn is_tag_tombstoned(&self, uid: &str, name: &str) -> SqlResult<bool> {
        let count: i64 = if uid.is_empty() {
            self.conn.query_row(
                "SELECT COUNT(*) FROM deleted_tags WHERE uid = '' AND name = ?1",
                params![name],
                |row| row.get(0),
            )?
        } else {
            self.conn.query_row(
                "SELECT COUNT(*) FROM deleted_tags WHERE uid = ?1",
                params![uid],
                |row| row.get(0),
            )?
        };
        Ok(count > 0)
    }

    /// Get the most recent deleted_at timestamp of a tag tombstone.
    /// Used during merge to compare with remote tag's updated_at.
    pub fn get_tag_tombstone_deleted_at(&self, uid: &str, name: &str) -> SqlResult<Option<String>> {
        let sql = if uid.is_empty() {
            "SELECT deleted_at FROM deleted_tags WHERE uid = '' AND name = ?1 ORDER BY deleted_at DESC LIMIT 1"
        } else {
            "SELECT deleted_at FROM deleted_tags WHERE uid = ?1 ORDER BY deleted_at DESC LIMIT 1"
        };
        let key = if uid.is_empty() { name } else { uid };
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query(params![key])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Check if a tag has a tombstone from a device OTHER than the given one.
    /// Used during merge: if the tombstone is from the same device that sent
    /// the payload, the sender recreated the tag and we should accept it.
    pub fn is_tag_tombstoned_by_other_device(
        &self,
        uid: &str,
        name: &str,
        except_device: &str,
    ) -> SqlResult<bool> {
        let count: i64 = if uid.is_empty() {
            self.conn.query_row(
                "SELECT COUNT(*) FROM deleted_tags WHERE uid = '' AND name = ?1 AND device_name != ?2",
                params![name, except_device],
                |row| row.get(0),
            )?
        } else {
            self.conn.query_row(
                "SELECT COUNT(*) FROM deleted_tags WHERE uid = ?1 AND device_name != ?2",
                params![uid, except_device],
                |row| row.get(0),
            )?
        };
        Ok(count > 0)
    }

    /// Get the most recent deleted_at timestamp of an item tombstone.
    /// Used during merge to compare with remote item's updated_at.
    pub fn get_item_tombstone_deleted_at(&self, content_hash: u64) -> SqlResult<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT deleted_at FROM deleted_items WHERE content_hash = ?1 ORDER BY deleted_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![content_hash as i64])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Remove an item tombstone (item was recreated/updated after deletion).
    pub fn remove_item_tombstone(&self, content_hash: u64) -> SqlResult<()> {
        self.conn.execute(
            "DELETE FROM deleted_items WHERE content_hash = ?1",
            params![content_hash as i64],
        )?;
        Ok(())
    }

    /// Remove a tag tombstone (tag was recreated).
    pub fn remove_tag_tombstone(&self, uid: &str, name: &str) -> SqlResult<usize> {
        if uid.is_empty() {
            self.conn.execute(
                "DELETE FROM deleted_tags WHERE uid = '' AND name = ?1",
                params![name],
            )
        } else {
            self.conn
                .execute("DELETE FROM deleted_tags WHERE uid = ?1", params![uid])
        }
    }

    /// Delete a local item by content_hash (triggered by remote tombstone).
    pub fn delete_item_by_hash(&self, content_hash: u64) -> SqlResult<bool> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM stale_item_observations WHERE item_id IN \
             (SELECT id FROM clipboard_items WHERE content_hash = ?1)",
            params![content_hash as i64],
        )?;
        let affected = tx.execute(
            "DELETE FROM clipboard_items WHERE content_hash = ?1",
            params![content_hash as i64],
        )?;
        tx.commit()?;
        Ok(affected > 0)
    }

    /// Delete a local tag by name (triggered by remote tombstone).
    /// Also cleans up item_tags associations and touches affected items.
    pub fn delete_tag_by_name(&mut self, name: &str) -> SqlResult<bool> {
        let tx = self.conn.transaction()?;
        // --- Touch affected items ---
        tx.execute(
            "UPDATE clipboard_items SET updated_at = ?1
             WHERE id IN (SELECT it.item_id FROM item_tags it
                          INNER JOIN tags t ON t.id = it.tag_id
                          WHERE t.name = ?2)",
            rusqlite::params![chrono::Utc::now().to_rfc3339(), name],
        )?;
        // --- Delete item_tags associations ---
        tx.execute(
            "DELETE FROM item_tags WHERE tag_id IN (SELECT id FROM tags WHERE name = ?1)",
            params![name],
        )?;
        // --- Delete the tag ---
        let affected = tx.execute("DELETE FROM tags WHERE name = ?1", params![name])?;
        tx.commit()?;
        Ok(affected > 0)
    }

    /// Delete a local tag by sync uid (triggered by remote tombstone).
    /// Also cleans up item_tags associations and touches affected items.
    pub fn delete_tag_by_uid(&mut self, uid: &str) -> SqlResult<bool> {
        if uid.is_empty() {
            return Ok(false);
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE clipboard_items SET updated_at = ?1
             WHERE id IN (SELECT it.item_id FROM item_tags it
                          INNER JOIN tags t ON t.id = it.tag_id
                          WHERE t.uid = ?2)",
            rusqlite::params![chrono::Utc::now().to_rfc3339(), uid],
        )?;
        tx.execute(
            "DELETE FROM item_tags WHERE tag_id IN (SELECT id FROM tags WHERE uid = ?1)",
            params![uid],
        )?;
        let affected = tx.execute("DELETE FROM tags WHERE uid = ?1", params![uid])?;
        tx.commit()?;
        Ok(affected > 0)
    }

    /// Update tag with explicit timestamp (for sync merge).
    pub fn update_tag_with_timestamp(
        &self,
        id: i64,
        name: &str,
        color: &str,
        updated_at: &str,
    ) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE tags SET name = ?1, color = ?2, updated_at = ?3 WHERE id = ?4",
            params![name, color, updated_at, id],
        )?;
        Ok(())
    }

    /// Update tag uid and fields with explicit timestamp (for name fallback during sync merge).
    pub fn update_tag_uid_with_timestamp(
        &self,
        id: i64,
        uid: &str,
        name: &str,
        color: &str,
        updated_at: &str,
    ) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE tags SET uid = ?1, name = ?2, color = ?3, updated_at = ?4 WHERE id = ?5",
            params![uid, name, color, updated_at, id],
        )?;
        Ok(())
    }

    /// Create tag with explicit timestamp (for sync merge).
    /// Caller must ensure the tag does not already exist.
    pub fn create_tag_with_timestamp(
        &self,
        name: &str,
        color: &str,
        updated_at: &str,
    ) -> SqlResult<i64> {
        let uid = legacy_tag_uid(name);
        // --- Clear any existing tombstone — a remote device recreated this tag. ---
        let _ = self
            .conn
            .execute("DELETE FROM deleted_tags WHERE name = ?1", params![name]);
        self.conn.execute(
            "INSERT INTO tags (uid, name, color, updated_at) VALUES (?1, ?2, ?3, ?4)",
            params![uid, name, color, updated_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Create tag with explicit sync uid and timestamp (for sync merge).
    pub fn create_tag_with_uid_and_timestamp(
        &self,
        uid: &str,
        name: &str,
        color: &str,
        updated_at: &str,
    ) -> SqlResult<i64> {
        let uid = if uid.is_empty() {
            legacy_tag_uid(name)
        } else {
            uid.to_string()
        };
        let _ = self.remove_tag_tombstone(&uid, name);
        self.conn.execute(
            "INSERT INTO tags (uid, name, color, updated_at) VALUES (?1, ?2, ?3, ?4)",
            params![uid, name, color, updated_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Merge data from an external database into this one.
    ///
    /// Uses `ATTACH DATABASE` to merge the source database in-process:
    /// - Items are deduplicated by `content_hash` (last-writer-wins on `updated_at`).
    /// - Tags are deduplicated by `name` (last-writer-wins on `updated_at`).
    /// - Tag associations (`item_tags`) are resolved across databases by joining
    ///   on `content_hash` (items) and `name` (tags) to handle differing
    ///   auto-increment IDs.
    /// - Tombstone tables are merged with `INSERT OR IGNORE` (duplicates skipped
    ///   via UNIQUE indexes).
    ///
    /// The source database is never modified. Both databases must have been
    /// through `init_schema()` so that their schemas match (same columns).
    pub fn merge_from(&self, source_path: &Path) -> SqlResult<MergeStats> {
        // Ensure the current WAL is flushed before attaching another DB.
        self.checkpoint()?;

        let source_str = source_path.to_string_lossy();
        self.conn
            .execute("ATTACH DATABASE ?1 AS source", params![source_str.as_ref()])?;

        // Rollback on error: best-effort DETACH.
        let result = self.merge_from_attached();
        let _ = self.conn.execute("DETACH DATABASE source", params![]);
        self.checkpoint()?;
        result
    }

    /// Inner merge logic — assumes `source` is already attached.
    fn merge_from_attached(&self) -> SqlResult<MergeStats> {
        // ── 1. Items: insert rows whose content_hash doesn't exist locally ──
        // Stale-cleanup protection fields (`existence_observed_at`,
        // `sync_pending`) are copied along: dropping them would either
        // permanently lose the capture-time existence evidence or turn a
        // not-yet-downloaded sync image into a deletable local image.
        let items_added = self.conn.execute(
            "INSERT INTO main.clipboard_items
             (content_type, full_text, content_hash, created_at, updated_at,
              image_path, rich_data, file_data, is_favorite, note,
              source_app_name, source_app_icon, image_width, image_height, size, meta_type,
              existence_observed_at, sync_pending)
             SELECT content_type, full_text, content_hash, created_at, updated_at,
                    image_path, rich_data, file_data, is_favorite, note,
                    source_app_name, source_app_icon, image_width, image_height, size,
                    COALESCE(meta_type, ''), existence_observed_at, sync_pending
             FROM source.clipboard_items s
             WHERE s.content_hash NOT IN (
                 SELECT content_hash FROM main.clipboard_items
             )",
            params![],
        )?;

        // ── 2. Items: update existing rows when source has a newer updated_at ──
        // The winning (newer) source record also wins on `sync_pending`, but
        // `existence_observed_at` follows the same non-empty-evidence rule as
        // `upsert`: an empty source value must not erase target evidence.
        let items_updated = self.conn.execute(
            "UPDATE main.clipboard_items
             SET content_type   = (SELECT s.content_type    FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash),
                 full_text      = (SELECT s.full_text       FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash),
                 updated_at     = (SELECT s.updated_at      FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash),
                 image_path     = (SELECT s.image_path      FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash),
                 rich_data      = (SELECT s.rich_data       FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash),
                 file_data      = (SELECT s.file_data       FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash),
                 is_favorite    = (SELECT s.is_favorite     FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash),
                 note           = (SELECT s.note            FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash),
                 source_app_name= (SELECT s.source_app_name FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash),
                 source_app_icon= (SELECT s.source_app_icon FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash),
                 image_width    = (SELECT s.image_width     FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash),
                 image_height   = (SELECT s.image_height    FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash),
                 size           = (SELECT s.size            FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash),
                 meta_type      = (SELECT COALESCE(s.meta_type, '') FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash),
                 existence_observed_at = (SELECT CASE WHEN s.existence_observed_at = '' THEN main.clipboard_items.existence_observed_at ELSE s.existence_observed_at END FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash),
                 sync_pending   = (SELECT s.sync_pending    FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash)
             WHERE EXISTS (
                 SELECT 1 FROM source.clipboard_items s
                 WHERE s.content_hash = main.clipboard_items.content_hash
                   AND s.updated_at > main.clipboard_items.updated_at
             )",
            params![],
        )?;

        // ── 3. Tags: insert sync uids that don't exist locally ──
        let tags_added = self.conn.execute(
            "INSERT INTO main.tags (uid, name, color, updated_at)
             SELECT uid, name, color, updated_at FROM source.tags
             WHERE uid NOT IN (SELECT uid FROM main.tags)
               AND name NOT IN (SELECT name FROM main.tags)",
            params![],
        )?;

        // ── 4. Tags: update existing rows when source has a newer updated_at ──
        let tags_updated = self.conn.execute(
            "UPDATE main.tags
             SET name       = (SELECT s.name       FROM source.tags s WHERE s.uid = main.tags.uid),
                 color      = (SELECT s.color      FROM source.tags s WHERE s.uid = main.tags.uid),
                 updated_at = (SELECT s.updated_at FROM source.tags s WHERE s.uid = main.tags.uid)
             WHERE EXISTS (
                 SELECT 1 FROM source.tags s
                 WHERE s.uid = main.tags.uid
                   AND s.updated_at > main.tags.updated_at
              )",
            params![],
        )?;

        // ── 5. Item-tag associations: resolve IDs across databases ──
        // source.item_tags → source.items (by id → content_hash) → main.items (by content_hash → id)
        // source.item_tags → source.tags  (by id → uid)          → main.tags  (by uid → id)
        self.conn.execute(
            "INSERT OR IGNORE INTO main.item_tags (tag_id, item_id, used_at)
             SELECT mt.id, mi.id, sit.used_at
             FROM source.item_tags sit
             JOIN source.tags            st  ON st.id  = sit.tag_id
             JOIN source.clipboard_items sci ON sci.id = sit.item_id
             JOIN main.clipboard_items   mi  ON mi.content_hash = sci.content_hash
             JOIN main.tags              mt  ON mt.uid          = st.uid",
            params![],
        )?;

        // ── 6. Tombstones: INSERT OR IGNORE (UNIQUE indexes prevent duplicates) ──
        self.conn.execute(
            "INSERT OR IGNORE INTO main.deleted_items (content_hash, deleted_at, device_name)
             SELECT content_hash, deleted_at, device_name FROM source.deleted_items",
            params![],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO main.deleted_tags (uid, name, deleted_at, device_name)
             SELECT uid, name, deleted_at, device_name FROM source.deleted_tags",
            params![],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO main.unfavorited_items (content_hash, unfavorited_at, device_name)
             SELECT content_hash, unfavorited_at, device_name FROM source.unfavorited_items",
            params![],
        )?;

        Ok(MergeStats {
            items_added,
            items_updated,
            tags_added,
            tags_updated,
        })
    }

    /// Find clipboard items that are candidates for stale-item cleanup.
    /// Returns non-favorite, non-transfer file, native-path, and locally
    /// captured image items, together with their current observation state.
    ///
    /// Rows whose `updated_at` cannot be parsed are counted and skipped
    /// instead of failing the whole batch, so one corrupt row cannot disable
    /// stale cleanup for every other item (see design §5.11).
    pub fn find_stale_item_candidates(&self) -> SqlResult<(Vec<StaleItemCandidate>, u32)> {
        let mut stmt = self.conn.prepare(
            "SELECT ci.id, ci.content_hash, ci.updated_at, ci.content_type, ci.full_text, \
                    ci.image_path, ci.file_data, ci.meta_type, ci.is_favorite, \
                    ci.existence_observed_at, ci.sync_pending, \
                    obs.content_hash, obs.item_updated_at, obs.first_missing_at, \
                    obs.last_checked_at, obs.consecutive_missing_count, obs.last_status, \
                    obs.last_reason \
             FROM clipboard_items ci \
             LEFT JOIN stale_item_observations obs ON obs.item_id = ci.id \
             WHERE ci.is_favorite = 0 \
               AND ci.meta_type != 'transfer' \
               AND (ci.content_type = 'file' OR ci.content_type = 'image' OR ci.meta_type = 'path') \
             ORDER BY ci.id",
        )?;

        let mut skipped: u32 = 0;
        let candidates: Vec<StaleItemCandidate> = stmt
            .query_map([], |row| {
                let content_type_str: String = row.get(3)?;
                let is_favorite_int: i32 = row.get(8)?;
                let observation = match row.get::<_, Option<String>>(13)? {
                    Some(first_missing_raw) => {
                        // A corrupt timestamp or an out-of-range missing count
                        // must not be treated as "long ago / huge count" (that
                        // would bypass the grace period). Treat the whole
                        // observation as absent so the counter restarts.
                        let first_missing_at = first_missing_raw
                            .parse::<chrono::DateTime<chrono::Utc>>()
                            .ok();
                        let item_updated_at = row
                            .get::<_, Option<String>>(12)?
                            .unwrap_or_default()
                            .parse::<chrono::DateTime<chrono::Utc>>()
                            .ok();
                        let count_raw = row.get::<_, Option<i64>>(15)?.unwrap_or(0);
                        let count_ok = (0..=STALE_OBSERVATION_COUNT_MAX).contains(&count_raw);
                        match (first_missing_at, item_updated_at, count_ok) {
                            (Some(first_missing_at), Some(item_updated_at), true) => {
                                Some(crate::core::cache_cleanup::StaleObservation {
                                    item_id: row.get(0)?,
                                    content_hash: row.get::<_, Option<i64>>(11)?.unwrap_or(0)
                                        as u64,
                                    item_updated_at,
                                    first_missing_at,
                                    last_checked_at: row
                                        .get::<_, Option<String>>(14)?
                                        .unwrap_or_default()
                                        .parse::<chrono::DateTime<chrono::Utc>>()
                                        .unwrap_or(chrono::DateTime::UNIX_EPOCH),
                                    consecutive_missing_count: count_raw as u32,
                                    last_status: row.get(16).unwrap_or_default(),
                                    last_reason: row.get(17).unwrap_or_default(),
                                })
                            }
                            _ => None,
                        }
                    }
                    None => None,
                };
                Ok(StaleItemCandidate {
                    id: row.get(0)?,
                    content_hash: row.get::<_, i64>(1)? as u64,
                    updated_at: {
                        let s: String = row.get(2)?;
                        s.parse::<chrono::DateTime<chrono::Utc>>()
                            .map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    2,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?
                    },
                    content_type: crate::core::types::ContentType::from_str(&content_type_str),
                    full_text: row.get(4).unwrap_or_default(),
                    image_path: row.get(5).unwrap_or_default(),
                    file_data: row.get(6).unwrap_or_default(),
                    meta_type: row.get(7).unwrap_or_default(),
                    is_favorite: is_favorite_int != 0,
                    existence_observed_at: row.get(9).unwrap_or_default(),
                    sync_pending: row.get::<_, Option<i64>>(10)?.unwrap_or(0) != 0,
                    observation,
                })
            })?
            .filter_map(|result| match result {
                Ok(candidate) => Some(candidate),
                Err(error) => {
                    log::warn!("find_stale_item_candidates: skipping row: {error}");
                    skipped += 1;
                    None
                }
            })
            .collect();

        Ok((candidates, skipped))
    }

    /// Insert or replace the persisted stale-item observation for an item.
    pub fn save_stale_observation(
        &self,
        observation: &crate::core::cache_cleanup::StaleObservation,
    ) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO stale_item_observations \
             (item_id, content_hash, item_updated_at, first_missing_at, last_checked_at, \
              consecutive_missing_count, last_status, last_reason) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                observation.item_id,
                observation.content_hash as i64,
                observation.item_updated_at.to_rfc3339(),
                observation.first_missing_at.to_rfc3339(),
                observation.last_checked_at.to_rfc3339(),
                observation.consecutive_missing_count as i64,
                observation.last_status,
                observation.last_reason,
            ],
        )?;
        Ok(())
    }

    /// Remove the persisted stale-item observation for an item (present
    /// again, identity changed, or item deleted).
    pub fn clear_stale_observation(&self, item_id: i64) -> SqlResult<()> {
        self.conn.execute(
            "DELETE FROM stale_item_observations WHERE item_id = ?1",
            params![item_id],
        )?;
        Ok(())
    }

    /// Delete confirmed-stale items in a transaction with identity re-check.
    ///
    /// Each candidate is re-checked within the transaction using
    /// `id + content_hash + updated_at` to ensure the record hasn't changed
    /// since the filesystem scan. Sync tombstones are written for items
    /// within the sync scope.
    pub fn delete_stale_items(
        &self,
        confirmed: &[ConfirmedStaleItem],
        sync_scope: Option<&CleanupSyncScope>,
    ) -> SqlResult<DeleteItemsResult> {
        use crate::core::types::ContentType;

        type StaleDeleteRow = (
            String,
            String,
            String,
            String,
            String,
            String,
            i32,
            String,
            String,
            bool,
        );

        if confirmed.is_empty() {
            return Ok(DeleteItemsResult::default());
        }

        let now = chrono::Utc::now().to_rfc3339();
        let device_name = sync_scope.map(|s| s.device_name.as_str()).unwrap_or("");
        let mut deleted: u32 = 0;
        let mut tombstones_written: u32 = 0;
        let mut hotkey_ids: Vec<i64> = Vec::new();
        let mut deleted_file_paths: Vec<String> = Vec::new();

        // Process in batches of 100.
        for chunk in confirmed.chunks(100) {
            let tx = self.conn.unchecked_transaction()?;

            for item in chunk {
                // Re-check identity + safety guards and read fields in one query.
                let mut stmt = tx.prepare(
                    "SELECT updated_at, content_type, full_text, image_path, file_data, meta_type, \
                            is_favorite, custom_hotkey, existence_observed_at, sync_pending \
                     FROM clipboard_items \
                     WHERE id = ?1 AND content_hash = ?2 \
                       AND is_favorite = 0 AND meta_type != 'transfer'",
                )?;

                let row: Option<StaleDeleteRow> = stmt
                    .query_row(params![item.id, item.content_hash as i64], |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2).unwrap_or_default(),
                            row.get(3).unwrap_or_default(),
                            row.get(4).unwrap_or_default(),
                            row.get(5).unwrap_or_default(),
                            row.get(6).unwrap_or_default(),
                            row.get(7).unwrap_or_default(),
                            row.get(8).unwrap_or_default(),
                            row.get::<_, Option<i64>>(9)?.unwrap_or(0) != 0,
                        ))
                    })
                    .optional()?;

                let Some((
                    updated_at_str,
                    content_type_str,
                    full_text,
                    image_path,
                    file_data,
                    meta_type,
                    is_favorite,
                    custom_hotkey,
                    existence_observed_at,
                    sync_pending,
                )) = row
                else {
                    continue; // Record changed — skip this item.
                };
                drop(stmt);

                let Ok(updated_at) = updated_at_str.parse::<chrono::DateTime<chrono::Utc>>() else {
                    continue;
                };
                if updated_at != item.expected_updated_at {
                    continue;
                }

                let content_type = ContentType::from_str(&content_type_str);
                let candidate = StaleItemCandidate {
                    id: item.id,
                    content_hash: item.content_hash,
                    updated_at,
                    content_type,
                    full_text,
                    image_path,
                    file_data: file_data.clone(),
                    meta_type,
                    is_favorite: is_favorite != 0,
                    existence_observed_at,
                    sync_pending,
                    observation: None,
                };
                // The filesystem re-check must still classify the item as
                // definitely missing before the delete is committed.
                if !matches!(
                    crate::core::cache_cleanup::classify_item_status(&candidate),
                    crate::core::cache_cleanup::ItemStatus::DefinitelyMissing
                ) {
                    continue;
                }

                if !custom_hotkey.is_empty() {
                    hotkey_ids.push(item.id);
                }

                // Write sync tombstone if applicable.
                if let Some(scope) = sync_scope {
                    if crate::core::sync_scope::item_in_sync_scope(
                        content_type,
                        false, // is_favorite is always 0 here
                        scope.include_images,
                        scope.favorites_only,
                    ) {
                        tx.execute(
                            "INSERT INTO deleted_items (content_hash, deleted_at, device_name) \
                             VALUES (?1, ?2, ?3) \
                             ON CONFLICT(content_hash) DO UPDATE SET \
                               deleted_at = excluded.deleted_at, \
                               device_name = excluded.device_name",
                            params![item.content_hash as i64, &now, device_name],
                        )?;
                        tombstones_written += 1;
                    }
                }

                if content_type == ContentType::File {
                    deleted_file_paths.extend(
                        FileData::from_json(&file_data)
                            .files
                            .into_iter()
                            .map(|file| file.path),
                    );
                }

                // Delete observation, item_tags and the item itself.
                tx.execute(
                    "DELETE FROM stale_item_observations WHERE item_id = ?1",
                    params![item.id],
                )?;
                tx.execute("DELETE FROM item_tags WHERE item_id = ?1", params![item.id])?;
                tx.execute(
                    "DELETE FROM clipboard_items WHERE id = ?1",
                    params![item.id],
                )?;
                deleted += 1;
            }

            tx.commit()?;
        }

        Ok(DeleteItemsResult {
            deleted_items: deleted,
            deleted_hotkey_item_ids: hotkey_ids,
            deleted_file_paths,
            tombstones_written,
        })
    }

    /// Clear all non-transfer clipboard history in a single transaction.
    ///
    /// Writes deletion tombstones for all protocol-syncable content types
    /// (text, rich_text, image) to prevent history from flowing back from
    /// other devices. File items are excluded from tombstones but still
    /// deleted locally.
    pub fn clear_clipboard_history(
        &self,
        device_name: &str,
        include_favorites: bool,
    ) -> SqlResult<ClearClipboardResult> {
        let now = chrono::Utc::now().to_rfc3339();

        let tx = self.conn.unchecked_transaction()?;

        // Read all non-transfer items before deletion.
        let mut stmt = tx.prepare(
            "SELECT id, content_hash, content_type, is_favorite, custom_hotkey, file_data \
             FROM clipboard_items \
             WHERE meta_type != 'transfer' AND (?1 OR is_favorite = 0)",
        )?;

        struct ItemRow {
            id: i64,
            content_hash: i64,
            content_type: String,
            is_favorite: bool,
            custom_hotkey: String,
            file_data: String,
        }

        let rows: Vec<ItemRow> = stmt
            .query_map(params![include_favorites], |row| {
                let is_fav: i32 = row.get(3)?;
                Ok(ItemRow {
                    id: row.get(0)?,
                    content_hash: row.get(1)?,
                    content_type: row.get(2)?,
                    is_favorite: is_fav != 0,
                    custom_hotkey: row.get(4).unwrap_or_default(),
                    file_data: row.get(5).unwrap_or_default(),
                })
            })?
            .collect::<SqlResult<Vec<_>>>()?;
        drop(stmt);

        if rows.is_empty() {
            tx.commit()?;
            return Ok(ClearClipboardResult::default());
        }

        let mut deleted_items: u32 = 0;
        let mut deleted_favorites: u32 = 0;
        let mut tombstones_written: u32 = 0;
        let mut hotkey_ids: Vec<i64> = Vec::new();
        let mut deleted_file_paths: Vec<String> = Vec::new();

        // Write tombstones for all protocol-syncable types.
        let syncable_types = ["plain_text", "rich_text", "image"];
        for row in &rows {
            if syncable_types.contains(&row.content_type.as_str()) {
                tx.execute(
                    "INSERT INTO deleted_items (content_hash, deleted_at, device_name) \
                     VALUES (?1, ?2, ?3) \
                     ON CONFLICT(content_hash) DO UPDATE SET \
                       deleted_at = excluded.deleted_at, \
                       device_name = excluded.device_name",
                    params![row.content_hash, &now, device_name],
                )?;
                tombstones_written += 1;
            }

            if row.content_type == ContentType::File.as_str() {
                deleted_file_paths.extend(
                    FileData::from_json(&row.file_data)
                        .files
                        .into_iter()
                        .map(|file| file.path),
                );
            }

            if row.is_favorite {
                deleted_favorites += 1;
            }

            if !row.custom_hotkey.is_empty() {
                hotkey_ids.push(row.id);
            }

            deleted_items += 1;
        }

        // Delete all item_tags and stale observations for these items.
        let item_ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
        for chunk in item_ids.chunks(100) {
            let placeholders: Vec<String> = chunk
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 1))
                .collect();
            let sql = format!(
                "DELETE FROM item_tags WHERE item_id IN ({})",
                placeholders.join(",")
            );
            let params_refs: Vec<&dyn rusqlite::types::ToSql> = chunk
                .iter()
                .map(|id| id as &dyn rusqlite::types::ToSql)
                .collect();
            tx.execute(&sql, params_refs.as_slice())?;
        }
        for chunk in item_ids.chunks(100) {
            let placeholders: Vec<String> = chunk
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 1))
                .collect();
            let sql = format!(
                "DELETE FROM stale_item_observations WHERE item_id IN ({})",
                placeholders.join(",")
            );
            let params_refs: Vec<&dyn rusqlite::types::ToSql> = chunk
                .iter()
                .map(|id| id as &dyn rusqlite::types::ToSql)
                .collect();
            tx.execute(&sql, params_refs.as_slice())?;
        }

        // Delete all clipboard_items.
        for chunk in item_ids.chunks(100) {
            let placeholders: Vec<String> = chunk
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 1))
                .collect();
            let sql = format!(
                "DELETE FROM clipboard_items WHERE id IN ({})",
                placeholders.join(",")
            );
            let params_refs: Vec<&dyn rusqlite::types::ToSql> = chunk
                .iter()
                .map(|id| id as &dyn rusqlite::types::ToSql)
                .collect();
            tx.execute(&sql, params_refs.as_slice())?;
        }

        tx.commit()?;

        Ok(ClearClipboardResult {
            deleted_items,
            deleted_favorites,
            deleted_hotkey_item_ids: hotkey_ids,
            deleted_file_paths,
            tombstones_written,
        })
    }
}

fn rfc3339_newer_str(a: &str, b: &str) -> bool {
    match (
        a.parse::<chrono::DateTime<chrono::Utc>>(),
        b.parse::<chrono::DateTime<chrono::Utc>>(),
    ) {
        (Ok(a), Ok(b)) => a > b,
        _ => a > b,
    }
}

fn rfc3339_newer_or_equal_str(a: &str, b: &str) -> bool {
    match (
        a.parse::<chrono::DateTime<chrono::Utc>>(),
        b.parse::<chrono::DateTime<chrono::Utc>>(),
    ) {
        (Ok(a), Ok(b)) => a >= b,
        _ => a >= b,
    }
}

fn row_to_item(row: &rusqlite::Row<'_>) -> SqlResult<ClipboardItem> {
    let ct_str: String = row.get(1)?;
    // Lazy reclassification: if migration v4 hasn't been applied yet (legacy DB),
    // normalize link/path/color content_type to plain_text with the corresponding
    // meta_type. Only sets meta_type when it's empty to avoid overwriting.
    let (ct_str, lazy_meta) = match ct_str.as_str() {
        "link" => ("plain_text".to_string(), Some("link")),
        "path" => ("plain_text".to_string(), Some("path")),
        "color" => ("plain_text".to_string(), Some("color")),
        _ => (ct_str, None),
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
    let meta_type: String = row.get(16).unwrap_or_default();
    let meta_type = if meta_type.is_empty() {
        lazy_meta.map(|s| s.to_string()).unwrap_or(meta_type)
    } else {
        meta_type
    };
    Ok(ClipboardItem {
        id: row.get(0)?,
        content_type: ContentType::from_str(&ct_str),
        full_text: row.get(2)?,
        content_hash: row.get::<_, i64>(3)? as u64,
        created_at: created_str.parse().unwrap_or_else(|_| {
            log::warn!("[db] unparseable created_at timestamp: {created_str:?}");
            chrono::DateTime::UNIX_EPOCH
        }),
        updated_at: updated_str.parse().unwrap_or_else(|_| {
            log::warn!("[db] unparseable updated_at timestamp: {updated_str:?}");
            chrono::DateTime::UNIX_EPOCH
        }),
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
        meta_type,
        custom_hotkey: row.get(17).unwrap_or_default(),
        custom_hotkey_format: row.get(18).unwrap_or_default(),
        existence_observed_at: row.get(19).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEMP_DB_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn temp_db(name: &str) -> (std::path::PathBuf, Database) {
        let counter = TEMP_DB_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "clippi-merge-test-{}-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            counter
        ));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("clippi.db");
        // Remove stale file if present.
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path.to_string_lossy()).unwrap();
        (path, db)
    }

    fn insert_item(db: &Database, hash: u64, text: &str, updated: &str) {
        db.conn.execute(
            "INSERT INTO clipboard_items (content_type, full_text, content_hash, created_at, updated_at, meta_type)
             VALUES ('plain_text', ?1, ?2, ?3, ?4, '')",
            rusqlite::params![text, hash as i64, updated, updated],
        )
        .unwrap();
    }

    fn insert_tag(db: &Database, name: &str, color: &str, updated: &str) -> i64 {
        let uid = legacy_tag_uid(name);
        db.conn
            .execute(
                "INSERT INTO tags (uid, name, color, updated_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![uid, name, color, updated],
            )
            .unwrap();
        db.conn.last_insert_rowid()
    }

    fn tag_item(db: &Database, item_id: i64, tag_id: i64) {
        db.conn
            .execute(
                "INSERT INTO item_tags (tag_id, item_id, used_at) VALUES (?1, ?2, datetime('now'))",
                rusqlite::params![tag_id, item_id],
            )
            .unwrap();
    }

    fn count_items(db: &Database) -> usize {
        db.conn
            .query_row("SELECT COUNT(*) FROM clipboard_items", [], |r| {
                r.get::<_, usize>(0)
            })
            .unwrap()
    }

    fn count_tags(db: &Database) -> usize {
        db.conn
            .query_row("SELECT COUNT(*) FROM tags", [], |r| r.get::<_, usize>(0))
            .unwrap()
    }

    #[test]
    fn record_tag_deletion_upserts_by_uid() {
        let (_path, db) = temp_db("tag-tombstone-upsert");
        db.record_tag_deletion("tag-1", "old", "2026-01-01T00:00:00Z", "a")
            .unwrap();
        db.record_tag_deletion("tag-1", "new", "2026-01-02T00:00:00Z", "b")
            .unwrap();

        let row: (String, String, String, String) = db
            .conn
            .query_row(
                "SELECT uid, name, deleted_at, device_name FROM deleted_tags",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row.0, "tag-1");
        assert_eq!(row.1, "new");
        assert_eq!(row.2, "2026-01-02T00:00:00Z");
        assert_eq!(row.3, "b");
    }

    // ── merge_from tests ──

    #[test]
    fn merge_adds_new_items() {
        let (src_path, src) = temp_db("src");
        let (_tgt_path, tgt) = temp_db("tgt");

        insert_item(&src, 100, "hello", "2025-01-01T00:00:00Z");
        // Target has different item
        insert_item(&tgt, 200, "world", "2025-01-01T00:00:00Z");

        let stats = tgt.merge_from(&src_path).unwrap();
        assert_eq!(stats.items_added, 1);
        assert_eq!(stats.items_updated, 0);
        assert_eq!(count_items(&tgt), 2); // both items present
    }

    #[test]
    fn merge_skips_duplicate_by_hash() {
        let (src_path, src) = temp_db("src");
        let (_tgt_path, tgt) = temp_db("tgt");

        insert_item(&src, 100, "hello", "2025-01-01T00:00:00Z");
        insert_item(&tgt, 100, "hello-old", "2024-06-01T00:00:00Z");

        let stats = tgt.merge_from(&src_path).unwrap();
        assert_eq!(stats.items_added, 0);
        // Source is newer → should update
        assert_eq!(stats.items_updated, 1);
        assert_eq!(count_items(&tgt), 1);
    }

    #[test]
    fn merge_preserves_newer_target_item() {
        let (src_path, src) = temp_db("src");
        let (_tgt_path, tgt) = temp_db("tgt");

        // Source has older version
        insert_item(&src, 100, "hello-old", "2024-06-01T00:00:00Z");
        // Target has newer version
        insert_item(&tgt, 100, "hello-new", "2025-01-01T00:00:00Z");

        let stats = tgt.merge_from(&src_path).unwrap();
        assert_eq!(stats.items_added, 0);
        assert_eq!(stats.items_updated, 0); // source is older, no update
        assert_eq!(count_items(&tgt), 1);

        // Verify target text wasn't changed.
        let text: String = tgt
            .conn
            .query_row(
                "SELECT full_text FROM clipboard_items WHERE content_hash = 100",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(text, "hello-new");
    }

    #[test]
    fn merge_preserves_cleanup_protection_fields() {
        // Stale-cleanup protection fields must survive a database merge:
        // 100 is a new row (INSERT path), 101 is updated by a newer source
        // row, 102 is updated by a source row without existence evidence.
        let (src_path, src) = temp_db("src-protect");
        let (_tgt_path, tgt) = temp_db("tgt-protect");

        // 100: only in source — carry over evidence + pending flag.
        src.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, image_path, \
                  source_app_name, source_app_icon, existence_observed_at, sync_pending) \
                 VALUES ('image', '', 100, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', \
                         '/tmp/src/100.png', 'Sync', 'icon', '2025-01-01T00:00:00Z', 1)",
                [],
            )
            .unwrap();

        // 101: target is older and locally captured (pending=0); the newer
        // source row points at a not-yet-downloaded sync blob (pending=1).
        // The winning source record must win on sync_pending, otherwise the
        // merged row pairs a missing blob path with an unprotected state.
        tgt.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, image_path, \
                  source_app_name, source_app_icon, existence_observed_at, sync_pending) \
                 VALUES ('image', '', 101, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', \
                         '/tmp/tgt/101.png', 'Local App', 'icon', '2024-01-01T00:00:00Z', 0)",
                [],
            )
            .unwrap();
        src.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, image_path, \
                  source_app_name, source_app_icon, existence_observed_at, sync_pending) \
                 VALUES ('image', '', 101, '2024-01-01T00:00:00Z', '2025-01-01T00:00:00Z', \
                         '/tmp/src/101.png', '', '', '2025-01-01T00:00:00Z', 1)",
                [],
            )
            .unwrap();

        // 102: target has capture evidence, the newer source row has none —
        // the non-empty evidence rule must keep the target value.
        tgt.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, image_path, \
                  source_app_name, source_app_icon, existence_observed_at, sync_pending) \
                 VALUES ('image', '', 102, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', \
                         '/tmp/tgt/102.png', 'Local App', 'icon', '2024-01-01T00:00:00Z', 0)",
                [],
            )
            .unwrap();
        src.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, image_path, \
                  source_app_name, source_app_icon, existence_observed_at, sync_pending) \
                 VALUES ('image', '', 102, '2024-01-01T00:00:00Z', '2025-01-01T00:00:00Z', \
                         '/tmp/src/102.png', '', '', '', 0)",
                [],
            )
            .unwrap();

        let stats = tgt.merge_from(&src_path).unwrap();
        assert_eq!(stats.items_added, 1);
        assert_eq!(stats.items_updated, 2);

        let row = |hash: i64| -> (String, i64) {
            tgt.conn
                .query_row(
                    "SELECT existence_observed_at, sync_pending FROM clipboard_items \
                     WHERE content_hash = ?1",
                    [hash],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap()
        };

        // 100: new row keeps both protection fields.
        assert_eq!(row(100), ("2025-01-01T00:00:00Z".to_string(), 1));
        // 101: newer source record wins on sync_pending and evidence.
        assert_eq!(row(101), ("2025-01-01T00:00:00Z".to_string(), 1));
        // 102: empty source evidence must not erase target evidence.
        assert_eq!(row(102), ("2024-01-01T00:00:00Z".to_string(), 0));
    }

    #[test]
    fn merge_adds_new_tags() {
        let (src_path, src) = temp_db("src");
        let (_tgt_path, tgt) = temp_db("tgt");

        insert_tag(&src, "rust", "#FF0000", "2025-01-01T00:00:00Z");
        insert_tag(&tgt, "golang", "#00FF00", "2025-01-01T00:00:00Z");

        let stats = tgt.merge_from(&src_path).unwrap();
        assert_eq!(stats.tags_added, 1);
        assert_eq!(stats.tags_updated, 0);
        assert_eq!(count_tags(&tgt), 2);
    }

    #[test]
    fn merge_updates_tag_when_source_newer() {
        let (src_path, src) = temp_db("src");
        let (_tgt_path, tgt) = temp_db("tgt");

        insert_tag(&src, "rust", "#FF0000", "2025-06-01T00:00:00Z");
        insert_tag(&tgt, "rust", "#0000FF", "2024-01-01T00:00:00Z");

        let stats = tgt.merge_from(&src_path).unwrap();
        assert_eq!(stats.tags_added, 0);
        assert_eq!(stats.tags_updated, 1);

        let color: String = tgt
            .conn
            .query_row("SELECT color FROM tags WHERE name = 'rust'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(color, "#FF0000");
    }

    #[test]
    fn merge_resolves_tag_associations_across_ids() {
        let (src_path, src) = temp_db("src");
        let (_tgt_path, tgt) = temp_db("tgt");

        // Source: item hash=100 tagged "rust"
        insert_item(&src, 100, "hello", "2025-01-01T00:00:00Z");
        let src_tag = insert_tag(&src, "rust", "#FF0000", "2025-01-01T00:00:00Z");
        let src_item_id: i64 = src
            .conn
            .query_row(
                "SELECT id FROM clipboard_items WHERE content_hash = 100",
                [],
                |r| r.get(0),
            )
            .unwrap();
        tag_item(&src, src_item_id, src_tag);

        // Target: item hash=100 already exists (will be updated by source),
        // tag "rust" NOT yet in target.
        insert_item(&tgt, 100, "hello-old", "2024-06-01T00:00:00Z");

        let stats = tgt.merge_from(&src_path).unwrap();
        assert!(stats.items_updated >= 1);
        assert!(stats.tags_added >= 1);

        // Verify tag association was carried over.
        let tag_count: usize = tgt
            .conn
            .query_row(
                "SELECT COUNT(*) FROM item_tags it
                 JOIN tags t ON t.id = it.tag_id
                 JOIN clipboard_items ci ON ci.id = it.item_id
                 WHERE t.name = 'rust' AND ci.content_hash = 100",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tag_count, 1);
    }

    #[test]
    fn merge_tombstones_are_inserted() {
        let (src_path, src) = temp_db("src");
        let (_tgt_path, tgt) = temp_db("tgt");

        // Insert a deleted item tombstone in source.
        src.conn
            .execute(
                "INSERT INTO deleted_items (content_hash, deleted_at, device_name) VALUES (999, '2025-01-01T00:00:00Z', 'test')",
                [],
            )
            .unwrap();

        let stats = tgt.merge_from(&src_path).unwrap();

        let tombstones: usize = tgt
            .conn
            .query_row("SELECT COUNT(*) FROM deleted_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tombstones, 1);
        // Stats don't track tombstones separately.
        assert_eq!(stats.items_added + stats.items_updated, 0);
    }

    #[test]
    fn merge_idempotent() {
        let (src_path, src) = temp_db("src");
        let (_tgt_path, tgt) = temp_db("tgt");

        insert_item(&src, 100, "hello", "2025-01-01T00:00:00Z");
        insert_tag(&src, "rust", "#FF0000", "2025-01-01T00:00:00Z");

        // First merge.
        let stats1 = tgt.merge_from(&src_path).unwrap();
        assert_eq!(stats1.items_added, 1);
        assert_eq!(stats1.tags_added, 1);

        // Second merge — nothing new to add.
        let stats2 = tgt.merge_from(&src_path).unwrap();
        assert_eq!(stats2.items_added, 0);
        assert_eq!(stats2.items_updated, 0);
        assert_eq!(stats2.tags_added, 0);
        assert_eq!(stats2.tags_updated, 0);
        assert_eq!(count_items(&tgt), 1);
        assert_eq!(count_tags(&tgt), 1);
    }

    #[test]
    fn merge_both_directions_same_result() {
        // Merging A→B should produce the same item count as B→A (union).
        let (a_path, a) = temp_db("a");
        let (_b_path, b) = temp_db("b");
        let (c_path, c) = temp_db("c"); // copy of B for reverse merge

        insert_item(&a, 100, "hello", "2025-01-01T00:00:00Z");
        insert_item(&b, 200, "world", "2025-01-01T00:00:00Z");
        insert_item(&c, 200, "world", "2025-01-01T00:00:00Z");

        // A → B
        b.merge_from(&a_path).unwrap();
        // B (now merged) ← A (reverse)
        a.merge_from(&c_path).unwrap();

        assert_eq!(count_items(&a), 2);
        assert_eq!(count_items(&b), 2);
    }

    // ── prune_excess_non_favorites ──────────────────────────────────────

    #[test]
    fn prune_excess_max_items_zero_does_nothing() {
        let (_path, db) = temp_db("prune-max-zero");
        insert_item(&db, 1, "a", "2025-01-01T00:00:00Z");
        insert_item(&db, 2, "b", "2025-01-01T00:00:00Z");
        assert_eq!(count_items(&db), 2);
        let removed = db.prune_excess_non_favorites(0).unwrap();
        assert!(removed.is_empty());
        assert_eq!(count_items(&db), 2);
    }

    #[test]
    fn transfer_backing_rows_do_not_consume_history_limits_or_expire() {
        let (_path, db) = temp_db("prune-transfer-backing");
        insert_item(&db, 1, "ordinary", "2025-01-01T00:00:00Z");
        db.conn
            .execute(
                "INSERT INTO clipboard_items
                 (content_type, full_text, content_hash, created_at, updated_at, meta_type)
                 VALUES ('file', 'transfer.bin', 2, '2020-01-01T00:00:00Z',
                         '2020-01-01T00:00:00Z', 'transfer')",
                [],
            )
            .unwrap();

        assert!(db.prune_excess_non_favorites(1).unwrap().is_empty());
        assert_eq!(db.prune_expired_items(1).unwrap().len(), 1);
        assert_eq!(count_items(&db), 1);
        let remaining_meta: String = db
            .conn
            .query_row("SELECT meta_type FROM clipboard_items", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(remaining_meta, "transfer");
    }

    #[test]
    fn prune_excess_removes_oldest_non_favorites() {
        let (_path, db) = temp_db("prune-max-excess");
        insert_item(&db, 1, "oldest", "2025-01-01T00:00:00Z");
        insert_item(&db, 2, "middle", "2025-01-02T00:00:00Z");
        insert_item(&db, 3, "newest", "2025-01-03T00:00:00Z");
        assert_eq!(count_items(&db), 3);

        let removed = db.prune_excess_non_favorites(2).unwrap();
        assert_eq!(removed.len(), 1);
        // Oldest should be removed.
        let remaining: Vec<String> = db
            .conn
            .prepare("SELECT full_text FROM clipboard_items ORDER BY created_at ASC")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(remaining, vec!["middle".to_string(), "newest".to_string()]);
    }

    #[test]
    fn prune_excess_keeps_favorites() {
        let (_path, db) = temp_db("prune-max-fav");
        // Insert 3 non-fav items.
        insert_item(&db, 1, "a", "2025-01-01T00:00:00Z");
        insert_item(&db, 2, "b", "2025-01-01T00:00:00Z");
        insert_item(&db, 3, "c", "2025-01-01T00:00:00Z");
        // Make item 'a' a favorite.
        db.conn
            .execute(
                "UPDATE clipboard_items SET is_favorite = 1 WHERE full_text = 'a'",
                [],
            )
            .unwrap();
        assert_eq!(count_items(&db), 3);

        // Only 2 non-fav items, limit = 1 → should remove oldest non-fav.
        let removed = db.prune_excess_non_favorites(1).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(count_items(&db), 2);

        // Remaining: the favorite + 1 non-fav.
        let remaining: Vec<String> = db
            .conn
            .prepare("SELECT full_text FROM clipboard_items ORDER BY created_at ASC")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        // 'a' is fav, 'b' was oldest non-fav, so 'c' remains as non-fav.
        assert!(remaining.contains(&"a".to_string()));
        assert!(remaining.contains(&"c".to_string()));
    }

    #[test]
    fn prune_excess_does_not_over_prune() {
        let (_path, db) = temp_db("prune-max-exact");
        insert_item(&db, 1, "a", "2025-01-01T00:00:00Z");
        insert_item(&db, 2, "b", "2025-01-01T00:00:00Z");
        assert_eq!(count_items(&db), 2);
        let removed = db.prune_excess_non_favorites(2).unwrap();
        assert!(removed.is_empty());
        assert_eq!(count_items(&db), 2);
    }

    // ── prune_expired_items ─────────────────────────────────────────────

    #[test]
    fn prune_expired_retention_zero_does_nothing() {
        let (_path, db) = temp_db("prune-exp-zero");
        insert_item(&db, 1, "old", "2020-01-01T00:00:00Z");
        assert_eq!(count_items(&db), 1);
        let removed = db.prune_expired_items(0).unwrap();
        assert!(removed.is_empty());
        assert_eq!(count_items(&db), 1);
    }

    #[test]
    fn prune_expired_removes_old_items() {
        let (_path, db) = temp_db("prune-exp-old");
        // Insert an item with very old updated_at.
        db.conn
            .execute(
                "INSERT INTO clipboard_items (content_type, full_text, content_hash, created_at, updated_at, meta_type)
                 VALUES ('plain_text', 'old_item', 1, '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z', '')",
                [],
            )
            .unwrap();
        // Insert a recent item.
        db.conn
            .execute(
                "INSERT INTO clipboard_items (content_type, full_text, content_hash, created_at, updated_at, meta_type)
                 VALUES ('plain_text', 'recent_item', 2, datetime('now'), datetime('now'), '')",
                [],
            )
            .unwrap();
        assert_eq!(count_items(&db), 2);

        // With 30-day retention, only the old item should be removed.
        let removed = db.prune_expired_items(30).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].content_hash, 1);
        assert_eq!(removed[0].content_type, ContentType::PlainText);
        assert_eq!(count_items(&db), 1);

        let remaining: String = db
            .conn
            .query_row("SELECT full_text FROM clipboard_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, "recent_item");
    }

    #[test]
    fn prune_expired_writes_sync_tombstones_atomically() {
        let (_path, db) = temp_db("prune-exp-sync");
        insert_item(&db, 77, "old", "2020-01-01T00:00:00Z");
        let scope = CleanupSyncScope {
            include_images: true,
            favorites_only: false,
            device_name: "test-device".to_string(),
        };

        let (removed, tombstones) = db
            .prune_expired_items_with_sync_scope(30, Some(&scope))
            .unwrap();

        assert_eq!(removed.len(), 1);
        assert_eq!(tombstones, 1);
        assert_eq!(count_items(&db), 0);
        assert!(db.is_item_tombstoned(77).unwrap());
    }

    #[test]
    fn prune_expired_rolls_back_when_tombstone_write_fails() {
        let (_path, db) = temp_db("prune-exp-sync-rollback");
        insert_item(&db, 78, "old", "2020-01-01T00:00:00Z");
        db.conn
            .execute_batch(
                "CREATE TRIGGER reject_expiry_tombstone \
                 BEFORE INSERT ON deleted_items \
                 BEGIN SELECT RAISE(ABORT, 'reject tombstone'); END;",
            )
            .unwrap();
        let scope = CleanupSyncScope {
            include_images: true,
            favorites_only: false,
            device_name: "test-device".to_string(),
        };

        assert!(db
            .prune_expired_items_with_sync_scope(30, Some(&scope))
            .is_err());
        assert_eq!(count_items(&db), 1);
        assert!(!db.is_item_tombstoned(78).unwrap());
    }

    #[test]
    fn prune_expired_compares_rfc3339_timestamps_by_time_value() {
        let (_path, db) = temp_db("prune-exp-rfc3339");
        db.conn
            .execute(
                "INSERT INTO clipboard_items (content_type, full_text, content_hash, created_at, updated_at, meta_type)
                 VALUES (
                     'plain_text',
                     'older_than_one_day',
                     1,
                     strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-25 hours'),
                     strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-25 hours'),
                     ''
                 )",
                [],
            )
            .unwrap();

        let removed = db.prune_expired_items(1).unwrap();

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].content_hash, 1);
        assert_eq!(count_items(&db), 0);
    }

    #[test]
    fn prune_expired_keeps_favorites() {
        let (_path, db) = temp_db("prune-exp-fav");
        db.conn
            .execute(
                "INSERT INTO clipboard_items (content_type, full_text, content_hash, created_at, updated_at, meta_type, is_favorite)
                 VALUES ('plain_text', 'fav_old', 1, '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z', '', 1)",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO clipboard_items (content_type, full_text, content_hash, created_at, updated_at, meta_type)
                 VALUES ('plain_text', 'nonfav_old', 2, '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z', '')",
                [],
            )
            .unwrap();
        assert_eq!(count_items(&db), 2);

        let removed = db.prune_expired_items(30).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(count_items(&db), 1);

        let remaining: String = db
            .conn
            .query_row("SELECT full_text FROM clipboard_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, "fav_old");
    }

    #[test]
    fn prune_expired_returns_custom_hotkey_metadata() {
        let (_path, db) = temp_db("prune-exp-hotkey");
        insert_item(&db, 1, "hotkey_old", "2020-01-01T00:00:00Z");
        db.conn
            .execute(
                "UPDATE clipboard_items SET custom_hotkey = 'Ctrl+Alt+1' WHERE content_hash = 1",
                [],
            )
            .unwrap();

        let removed = db.prune_expired_items(30).unwrap();

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].content_hash, 1);
        assert_eq!(removed[0].custom_hotkey, "Ctrl+Alt+1");
    }

    #[test]
    fn prune_expired_recent_items_not_removed() {
        let (_path, db) = temp_db("prune-exp-recent");
        db.conn
            .execute(
                "INSERT INTO clipboard_items (content_type, full_text, content_hash, created_at, updated_at, meta_type)
                 VALUES ('plain_text', 'today', 1, datetime('now'), datetime('now'), '')",
                [],
            )
            .unwrap();
        assert_eq!(count_items(&db), 1);

        // With 7-day retention, a brand-new item should stay.
        let removed = db.prune_expired_items(7).unwrap();
        assert!(removed.is_empty());
        assert_eq!(count_items(&db), 1);
    }

    #[test]
    fn get_all_custom_item_hotkeys_reads_beyond_loaded_item_pages() {
        let (_path, db) = temp_db("all-hotkeys");
        insert_item(&db, 1, "plain", "2025-01-01T00:00:00Z");
        insert_item(&db, 2, "hotkey", "2025-01-02T00:00:00Z");
        let hotkey_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM clipboard_items WHERE content_hash = 2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        db.set_item_hotkey(hotkey_id, "Ctrl+Alt+2", "").unwrap();

        let hotkeys = db.get_all_custom_item_hotkeys().unwrap();

        assert_eq!(hotkeys, vec![(hotkey_id, "Ctrl+Alt+2".to_string())]);
    }

    #[test]
    fn titlebar_stats_are_zero_for_an_empty_database() {
        let (_path, db) = temp_db("empty-titlebar-stats");

        let stats = db.load_titlebar_stats().unwrap();

        assert!(!stats.has_hotkey_items);
        assert!(!stats.has_favorite_items);
        assert_eq!(stats.clearable_history_count, 0);
        assert_eq!(stats.clearable_non_favorite_history_count, 0);
    }

    #[test]
    fn clear_clipboard_history_is_atomic_and_preserves_transfer_rows_and_tags() {
        let (path, db) = temp_db("clear-history");
        let missing_file = path.parent().unwrap().join("missing-file.txt");
        let file_data = FileData {
            files: vec![crate::core::types::FileInfo {
                name: "missing-file.txt".into(),
                path: missing_file.to_string_lossy().into_owned(),
                is_dir: false,
            }],
            transfer: false,
            remote_hash: String::new(),
        }
        .to_json();

        db.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, \
                  is_favorite, custom_hotkey, file_data, meta_type) VALUES \
                 ('plain_text', 'text', 101, ?1, ?1, 0, 'Ctrl+Alt+1', '', ''), \
                 ('image', 'image', 102, ?1, ?1, 1, '', '', ''), \
                 ('file', 'missing-file.txt', 103, ?1, ?1, 0, '', ?2, ''), \
                 ('file', 'transfer', 104, ?1, ?1, 0, '', '', 'transfer')",
                params!["2026-07-28T00:00:00Z", file_data],
            )
            .unwrap();
        let tag_id = insert_tag(&db, "keep-tag", "#FF0000", "2026-07-28T00:00:00Z");
        let text_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM clipboard_items WHERE content_hash = 101",
                [],
                |row| row.get(0),
            )
            .unwrap();
        tag_item(&db, text_id, tag_id);
        db.conn
            .execute(
                "INSERT INTO deleted_items (content_hash, deleted_at, device_name) \
                 VALUES (101, '2025-01-01T00:00:00Z', 'old-device')",
                [],
            )
            .unwrap();

        let stats = db.load_titlebar_stats().unwrap();
        assert_eq!(stats.clearable_history_count, 3);
        assert_eq!(stats.clearable_non_favorite_history_count, 2);
        assert!(stats.has_hotkey_items);
        assert!(stats.has_favorite_items);
        let result = db.clear_clipboard_history("test-device", true).unwrap();

        assert_eq!(result.deleted_items, 3);
        assert_eq!(result.deleted_favorites, 1);
        assert_eq!(result.deleted_hotkey_item_ids, vec![text_id]);
        assert_eq!(
            result.deleted_file_paths,
            vec![missing_file.to_string_lossy().into_owned()]
        );
        assert_eq!(result.tombstones_written, 2);
        assert_eq!(count_items(&db), 1);
        assert_eq!(count_tags(&db), 1);
        let associations: u32 = db
            .conn
            .query_row("SELECT COUNT(*) FROM item_tags", [], |row| row.get(0))
            .unwrap();
        assert_eq!(associations, 0);
        let tombstones: u32 = db
            .conn
            .query_row("SELECT COUNT(*) FROM deleted_items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tombstones, 2);
        let tombstone_device: String = db
            .conn
            .query_row(
                "SELECT device_name FROM deleted_items WHERE content_hash = 101",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tombstone_device, "test-device");
    }

    #[test]
    fn stale_delete_rechecks_identity_and_returns_deleted_file_paths() {
        let (path, db) = temp_db("stale-identity");
        let missing_file = path.parent().unwrap().join("missing.txt");
        let file_data = FileData {
            files: vec![crate::core::types::FileInfo {
                name: "missing.txt".into(),
                path: missing_file.to_string_lossy().into_owned(),
                is_dir: false,
            }],
            transfer: false,
            remote_hash: String::new(),
        }
        .to_json();
        db.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, file_data, \
                  meta_type, existence_observed_at) \
                 VALUES ('file', 'missing.txt', 201, ?1, ?1, ?2, '', ?3)",
                params!["2026-07-28T00:00:00Z", file_data, "2026-07-28T00:00:00Z"],
            )
            .unwrap();

        let t0 = "2026-07-28T00:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap();

        // Observation phase 1: missing but not eligible.
        let mut stats = crate::core::cache_cleanup::CleanupStats::default();
        crate::core::cache_cleanup::run_stale_scan(&db, t0, None, &mut stats);
        assert_eq!(stats.stale_items, 0);
        assert_eq!(count_items(&db), 1);

        // The record changes between the scan and the delete: the
        // in-transaction identity re-check must skip it.
        let confirmed = vec![crate::core::cache_cleanup::ConfirmedStaleItem {
            id: db.get_by_hash(201).unwrap().unwrap().id,
            content_hash: 201,
            expected_updated_at: t0,
        }];
        db.conn
            .execute(
                "UPDATE clipboard_items SET updated_at = ?1 WHERE content_hash = 201",
                params!["2026-07-28T00:00:01Z"],
            )
            .unwrap();
        let skipped = db.delete_stale_items(&confirmed, None).unwrap();
        assert_eq!(skipped.deleted_items, 0);
        assert_eq!(count_items(&db), 1);

        // The identity change restarts the observation: the next scan is a
        // fresh first-missing observation again.
        let mut stats = crate::core::cache_cleanup::CleanupStats::default();
        crate::core::cache_cleanup::run_stale_scan(
            &db,
            t0 + chrono::Duration::hours(25),
            None,
            &mut stats,
        );
        assert_eq!(stats.stale_first_missing, 1);
        assert_eq!(stats.stale_items, 0);

        // A further scan beyond the (restarted) grace period deletes the
        // item, with the deleted file paths reported for transfer refresh.
        let mut stats = crate::core::cache_cleanup::CleanupStats::default();
        crate::core::cache_cleanup::run_stale_scan(
            &db,
            t0 + chrono::Duration::hours(49),
            None,
            &mut stats,
        );
        assert_eq!(stats.stale_eligible, 1);
        assert_eq!(stats.stale_items, 1);
        assert_eq!(
            stats.deleted_file_paths,
            vec![missing_file.to_string_lossy().into_owned()]
        );
        assert_eq!(count_items(&db), 0);
    }

    #[test]
    fn clear_clipboard_history_preserves_favorites_by_default() {
        let (_path, db) = temp_db("clear-preserve-favorites");
        db.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, is_favorite) \
                 VALUES ('plain_text', 'normal', 401, ?1, ?1, 0), \
                        ('plain_text', 'favorite', 402, ?1, ?1, 1)",
                params!["2026-07-28T00:00:00Z"],
            )
            .unwrap();

        let result = db.clear_clipboard_history("test-device", false).unwrap();

        assert_eq!(result.deleted_items, 1);
        assert_eq!(result.deleted_favorites, 0);
        assert!(db.get_by_hash(401).unwrap().is_none());
        assert!(db.get_by_hash(402).unwrap().is_some());
    }

    #[test]
    fn update_sync_item_image_marks_sync_pending_and_keeps_source_metadata() {
        let (_path, db) = temp_db("sync-item-pending");
        let images_dir = crate::core::paths::images_dir();
        let local_path = images_dir.join(format!(
            "local-captured-{}-{}.png",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        db.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, image_path, \
                  source_app_name, source_app_icon, existence_observed_at) \
                 VALUES ('image', '', 601, ?1, ?1, ?2, 'Local App', 'icon', ?3)",
                params![
                    "2026-07-28T00:00:00Z",
                    local_path.to_string_lossy(),
                    "2026-07-28T00:00:00Z"
                ],
            )
            .unwrap();

        // A newer remote version points at a sync blob that may not be
        // downloaded yet (e.g. failed download).
        let remote = crate::core::sync::SyncItem {
            content_type: "image".to_string(),
            full_text: String::new(),
            content_hash: 601,
            created_at: "2026-07-28T00:00:00Z".to_string(),
            updated_at: "2026-07-28T01:00:00Z".to_string(),
            rich_data: String::new(),
            is_favorite: false,
            note: String::new(),
            size: 0,
            tags: Vec::new(),
            meta_type: String::new(),
            image_width: 100,
            image_height: 100,
            image_blob: "0000000000000259.jpg".to_string(),
        };
        let id = db.get_by_hash(601).unwrap().unwrap().id;
        db.update_sync_item(id, &remote).unwrap();

        // Local display metadata is preserved; the sync-pending flag marks
        // the item as sync-owned so the classifier protects it while the
        // blob is missing.
        let item = db.get_by_hash(601).unwrap().unwrap();
        assert_eq!(item.source_app_name, "Local App");
        assert_eq!(item.source_app_icon, "icon");
        assert!(item.image_path.ends_with("0000000000000259.jpg"));

        let candidates = db.find_stale_item_candidates().unwrap().0;
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].sync_pending);
        assert_eq!(
            crate::core::cache_cleanup::classify_item_status(&candidates[0]),
            crate::core::cache_cleanup::ItemStatus::Protected {
                reason: crate::core::types::PathStatusReason::PendingSync
            }
        );

        // Blob download completes → path recorded and pending cleared.
        let blob = images_dir.join("0000000000000259.jpg");
        std::fs::write(&blob, b"jpg").unwrap();
        db.set_item_image_path(601, &blob.to_string_lossy())
            .unwrap();
        let candidates = db.find_stale_item_candidates().unwrap().0;
        assert!(!candidates[0].sync_pending);
        assert_eq!(
            crate::core::cache_cleanup::classify_item_status(&candidates[0]),
            crate::core::cache_cleanup::ItemStatus::Present
        );
        std::fs::remove_file(&blob).unwrap();
    }

    #[test]
    fn corrupt_observation_timestamps_are_treated_as_no_history() {
        let (_path, db) = temp_db("stale-corrupt-obs");
        let images_dir = crate::core::paths::images_dir();
        let missing = images_dir.join(format!(
            "missing-local-{}-{}.png",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        db.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, image_path, \
                  source_app_name) \
                 VALUES ('image', '', 701, ?1, ?1, ?2, 'Local App')",
                params!["2026-07-28T00:00:00Z", missing.to_string_lossy()],
            )
            .unwrap();
        // A corrupt observation: count already at 1, but timestamps are
        // garbage. If parsed as UNIX_EPOCH the grace period would appear
        // satisfied and one more scan would delete the item.
        db.conn
            .execute(
                "INSERT INTO stale_item_observations \
                 (item_id, content_hash, item_updated_at, first_missing_at, last_checked_at, \
                  consecutive_missing_count, last_status, last_reason) \
                 SELECT id, content_hash, 'garbage', 'garbage', 'garbage', 1, 'missing', '' \
                 FROM clipboard_items WHERE content_hash = 701",
                [],
            )
            .unwrap();

        // The corrupt observation is treated as absent: the next scan
        // restarts the counter instead of deleting the item.
        let candidates = db.find_stale_item_candidates().unwrap().0;
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].observation.is_none());

        let t0 = "2026-07-28T00:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap();
        let mut stats = crate::core::cache_cleanup::CleanupStats {
            scan_complete: true,
            ..crate::core::cache_cleanup::CleanupStats::default()
        };
        crate::core::cache_cleanup::run_stale_scan(&db, t0, None, &mut stats);
        assert_eq!(stats.stale_first_missing, 1);
        assert_eq!(stats.stale_eligible, 0);
        assert_eq!(stats.stale_items, 0);
        assert_eq!(count_items(&db), 1);
    }

    #[test]
    fn corrupt_observation_count_is_treated_as_no_history() {
        let (_path, db) = temp_db("stale-corrupt-count");
        let images_dir = crate::core::paths::images_dir();
        let missing = images_dir.join(format!(
            "missing-local-{}-{}.png",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        db.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, image_path, \
                  source_app_name) \
                 VALUES ('image', '', 702, ?1, ?1, ?2, 'Local App')",
                params!["2026-07-28T00:00:00Z", missing.to_string_lossy()],
            )
            .unwrap();
        // Valid timestamps (already past the grace period) but a corrupt
        // negative count. Cast to u32 would make it huge and instantly
        // satisfy the delete threshold.
        db.conn
            .execute(
                "INSERT INTO stale_item_observations \
                 (item_id, content_hash, item_updated_at, first_missing_at, last_checked_at, \
                  consecutive_missing_count, last_status, last_reason) \
                 SELECT id, content_hash, '2026-07-20T00:00:00Z', '2026-07-20T00:00:00Z', \
                        '2026-07-28T00:00:00Z', -5, 'missing', '' \
                 FROM clipboard_items WHERE content_hash = 702",
                [],
            )
            .unwrap();

        // Out-of-range count → observation treated as absent.
        let candidates = db.find_stale_item_candidates().unwrap().0;
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].observation.is_none());

        let t0 = "2026-07-28T00:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap();
        let mut stats = crate::core::cache_cleanup::CleanupStats {
            scan_complete: true,
            ..crate::core::cache_cleanup::CleanupStats::default()
        };
        crate::core::cache_cleanup::run_stale_scan(&db, t0, None, &mut stats);
        assert_eq!(stats.stale_first_missing, 1);
        assert_eq!(stats.stale_eligible, 0);
        assert_eq!(stats.stale_items, 0);
        assert_eq!(count_items(&db), 1);
    }

    #[test]
    fn local_upsert_clears_sync_pending() {
        let (_path, db) = temp_db("upsert-clears-pending");
        let images_dir = crate::core::paths::images_dir();
        let blob = images_dir.join("0000000000000259.jpg");
        db.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, image_path, \
                  source_app_name, sync_pending) \
                 VALUES ('image', '', 801, ?1, ?1, ?2, 'Local App', 1)",
                params!["2026-07-28T00:00:00Z", blob.to_string_lossy()],
            )
            .unwrap();

        // A local clipboard capture of the same content (same hash) ends the
        // sync-pending state: the item is locally managed again.
        let mut item = ClipboardItem::new_image(0, &blob.to_string_lossy(), 801, 100, 100, None);
        item.source_app_name = "Local App".to_string();
        item.existence_observed_at = "2026-07-28T01:00:00Z".to_string();
        item.updated_at = "2026-07-28T01:00:00Z".parse().unwrap();
        db.upsert(&item).unwrap();

        let pending: i64 = db
            .conn
            .query_row(
                "SELECT sync_pending FROM clipboard_items WHERE content_hash = 801",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0);
    }

    #[test]
    fn stale_candidates_skip_rows_with_unparseable_timestamps() {
        let (_path, db) = temp_db("stale-tolerant");
        let images_dir = crate::core::paths::images_dir();
        let missing = images_dir.join(format!(
            "missing-local-{}-{}.png",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        // A normal candidate with evidence.
        db.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, image_path, \
                  source_app_name, existence_observed_at) \
                 VALUES ('image', '', 401, ?1, ?1, ?2, 'Local App', ?3)",
                params![
                    "2026-07-28T00:00:00Z",
                    missing.to_string_lossy(),
                    "2026-07-28T00:00:00Z"
                ],
            )
            .unwrap();
        // A corrupt row whose updated_at cannot be parsed.
        db.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, image_path, \
                  source_app_name) \
                 VALUES ('image', '', 402, ?1, 'not-a-timestamp', ?2, 'Local App')",
                params!["2026-07-28T00:00:00Z", missing.to_string_lossy()],
            )
            .unwrap();

        let (candidates, skipped) = db.find_stale_item_candidates().unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(skipped, 1);
        assert_eq!(candidates[0].content_hash, 401);
    }

    #[test]
    fn stale_cleanup_deletes_missing_local_image_but_keeps_pending_sync_image() {
        let (_path, db) = temp_db("stale-images");
        let images_dir = crate::core::paths::images_dir();
        let local_image = images_dir.join(format!(
            "missing-local-{}-{}.png",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let synced_image = images_dir.join(format!(
            "missing-synced-{}-{}.png",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        assert!(!local_image.exists());
        assert!(!synced_image.exists());

        db.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, image_path, \
                  source_app_name, meta_type) \
                 VALUES ('image', '', 301, ?1, ?1, ?2, 'Local App', '')",
                params![
                    "2026-07-28T00:00:00Z",
                    local_image.to_string_lossy().as_ref()
                ],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, image_path, \
                  source_app_name, meta_type, sync_pending) \
                 VALUES ('image', '', 302, ?1, ?1, ?2, '', '', 1)",
                params![
                    "2026-07-28T00:00:00Z",
                    synced_image.to_string_lossy().as_ref()
                ],
            )
            .unwrap();

        let (candidates, skipped) = db.find_stale_item_candidates().unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(skipped, 0);

        let t0 = "2026-07-28T00:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap();

        // Observation phase 1: the local image is first-missing; the synced
        // image is protected (PendingSync) and never counted as missing.
        let mut stats = crate::core::cache_cleanup::CleanupStats::default();
        crate::core::cache_cleanup::run_stale_scan(&db, t0, None, &mut stats);
        assert_eq!(stats.stale_scanned, 2);
        assert_eq!(stats.stale_first_missing, 1);
        assert_eq!(stats.stale_protected, 1);
        assert_eq!(stats.stale_items, 0);

        // Observation phase 2 beyond grace: the local image is deleted;
        // the pending-sync image survives.
        let mut stats = crate::core::cache_cleanup::CleanupStats::default();
        crate::core::cache_cleanup::run_stale_scan(
            &db,
            t0 + chrono::Duration::hours(25),
            None,
            &mut stats,
        );
        assert_eq!(stats.stale_items, 1);
        assert!(db.get_by_hash(301).unwrap().is_none());
        assert!(db.get_by_hash(302).unwrap().is_some());
    }

    // ── touch_items (batch usage-time updates) ──────────────────────

    #[test]
    fn touch_items_updates_all_ids_across_chunk_boundaries() {
        let db = Database::open(":memory:").unwrap();
        let old = "2020-01-01T00:00:00Z";
        let mut ids = Vec::with_capacity(1001);
        for i in 0..1001i64 {
            db.conn
                .execute(
                    "INSERT INTO clipboard_items \
                     (content_type, full_text, content_hash, created_at, updated_at, meta_type) \
                     VALUES ('plain_text', ?1, ?2, ?3, ?3, '')",
                    params![format!("item {i}"), i, old],
                )
                .unwrap();
            ids.push(db.conn.last_insert_rowid());
        }

        let now = "2026-08-10T00:00:00Z";
        let affected = db.touch_items(&ids, now).unwrap();
        assert_eq!(affected, 1001);

        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM clipboard_items WHERE updated_at = ?1",
                params![now],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1001);
    }

    #[test]
    fn touch_items_is_atomic_on_failure() {
        let db = Database::open(":memory:").unwrap();
        db.conn
            .execute(
                "INSERT INTO clipboard_items \
                 (content_type, full_text, content_hash, created_at, updated_at, meta_type) \
                 VALUES ('plain_text', 'a', 1, ?1, ?1, ''), \
                        ('plain_text', 'b', 2, ?1, ?1, '')",
                params!["2020-01-01T00:00:00Z"],
            )
            .unwrap();
        db.conn
            .execute_batch(
                "CREATE TRIGGER reject_touch BEFORE UPDATE OF updated_at ON clipboard_items \
                 BEGIN SELECT RAISE(ABORT, 'reject'); END;",
            )
            .unwrap();

        let ids: Vec<i64> = db
            .conn
            .prepare("SELECT id FROM clipboard_items ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(db.touch_items(&ids, "2026-08-10T00:00:00Z").is_err());

        // No partial update: both rows keep the old timestamp.
        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM clipboard_items \
                 WHERE updated_at = '2020-01-01T00:00:00Z'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn touch_items_empty_is_noop() {
        let db = Database::open(":memory:").unwrap();
        assert_eq!(db.touch_items(&[], "2026-08-10T00:00:00Z").unwrap(), 0);
    }
}
