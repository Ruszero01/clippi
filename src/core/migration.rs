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
//! 1. Bump `SYNC_VERSION`
//! 2. Add a `SyncPayload` migration step in `migrate_sync_payload` that
//!    transforms from the old format to the current one

use rusqlite::Connection;

/// Current database schema version — derived from migration count (versions are 1..=N).
#[allow(dead_code)]
pub const DB_VERSION: i64 = DB_MIGRATIONS.len() as i64;

/// Current sync protocol version — written into every `SyncPayload` snapshot.
pub const SYNC_VERSION: u32 = 1;

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
const DB_MIGRATIONS: &[DbMigration] = &[DbMigration {
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
}];

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
            log::info!("[db] migration v{} — {}", migration.version, migration.description);
            if !migration.sql.is_empty() {
                conn.execute_batch(migration.sql)?;
            }
            conn.pragma_update(None, "user_version", migration.version)?;
        }
    }

    Ok(())
}

/// Migrate an older sync payload to the current protocol version.
///
/// Currently a no-op (only v1 exists). When `SYNC_VERSION` is bumped, add
/// transform logic here to upgrade payloads from older versions.
pub fn migrate_sync_payload(payload: &mut crate::core::sync::SyncPayload) {
    // Example for future v2 migration:
    //
    // if payload.version < 2 {
    //     // Transform v1 → v2 fields
    //     payload.version = 2;
    // }
    //
    // Always set version to current after migration.
    let _ = payload; // Keep unused-var warning silent until first migration is added
}
