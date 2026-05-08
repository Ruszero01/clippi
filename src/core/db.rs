//! Database persistence for clipboard items

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
                text_preview TEXT NOT NULL,
                full_text TEXT NOT NULL,
                content_hash INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_hash ON clipboard_items(content_hash);
            CREATE INDEX IF NOT EXISTS idx_updated ON clipboard_items(updated_at DESC);
            "
        )?;

        // Migration: add image_path column if missing
        let has_image_path: bool = self.conn
            .prepare("SELECT image_path FROM clipboard_items LIMIT 0")
            .is_ok();
        if !has_image_path {
            self.conn.execute(
                "ALTER TABLE clipboard_items ADD COLUMN image_path TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }

        Ok(())
    }

    pub fn upsert(&self, item: &ClipboardItem) -> SqlResult<()> {
        let changed = self.conn.execute(
            "UPDATE clipboard_items SET updated_at = ?1, image_path = ?3 WHERE content_hash = ?2",
            params![item.updated_at.to_rfc3339(), item.content_hash as i64, item.image_path],
        )?;
        if changed == 0 {
            self.conn.execute(
                "INSERT INTO clipboard_items (content_type, text_preview, full_text, content_hash, created_at, updated_at, image_path)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    item.content_type.as_str(),
                    item.text_preview,
                    item.full_text,
                    item.content_hash as i64,
                    item.created_at.to_rfc3339(),
                    item.updated_at.to_rfc3339(),
                    item.image_path,
                ],
            )?;
        }
        Ok(())
    }

    pub fn load_by_updated(&self, limit: usize) -> SqlResult<Vec<ClipboardItem>> {
        self.load_items(limit, "updated_at")
    }

    pub fn load_by_created(&self, limit: usize) -> SqlResult<Vec<ClipboardItem>> {
        self.load_items(limit, "created_at")
    }

    pub fn load_by_type(&self, content_type: &str, limit: usize, order_by: &str) -> SqlResult<Vec<ClipboardItem>> {
        let query = format!(
            "SELECT id, content_type, text_preview, full_text, content_hash, created_at, updated_at, image_path
             FROM clipboard_items WHERE content_type = ?1 ORDER BY {} DESC LIMIT ?2",
            order_by
        );
        let mut stmt = self.conn.prepare(&query)?;
        let items = stmt.query_map(params![content_type, limit as i64], |row| {
            let ct_str: String = row.get(1)?;
            let created_str: String = row.get(5)?;
            let updated_str: String = row.get(6)?;
            let image_path: String = row.get(7).unwrap_or_default();
            Ok(ClipboardItem {
                id: row.get(0)?,
                content_type: ContentType::from_str(&ct_str),
                text_preview: row.get(2)?,
                full_text: row.get(3)?,
                content_hash: row.get::<_, i64>(4)? as u64,
                created_at: created_str.parse().unwrap_or_default(),
                updated_at: updated_str.parse().unwrap_or_default(),
                image_path,
            })
        })?;
        items.collect()
    }

    fn load_items(&self, limit: usize, order_by: &str) -> SqlResult<Vec<ClipboardItem>> {
        let query = format!(
            "SELECT id, content_type, text_preview, full_text, content_hash, created_at, updated_at, image_path
             FROM clipboard_items ORDER BY {} DESC LIMIT ?1",
            order_by
        );
        let mut stmt = self.conn.prepare(&query)?;
        let items = stmt.query_map(params![limit as i64], |row| {
            let ct_str: String = row.get(1)?;
            let created_str: String = row.get(5)?;
            let updated_str: String = row.get(6)?;
            let image_path: String = row.get(7).unwrap_or_default();
            Ok(ClipboardItem {
                id: row.get(0)?,
                content_type: ContentType::from_str(&ct_str),
                text_preview: row.get(2)?,
                full_text: row.get(3)?,
                content_hash: row.get::<_, i64>(4)? as u64,
                created_at: created_str.parse().unwrap_or_default(),
                updated_at: updated_str.parse().unwrap_or_default(),
                image_path,
            })
        })?;
        items.collect()
    }

    pub fn get_by_id(&self, id: i64) -> SqlResult<Option<ClipboardItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content_type, text_preview, full_text, content_hash, created_at, updated_at, image_path
             FROM clipboard_items WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            let ct_str: String = row.get(1)?;
            let created_str: String = row.get(5)?;
            let updated_str: String = row.get(6)?;
            let image_path: String = row.get(7).unwrap_or_default();
            Ok(Some(ClipboardItem {
                id: row.get(0)?,
                content_type: ContentType::from_str(&ct_str),
                text_preview: row.get(2)?,
                full_text: row.get(3)?,
                content_hash: row.get::<_, i64>(4)? as u64,
                created_at: created_str.parse().unwrap_or_default(),
                updated_at: updated_str.parse().unwrap_or_default(),
                image_path,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn get_by_hash(&self, hash: u64) -> SqlResult<Option<ClipboardItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content_type, text_preview, full_text, content_hash, created_at, updated_at, image_path
             FROM clipboard_items WHERE content_hash = ?1",
        )?;
        let mut rows = stmt.query(params![hash as i64])?;
        if let Some(row) = rows.next()? {
            let ct_str: String = row.get(1)?;
            let created_str: String = row.get(5)?;
            let updated_str: String = row.get(6)?;
            let image_path: String = row.get(7).unwrap_or_default();
            Ok(Some(ClipboardItem {
                id: row.get(0)?,
                content_type: ContentType::from_str(&ct_str),
                text_preview: row.get(2)?,
                full_text: row.get(3)?,
                content_hash: row.get::<_, i64>(4)? as u64,
                created_at: created_str.parse().unwrap_or_default(),
                updated_at: updated_str.parse().unwrap_or_default(),
                image_path,
            }))
        } else {
            Ok(None)
        }
    }
}