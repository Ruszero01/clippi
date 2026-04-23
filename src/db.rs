use rusqlite::{params, Connection, Result as SqlResult};

use crate::types::{ClipboardItem, ContentType};

const DEFAULT_DB_PATH: &str = "clippi.db";

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open() -> SqlResult<Self> {
        let conn = Connection::open(DEFAULT_DB_PATH)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> SqlResult<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS clipboard_items (
                id INTEGER PRIMARY KEY,
                content_type TEXT NOT NULL DEFAULT 'text',
                text_preview TEXT NOT NULL,
                full_text TEXT NOT NULL,
                content_hash INTEGER NOT NULL,
                captured_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_hash ON clipboard_items(content_hash);
            CREATE INDEX IF NOT EXISTS idx_captured ON clipboard_items(captured_at DESC);
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )
    }

    pub fn insert(&self, item: &ClipboardItem) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO clipboard_items (id, content_type, text_preview, full_text, content_hash, captured_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                item.id,
                item.content_type.as_str(),
                item.text_preview,
                item.full_text,
                item.content_hash as i64,
                item.captured_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn upsert(&self, item: &ClipboardItem) -> SqlResult<()> {
        let changed = self.conn.execute(
            "UPDATE clipboard_items SET captured_at = ?1, id = ?2 WHERE content_hash = ?3",
            params![
                item.captured_at.to_rfc3339(),
                item.id,
                item.content_hash as i64,
            ],
        )?;
        if changed == 0 {
            self.insert(item)?;
        }
        Ok(())
    }

    pub fn load_recent(&self, limit: usize) -> SqlResult<Vec<ClipboardItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content_type, text_preview, full_text, content_hash, captured_at
             FROM clipboard_items ORDER BY captured_at DESC LIMIT ?1",
        )?;
        let items = stmt.query_map(params![limit], |row| {
            let ct_str: String = row.get(1)?;
            let at_str: String = row.get(5)?;
            Ok(ClipboardItem {
                id: row.get(0)?,
                content_type: ContentType::from_str(&ct_str),
                text_preview: row.get(2)?,
                full_text: row.get(3)?,
                content_hash: row.get::<_, i64>(4)? as u64,
                captured_at: at_str.parse().unwrap_or_default(),
            })
        })?;
        items.collect()
    }

    pub fn clear(&self) -> SqlResult<()> {
        self.conn.execute("DELETE FROM clipboard_items", [])?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> SqlResult<Option<String>> {
        self.conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |row| row.get(0),
        ).ok().map_or(Ok(None), |v| Ok(Some(v)))
    }

    pub fn set_setting(&self, key: &str, value: &str) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }
}
