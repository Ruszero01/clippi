//! Unified version tracking and migration framework.
//!
//! Manages both database schema version (`PRAGMA user_version`) and sync
//! protocol version (`SyncPayload.version`). The two evolve independently
//! with one constraint: DB schema must support all fields that the current
//! sync protocol carries.
//!
//! ## Adding a new database migration
//!
//! 1. Add an entry to `DB_MIGRATIONS` with the next sequential version number
//! 2. Write the SQL (or leave `sql` empty and add custom logic in
//!    `run_db_migrations` for complex data transforms)
//! 3. Update `DB_VERSION` to match the new highest version
//!
//! ## Adding a new sync protocol version
//!
//! --- 1. Bump `SYNC_VERSION` ---
//! --- 2. Add a `SyncPayload` migration step in `migrate_sync_payload` that ---
//! --- transforms from the old format to the current one ---

use rusqlite::{params, Connection};
use uuid::Uuid;

/// Current database schema version — derived from migration count (versions are 1..=N).
#[allow(dead_code)]
pub const DB_VERSION: i64 = DB_MIGRATIONS.len() as i64;

/// Current sync protocol version — written into every `SyncPayload` snapshot.
pub const SYNC_VERSION: u32 = 5;

/// Current transfer station protocol version — written into every `FileManifest`.
pub const TRANSFER_PROTOCOL_VERSION: u32 = 2;

/// A registered database migration.
struct DbMigration {
    /// Sequential version number (1-based, matches `PRAGMA user_version` after applied).
    version: i64,
    /// Human-readable description.
    description: &'static str,
    /// SQL to execute (can be multiple statements separated by `;`).
    /// Empty string means no SQL — custom logic is handled in `run_db_migrations`.
    sql: &'static str,
}

/// Migration registry — ordered by `version` ascending.
///
/// Each migration is applied exactly once. To add a new migration, append an
/// entry with the next version number and bump `DB_VERSION`.
const DB_MIGRATIONS: &[DbMigration] = &[
    DbMigration {
        version: 1,
        description: "Add UNIQUE indexes to tombstone tables to prevent unbounded growth",
        sql: concat!(
            "DELETE FROM deleted_items WHERE rowid NOT IN (SELECT MIN(rowid) FROM deleted_items GROUP BY content_hash);",
            "DELETE FROM deleted_tags WHERE rowid NOT IN (SELECT MIN(rowid) FROM deleted_tags GROUP BY name);",
            "DELETE FROM unfavorited_items WHERE rowid NOT IN (SELECT MIN(rowid) FROM unfavorited_items GROUP BY content_hash);",
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_del_items_hash_uq ON deleted_items(content_hash);",
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_del_tags_name_uq ON deleted_tags(name);",
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_uf_items_hash_uq ON unfavorited_items(content_hash);",
        ),
    },
    DbMigration {
        version: 2,
        description: "Add meta_type column for email/phone plain-text subtypes",
        sql: "ALTER TABLE clipboard_items ADD COLUMN meta_type TEXT NOT NULL DEFAULT ''",
    },
    DbMigration {
        version: 3,
        description: "Add indexes on content_type and is_favorite for filtered queries",
        sql: concat!(
            "CREATE INDEX IF NOT EXISTS idx_content_type ON clipboard_items(content_type);",
            "CREATE INDEX IF NOT EXISTS idx_is_favorite ON clipboard_items(is_favorite);",
        ),
    },
    DbMigration {
        version: 4,
        description: "Unify content_type: migrate link/path/color to plain_text with meta_type",
        sql: concat!(
            "UPDATE clipboard_items SET meta_type = 'link', content_type = 'plain_text' WHERE content_type = 'link';",
            "UPDATE clipboard_items SET meta_type = 'path', content_type = 'plain_text' WHERE content_type = 'path';",
            "UPDATE clipboard_items SET meta_type = 'color', content_type = 'plain_text' WHERE content_type = 'color';",
        ),
    },
    DbMigration {
        version: 5,
        description: "Add index on created_at for sort-by-created query performance",
        sql: "CREATE INDEX IF NOT EXISTS idx_created ON clipboard_items(created_at DESC)",
    },
    DbMigration {
        version: 6,
        description: "Add stable sync uid columns for tags and tag tombstones",
        sql: "",
    },
    DbMigration {
        version: 7,
        description: "Add custom_hotkey and custom_hotkey_format columns for per-item hotkeys",
        sql: "",
    },
];

/// Run all pending database migrations, updating `PRAGMA user_version`.
///
/// Called from `Database::init_schema()` on every startup. Migrations that
/// have already been applied (version <= current user_version) are skipped.
pub fn run_db_migrations(conn: &Connection) -> rusqlite::Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;

    debug_assert!(
        DB_MIGRATIONS
            .iter()
            .enumerate()
            .all(|(i, m)| m.version == (i + 1) as i64),
        "DB_MIGRATIONS versions must be sequential starting from 1"
    );

    for migration in DB_MIGRATIONS {
        if migration.version > current {
            log::info!(
                "[db] migration v{} — {}",
                migration.version,
                migration.description
            );
            if !migration.sql.is_empty() {
                conn.execute_batch(migration.sql)?;
            }
            if migration.version == 6 {
                migrate_tag_sync_uids(conn)?;
            }
            if migration.version == 7 {
                migrate_item_hotkey_columns(conn)?;
            }
            conn.pragma_update(None, "user_version", migration.version)?;
        }
    }

    repair_db_schema(conn)?;

    Ok(())
}

fn repair_db_schema(conn: &Connection) -> rusqlite::Result<()> {
    migrate_item_hotkey_columns(conn)
}

fn migrate_item_hotkey_columns(conn: &Connection) -> rusqlite::Result<()> {
    if !column_exists(conn, "clipboard_items", "custom_hotkey")? {
        conn.execute(
            "ALTER TABLE clipboard_items ADD COLUMN custom_hotkey TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    if !column_exists(conn, "clipboard_items", "custom_hotkey_format")? {
        conn.execute(
            "ALTER TABLE clipboard_items ADD COLUMN custom_hotkey_format TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    Ok(())
}

fn migrate_tag_sync_uids(conn: &Connection) -> rusqlite::Result<()> {
    if !column_exists(conn, "tags", "uid")? {
        conn.execute(
            "ALTER TABLE tags ADD COLUMN uid TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    if !column_exists(conn, "deleted_tags", "uid")? {
        conn.execute(
            "ALTER TABLE deleted_tags ADD COLUMN uid TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }

    let tags: Vec<(i64, String)> = {
        let mut stmt = conn.prepare("SELECT id, name FROM tags WHERE uid = ''")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (id, name) in tags {
        conn.execute(
            "UPDATE tags SET uid = ?1 WHERE id = ?2",
            params![legacy_tag_uid(&name), id],
        )?;
    }

    let deleted_tags: Vec<(i64, String)> = {
        let mut stmt = conn.prepare("SELECT rowid, name FROM deleted_tags WHERE uid = ''")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (rowid, name) in deleted_tags {
        conn.execute(
            "UPDATE deleted_tags SET uid = ?1 WHERE rowid = ?2",
            params![legacy_tag_uid(&name), rowid],
        )?;
    }

    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_tags_uid_uq ON tags(uid) WHERE uid != '';
         DROP INDEX IF EXISTS idx_del_tags_name_uq;
         CREATE INDEX IF NOT EXISTS idx_del_tags_uid ON deleted_tags(uid);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_del_tags_uid_uq ON deleted_tags(uid) WHERE uid != '';
         CREATE UNIQUE INDEX IF NOT EXISTS idx_del_tags_legacy_name_uq ON deleted_tags(name) WHERE uid = '';",
    )?;

    Ok(())
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in rows {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn legacy_tag_uid(name: &str) -> String {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("clippi-tag:{name}").as_bytes(),
    )
    .to_string()
}

/// Migrate an older sync payload to the current protocol version.
///
/// When `SYNC_VERSION` is bumped, add transform logic here to upgrade
/// payloads from older versions.
pub fn migrate_sync_payload(payload: &mut crate::core::sync::SyncPayload) {
    if payload.version < 2 {
        // --- v1 → v2: SyncItem.meta_type added with #[serde(default)] — no data ---
        // transform needed since missing field defaults to "" on deserialization.
        payload.version = 2;
    }
    if payload.version < 3 {
        // v2 → v3: normalize legacy content_type strings for link/path/color
        // to plain_text with meta_type set, matching the DB migration v4.
        for item in &mut payload.items {
            let (normalized_ct, normalized_meta) = match item.content_type.as_str() {
                "link" => ("plain_text", "link"),
                "path" => ("plain_text", "path"),
                "color" => ("plain_text", "color"),
                _ => continue,
            };
            item.content_type = normalized_ct.to_string();
            if item.meta_type.is_empty() {
                item.meta_type = normalized_meta.to_string();
            }
        }
        payload.version = 3;
    }
    if payload.version < 4 {
        // v3 → v4: tag uid fields were added with #[serde(default)].
        payload.version = 4;
    }
    if payload.version < 5 {
        // v4 -> v5: remove inline thumbnail data from sync items. Unknown
        // `thumb_data` fields from old JSON are ignored during deserialization;
        // rewriting the migrated payload cleans them from the sync file.
        payload.version = 5;
    }
}

/// Migrate an older file manifest to the current transfer protocol version.
///
/// When `TRANSFER_PROTOCOL_VERSION` is bumped, add transform logic here to
/// upgrade manifests from older versions. Pattern matches `migrate_sync_payload`.
///
/// Called every time a manifest is pulled from the backend, before any
/// processing. Unknown versions reset to an empty manifest as a safety fallback.
pub fn migrate_file_manifest(manifest: &mut crate::core::transfer_types::FileManifest) {
    if manifest.version > TRANSFER_PROTOCOL_VERSION {
        log::warn!(
            "[transfer] manifest version {} is newer than supported version {}",
            manifest.version,
            TRANSFER_PROTOCOL_VERSION
        );
        return;
    }
    if manifest.version < 1 {
        // Unknown/invalid version — safety reset to empty.
        log::warn!(
            "[transfer] unknown manifest version {}, resetting",
            manifest.version
        );
        manifest.version = TRANSFER_PROTOCOL_VERSION;
        manifest.files.clear();
    }
    if manifest.version < 2 {
        // v1 -> v2 moves local-folder mutations to an append-only operation
        // log. The materialized fields are unchanged, so no data transform is
        // required for WebDAV or for the legacy local-folder baseline.
        manifest.version = 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repairs_v7_database_missing_hotkey_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE clipboard_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                content_type TEXT NOT NULL,
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
                size INTEGER NOT NULL DEFAULT 0,
                meta_type TEXT NOT NULL DEFAULT ''
            );
            PRAGMA user_version = 7;",
        )
        .unwrap();

        run_db_migrations(&conn).unwrap();

        assert!(column_exists(&conn, "clipboard_items", "custom_hotkey").unwrap());
        assert!(column_exists(&conn, "clipboard_items", "custom_hotkey_format").unwrap());
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, DB_VERSION);
    }
}
