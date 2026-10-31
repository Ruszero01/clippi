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

fn sender() -> &'static mpsc::Sender<Job> {
    static SENDER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::spawn(move || {
            for job in rx {
                match persist(&job.path, &job.items, &job.timestamp) {
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
                        job.needs_reload.store(true, Ordering::SeqCst);
                    }
                }
            }
        });
        tx
    })
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
        .send(Job {
            path,
            items,
            timestamp,
            mark_dirty,
            sync_dirty,
            needs_reload,
        })
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
