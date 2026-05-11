//! Database persistence for clipboard items

use crate::core::filters::ClipboardFilters;
use crate::core::types::{ClipboardItem, ContentType};
use rusqlite::{params, Connection, Result as SqlResult};

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &str) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
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
                is_favorite INTEGER NOT NULL DEFAULT 0,
                source_app_name TEXT NOT NULL DEFAULT '',
                source_app_icon TEXT NOT NULL DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_hash ON clipboard_items(content_hash);
            CREATE INDEX IF NOT EXISTS idx_updated ON clipboard_items(updated_at DESC);",
        )?;
        // Add columns to existing databases (ignore error if already present)
        let _ = self.conn.execute("ALTER TABLE clipboard_items ADD COLUMN source_app_name TEXT NOT NULL DEFAULT ''", []);
        let _ = self.conn.execute("ALTER TABLE clipboard_items ADD COLUMN source_app_icon TEXT NOT NULL DEFAULT ''", []);
        let _ = self.conn.execute("ALTER TABLE clipboard_items ADD COLUMN rich_data TEXT NOT NULL DEFAULT ''", []);
        let _ = self.conn.execute("ALTER TABLE clipboard_items ADD COLUMN note TEXT NOT NULL DEFAULT ''", []);
        Ok(())
    }

    pub fn upsert(&self, item: &ClipboardItem) -> SqlResult<()> {
        let changed = self.conn.execute(
            "UPDATE clipboard_items SET updated_at = ?1, image_path = ?3, rich_data = ?4 WHERE content_hash = ?2",
            params![item.updated_at.to_rfc3339(), item.content_hash as i64, item.image_path, item.rich_data],
        )?;
        if changed == 0 {
            self.conn.execute(
                "INSERT INTO clipboard_items (content_type, full_text, content_hash, created_at, updated_at, image_path, rich_data, source_app_name, source_app_icon)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    item.content_type.as_str(),
                    item.full_text,
                    item.content_hash as i64,
                    item.created_at.to_rfc3339(),
                    item.updated_at.to_rfc3339(),
                    item.image_path,
                    item.rich_data,
                    item.source_app_name,
                    item.source_app_icon,
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
            "SELECT id, content_type, full_text, content_hash, created_at, updated_at, image_path, rich_data, is_favorite, note, source_app_name, source_app_icon
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
            "SELECT id, content_type, full_text, content_hash, created_at, updated_at, image_path, rich_data, is_favorite, note, source_app_name, source_app_icon
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
            "SELECT id, content_type, full_text, content_hash, created_at, updated_at, image_path, rich_data, is_favorite, note, source_app_name, source_app_icon
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
        self.conn.execute(
            "UPDATE clipboard_items SET is_favorite = CASE WHEN is_favorite = 0 THEN 1 ELSE 0 END WHERE id = ?1",
            params![id],
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
}

fn row_to_item(row: &rusqlite::Row<'_>) -> SqlResult<ClipboardItem> {
    let ct_str: String = row.get(1)?;
    let created_str: String = row.get(4)?;
    let updated_str: String = row.get(5)?;
    let image_path: String = row.get(6).unwrap_or_default();
    let rich_data: String = row.get(7).unwrap_or_default();
    let is_favorite: i32 = row.get(8).unwrap_or(0);
    let note: String = row.get(9).unwrap_or_default();
    let source_app_name: String = row.get(10).unwrap_or_default();
    let source_app_icon: String = row.get(11).unwrap_or_default();
    Ok(ClipboardItem {
        id: row.get(0)?,
        content_type: ContentType::from_str(&ct_str),
        full_text: row.get(2)?,
        content_hash: row.get::<_, i64>(3)? as u64,
        created_at: created_str.parse().unwrap_or_default(),
        updated_at: updated_str.parse().unwrap_or_default(),
        image_path,
        rich_data,
        is_favorite: is_favorite != 0,
        note,
        source_app_name,
        source_app_icon,
    })
}
