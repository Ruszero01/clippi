//! Persist usage timestamps without blocking the UI on SQLite writers or I/O.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::Duration;

struct Job {
    path: String,
    items: Vec<(i64, i64)>,
    timestamp: String,
    mark_dirty: bool,
    sync_dirty: Arc<AtomicBool>,
    needs_reload: Arc<AtomicBool>,
}

enum Message {
    Touch(Job),
    Flush(mpsc::Sender<bool>),
}

fn sender() -> &'static mpsc::Sender<Message> {
    static SENDER: OnceLock<mpsc::Sender<Message>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Message>();
        std::thread::spawn(move || {
            let mut failed = false;
            for message in rx {
                let job = match message {
                    Message::Touch(job) => job,
                    Message::Flush(done) => {
                        if done.send(!failed).is_ok() {
                            failed = false;
                        }
                        continue;
                    }
                };
                match retry_locked(|| persist(&job.path, &job.items, &job.timestamp)) {
                    Ok(affected) => {
                        // A reload may have observed the database while this
                        // job was queued. Reconcile after every completion.
                        job.needs_reload.store(true, Ordering::SeqCst);
                        if affected > 0 && job.mark_dirty {
                            job.sync_dirty.store(true, Ordering::SeqCst);
                        }
                    }
                    Err(error) => {
                        log::error!("background usage update failed: {error}");
                        failed = true;
                        job.needs_reload.store(true, Ordering::SeqCst);
                    }
                }
            }
        });
        tx
    })
}

fn retry_locked<T>(mut operation: impl FnMut() -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    loop {
        match operation() {
            Err(rusqlite::Error::SqliteFailure(error, _))
                if matches!(
                    error.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                std::thread::sleep(Duration::from_millis(100));
            }
            result => return result,
        }
    }
}

/// Wait for previously queued writes, without hanging shutdown on a locked DB.
pub fn flush() -> bool {
    flush_timeout(Duration::from_secs(3))
}

fn flush_timeout(timeout: Duration) -> bool {
    let (done, completed) = mpsc::channel();
    if sender().send(Message::Flush(done)).is_err() {
        log::error!("background usage update worker stopped before flush");
        return false;
    }
    match completed.recv_timeout(timeout) {
        Ok(true) => true,
        Ok(false) => {
            log::error!("a background usage update failed before flush");
            false
        }
        Err(error) => {
            log::error!("background usage update flush did not complete: {error}");
            false
        }
    }
}

/// FIFO serialization keeps consecutive touches in order. A content hash and
/// timestamp guard prevent delayed jobs from reverting a changed or newer row.
fn persist(path: &str, items: &[(i64, i64)], timestamp: &str) -> rusqlite::Result<usize> {
    let mut db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    db.busy_timeout(Duration::from_secs(5))?;
    let tx = db.transaction()?;
    let mut affected = 0;
    {
        let mut update = tx.prepare_cached(
            "UPDATE clipboard_items SET updated_at = ?1 WHERE id = ?2 AND content_hash = ?3 \
             AND (julianday(updated_at) IS NULL OR julianday(updated_at) < julianday(?1))",
        )?;
        for &(id, hash) in items {
            affected += update.execute(rusqlite::params![timestamp, id, hash])?;
        }
    }
    tx.commit()?;
    Ok(affected)
}

pub fn enqueue(
    path: String,
    items: Vec<(i64, i64)>,
    timestamp: String,
    mark_dirty: bool,
    sync_dirty: Arc<AtomicBool>,
    needs_reload: Arc<AtomicBool>,
) -> bool {
    sender()
        .send(Message::Touch(Job {
            path,
            items,
            timestamp,
            mark_dirty,
            sync_dirty,
            needs_reload,
        }))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn locked_writer_does_not_block_enqueue_and_eventually_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE clipboard_items (id INTEGER PRIMARY KEY, content_hash INTEGER, updated_at TEXT); INSERT INTO clipboard_items VALUES (1, 42, '2020-01-01T00:00:00+00:00'); BEGIN IMMEDIATE;").unwrap();
        let dirty = Arc::new(AtomicBool::new(false));
        let reload = Arc::new(AtomicBool::new(false));
        let start = Instant::now();
        assert!(enqueue(
            path.to_string_lossy().into_owned(),
            vec![(1, 42)],
            "2026-01-01T00:00:00+00:00".into(),
            true,
            dirty.clone(),
            reload.clone()
        ));
        assert!(start.elapsed() < Duration::from_millis(250));
        db.execute_batch("COMMIT").unwrap();
        flush();
        let deadline = Instant::now() + Duration::from_secs(4);
        while !dirty.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(dirty.load(Ordering::SeqCst));
        assert!(reload.load(Ordering::SeqCst));
        assert_eq!(
            db.query_row(
                "SELECT updated_at FROM clipboard_items WHERE id=1",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "2026-01-01T00:00:00+00:00"
        );
        assert_eq!(
            persist(
                &path.to_string_lossy(),
                &[(1, 42)],
                "2025-01-01T00:00:00+00:00"
            )
            .unwrap(),
            0
        );
        assert_eq!(
            persist(
                &path.to_string_lossy(),
                &[(1, 99)],
                "2027-01-01T00:00:00+00:00"
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn transient_database_locks_are_retried() {
        let mut attempts = 0;
        let result = retry_locked(|| {
            attempts += 1;
            if attempts < 3 {
                Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                    None,
                ))
            } else {
                Ok(42)
            }
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts, 3);
    }

    #[test]
    fn flush_waits_for_pending_locked_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flush.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE clipboard_items (id INTEGER PRIMARY KEY, content_hash INTEGER, updated_at TEXT); INSERT INTO clipboard_items VALUES (1, 42, '2020-01-01T00:00:00Z'); BEGIN IMMEDIATE;").unwrap();
        assert!(enqueue(
            path.to_string_lossy().into_owned(),
            vec![(1, 42)],
            "2026-01-01T00:00:00Z".into(),
            false,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        ));
        let (done, completed) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            flush();
            done.send(()).unwrap();
        });
        assert!(completed.recv_timeout(Duration::from_millis(100)).is_err());
        db.execute_batch("COMMIT").unwrap();
        completed.recv_timeout(Duration::from_secs(4)).unwrap();
        thread.join().unwrap();
        let timestamp: String = db
            .query_row(
                "SELECT updated_at FROM clipboard_items WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(timestamp, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn flush_timeout_allows_exit_while_worker_keeps_retrying() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE clipboard_items (id INTEGER PRIMARY KEY, content_hash INTEGER, updated_at TEXT); INSERT INTO clipboard_items VALUES (1, 42, '2020-01-01T00:00:00Z'); BEGIN IMMEDIATE;").unwrap();
        assert!(enqueue(
            path.to_string_lossy().into_owned(),
            vec![(1, 42)],
            "2026-01-01T00:00:00Z".into(),
            false,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        ));
        let start = Instant::now();
        assert!(!flush_timeout(Duration::from_millis(100)));
        assert!(start.elapsed() < Duration::from_secs(2));
        db.execute_batch("COMMIT").unwrap();
        let _ = flush();
        let timestamp: String = db
            .query_row(
                "SELECT updated_at FROM clipboard_items WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(timestamp, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn migration_copy_after_flush_includes_queued_usage() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.db");
        let target = dir.path().join("target.db");
        let db = crate::core::db::Database::open(source.to_str().unwrap()).unwrap();
        // Build a real history row through the database's normal schema.
        let conn = rusqlite::Connection::open(&source).unwrap();
        conn.execute("INSERT INTO clipboard_items (content_type, full_text, content_hash, created_at, updated_at) VALUES ('plain_text', 'example', 42, '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z')", []).unwrap();
        let id = conn.last_insert_rowid();
        assert!(enqueue(
            source.to_string_lossy().into_owned(),
            vec![(id, 42)],
            "2026-01-01T00:00:00Z".into(),
            false,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false))
        ));
        let _ = flush();
        db.checkpoint_for_migration().unwrap();
        crate::core::settings::migrate_database(&source, &target).unwrap();
        let copy = rusqlite::Connection::open(target).unwrap();
        let timestamp: String = copy
            .query_row(
                "SELECT updated_at FROM clipboard_items WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(timestamp, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn failed_write_requests_reconciliation_without_marking_sync_dirty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.db");
        let dirty = Arc::new(AtomicBool::new(false));
        let reload = Arc::new(AtomicBool::new(false));
        assert!(enqueue(
            path.to_string_lossy().into_owned(),
            vec![(1, 42)],
            "2026-01-01T00:00:00+00:00".into(),
            true,
            dirty.clone(),
            reload.clone()
        ));
        let deadline = Instant::now() + Duration::from_secs(4);
        while !reload.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(reload.load(Ordering::SeqCst));
        assert!(!dirty.load(Ordering::SeqCst));
        assert!(!path.exists());
    }
}
