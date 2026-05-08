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
                searchable_text TEXT NOT NULL DEFAULT '',
                content_hash INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                image_path TEXT NOT NULL DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_hash ON clipboard_items(content_hash);
            CREATE INDEX IF NOT EXISTS idx_updated ON clipboard_items(updated_at DESC);",
        )
    }

    pub fn upsert(&self, item: &ClipboardItem) -> SqlResult<()> {
        let changed = self.conn.execute(
            "UPDATE clipboard_items SET updated_at = ?1, image_path = ?3, searchable_text = ?4 WHERE content_hash = ?2",
            params![item.updated_at.to_rfc3339(), item.content_hash as i64, item.image_path, item.searchable_text],
        )?;
        if changed == 0 {
            self.conn.execute(
                "INSERT INTO clipboard_items (content_type, full_text, searchable_text, content_hash, created_at, updated_at, image_path)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    item.content_type.as_str(),
                    item.full_text,
                    item.searchable_text,
                    item.content_hash as i64,
                    item.created_at.to_rfc3339(),
                    item.updated_at.to_rfc3339(),
                    item.image_path,
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
            "SELECT id, content_type, full_text, searchable_text, content_hash, created_at, updated_at, image_path
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
            "SELECT id, content_type, full_text, searchable_text, content_hash, created_at, updated_at, image_path
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
            "SELECT id, content_type, full_text, searchable_text, content_hash, created_at, updated_at, image_path
             FROM clipboard_items WHERE content_hash = ?1",
        )?;
        let mut rows = stmt.query(params![hash as i64])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_item(row)?))
        } else {
            Ok(None)
        }
    }
}

fn row_to_item(row: &rusqlite::Row<'_>) -> SqlResult<ClipboardItem> {
    let ct_str: String = row.get(1)?;
    let created_str: String = row.get(5)?;
    let updated_str: String = row.get(6)?;
    let image_path: String = row.get(7).unwrap_or_default();
    Ok(ClipboardItem {
        id: row.get(0)?,
        content_type: ContentType::from_str(&ct_str),
        full_text: row.get(2)?,
        searchable_text: row.get(3)?,
        content_hash: row.get::<_, i64>(4)? as u64,
        created_at: created_str.parse().unwrap_or_default(),
        updated_at: updated_str.parse().unwrap_or_default(),
        image_path,
    })
}
