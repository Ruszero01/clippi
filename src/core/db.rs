//! Database persistence for clipboard items

use crate::core::filters::ClipboardFilters;
use crate::core::types::{ClipboardItem, ContentType, TagInfo};
use rusqlite::{params, Connection, Result as SqlResult};
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

pub struct Database {
    conn: Connection,
}

const SOURCE_APP_ICON_INLINE_LIMIT: usize = 256 * 1024;
const LIST_FULL_TEXT_LIMIT: usize = 8192;
const LIST_RICH_HTML_LIMIT: usize = 4096;
const LIST_RICH_AUX_LIMIT: usize = 2048;
const LIST_NOTE_LIMIT: usize = 2048;

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
        "id, content_type, full_text, content_hash, created_at, updated_at, image_path, rich_data, file_data, is_favorite, note, source_app_name, CASE WHEN length(source_app_icon) <= {SOURCE_APP_ICON_INLINE_LIMIT} THEN source_app_icon ELSE '' END, image_width, image_height, size, meta_type"
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
                 'drive_label', NULLIF(substr(coalesce(json_extract(rich_data, '$.drive_label'), ''), 1, {LIST_RICH_AUX_LIMIT}), '')
             )
         END,
         file_data, is_favorite, substr(note, 1, {LIST_NOTE_LIMIT}), source_app_name, '', image_width, image_height, size, meta_type"
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
        let changed = self.conn.execute(
            "UPDATE clipboard_items SET updated_at = ?1, image_path = ?3, rich_data = ?4, file_data = ?5, image_width = ?6, image_height = ?7, size = ?8, meta_type = ?9 WHERE content_hash = ?2",
            params![item.updated_at.to_rfc3339(), item.content_hash as i64, item.image_path, item.rich_data, item.file_data, item.image_width as i64, item.image_height as i64, item.size, item.meta_type],
        )?;
        if changed == 0 {
            self.conn.execute(
                "INSERT INTO clipboard_items (content_type, full_text, content_hash, created_at, updated_at, image_path, rich_data, file_data, source_app_name, source_app_icon, image_width, image_height, size, meta_type)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
        self.conn
            .execute("DELETE FROM clipboard_items WHERE id = ?1", params![id])?;
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
    pub fn get_all_sync_items_with_tags(&self) -> SqlResult<Vec<ClipboardItem>> {
        let query = format!(
            "SELECT {}
             FROM clipboard_items
             WHERE content_type NOT IN ('image', 'file')
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

        self.conn.execute(
            "INSERT INTO clipboard_items (content_type, full_text, content_hash, created_at, updated_at,
             rich_data, is_favorite, note, source_app_name, size, meta_type)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Update an item's fields from a newer remote version (sync merge).
    pub fn update_sync_item(&self, id: i64, item: &crate::core::sync::SyncItem) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE clipboard_items SET full_text = ?1, content_type = ?2, updated_at = ?3,
             rich_data = ?4, is_favorite = ?5, note = ?6, size = ?7, meta_type = ?8
             WHERE id = ?9",
            rusqlite::params![
                item.full_text,
                item.content_type,
                item.updated_at,
                item.rich_data,
                item.is_favorite as i32,
                item.note,
                item.size,
                item.meta_type,
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
        let all_ids: Vec<i64> = stmt
            .query_map([], |row| row.get(0))?
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
        let items_added = self.conn.execute(
            "INSERT INTO main.clipboard_items
             (content_type, full_text, content_hash, created_at, updated_at,
              image_path, rich_data, file_data, is_favorite, note,
              source_app_name, source_app_icon, image_width, image_height, size, meta_type)
             SELECT content_type, full_text, content_hash, created_at, updated_at,
                    image_path, rich_data, file_data, is_favorite, note,
                    source_app_name, source_app_icon, image_width, image_height, size,
                    COALESCE(meta_type, '')
             FROM source.clipboard_items s
             WHERE s.content_hash NOT IN (
                 SELECT content_hash FROM main.clipboard_items
             )",
            params![],
        )?;

        // ── 2. Items: update existing rows when source has a newer updated_at ──
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
                 meta_type      = (SELECT COALESCE(s.meta_type, '') FROM source.clipboard_items s WHERE s.content_hash = main.clipboard_items.content_hash)
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(name: &str) -> (std::path::PathBuf, Database) {
        let dir = std::env::temp_dir().join(format!(
            "clippi-merge-test-{}-{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
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
}
