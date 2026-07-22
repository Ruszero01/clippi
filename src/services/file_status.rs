use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const STATUS_TTL: Duration = Duration::from_secs(30);
const MAX_CACHE_ENTRIES: usize = 2048;
const MAX_CONCURRENT_PROBES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachedPathKind {
    File,
    Directory,
    Missing,
}

#[derive(Clone, Copy)]
struct CacheEntry {
    kind: CachedPathKind,
    checked_at: Instant,
}

#[derive(Default)]
struct FileStatusCache {
    entries: HashMap<String, CacheEntry>,
    in_flight: HashSet<String>,
}

static CACHE: LazyLock<Mutex<FileStatusCache>> =
    LazyLock::new(|| Mutex::new(FileStatusCache::default()));
static STATUS_CHANGED: AtomicBool = AtomicBool::new(false);

/// Return a cached path kind without touching the source path on the calling
/// thread. A missing or expired entry schedules a bounded background probe and
/// returns the previous value (or None while initially unknown).
pub fn cached_path_kind(path: &str) -> Option<CachedPathKind> {
    if path.is_empty() {
        return Some(CachedPathKind::Missing);
    }

    let now = Instant::now();
    let path = path.to_string();
    let mut cache = CACHE.lock().unwrap_or_else(|error| error.into_inner());
    let cached = cache.entries.get(&path).copied();
    if cached.is_some_and(|entry| now.duration_since(entry.checked_at) < STATUS_TTL) {
        return cached.map(|entry| entry.kind);
    }
    if cache.in_flight.contains(&path) || cache.in_flight.len() >= MAX_CONCURRENT_PROBES {
        return cached.map(|entry| entry.kind);
    }
    cache.in_flight.insert(path.clone());
    drop(cache);

    std::thread::spawn(move || {
        let kind = match std::fs::metadata(Path::new(&path)) {
            Ok(metadata) if metadata.is_file() => CachedPathKind::File,
            Ok(_) => CachedPathKind::Directory,
            Err(_) => CachedPathKind::Missing,
        };
        let mut cache = CACHE.lock().unwrap_or_else(|error| error.into_inner());
        if cache.entries.len() >= MAX_CACHE_ENTRIES && !cache.entries.contains_key(&path) {
            if let Some(oldest) = cache
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.checked_at)
                .map(|(path, _)| path.clone())
            {
                cache.entries.remove(&oldest);
            }
        }
        cache.entries.insert(
            path.clone(),
            CacheEntry {
                kind,
                checked_at: Instant::now(),
            },
        );
        cache.in_flight.remove(&path);
        STATUS_CHANGED.store(true, Ordering::Release);
    });

    cached.map(|entry| entry.kind)
}

/// Return whether a path exists, preserving the existing card-status API.
pub fn cached_file_exists(path: &str) -> Option<bool> {
    cached_path_kind(path).map(|kind| kind != CachedPathKind::Missing)
}
/// Called by the unified WindowManager poll loop to request a lightweight
/// repaint after one or more background probes complete.
pub fn take_status_changed() -> bool {
    STATUS_CHANGED.swap(false, Ordering::AcqRel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_lookup_is_non_blocking_and_later_returns_cached_status() {
        let path = std::env::temp_dir().join(format!(
            "clippi-file-status-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"test").unwrap();
        let path = path.to_string_lossy().into_owned();

        assert_eq!(cached_file_exists(&path), None);
        let deadline = Instant::now() + Duration::from_secs(2);
        while cached_file_exists(&path).is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(cached_file_exists(&path), Some(true));
        assert_eq!(cached_path_kind(&path), Some(CachedPathKind::File));

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn cached_path_kind_distinguishes_directories() {
        let path = std::env::temp_dir().join(format!(
            "clippi-directory-status-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let path = path.to_string_lossy().into_owned();

        assert_eq!(cached_path_kind(&path), None);
        let deadline = Instant::now() + Duration::from_secs(2);
        while cached_path_kind(&path).is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(cached_path_kind(&path), Some(CachedPathKind::Directory));

        std::fs::remove_dir(path).unwrap();
    }
}
