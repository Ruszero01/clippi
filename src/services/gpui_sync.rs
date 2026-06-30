//! --- GPUI sync service. ---
//!
//! --- Keeps blocking backend work off the GPUI thread and publishes compact ---
//! --- snapshots into `AppState` from the unified WindowManager poll loop. ---

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;

use crate::core::db::Database;
use crate::core::settings::{AppSettings, BackendConfig};
use crate::core::sync::{self, BackendStatus, MergeStats, SyncBackend};
use crate::services::backends::local_folder::LocalFolderBackend;
use crate::services::backends::webdav::WebDAVBackend;
use crate::state::app::AppState;
use crate::state::sync::{service_label, BackendStatus as UiBackendStatus, SyncState};

#[derive(Debug, Clone)]
struct BackendSyncResult {
    backend_id: String,
    cycle: SyncCycleResult,
}

#[derive(Debug, Clone)]
struct SyncCycleResult {
    success: bool,
    message: String,
    stats: MergeStats,
    snapshot_counts: Option<(u32, u32)>,
    did_push: bool,
}

struct BackendRuntime {
    config: BackendConfig,
    backend: Arc<dyn SyncBackend>,
    status: BackendStatus,
    status_message: String,
    last_sync: Option<Instant>,
    is_running: Arc<AtomicBool>,
    cancel_flag: Arc<AtomicBool>,
    pending_result: Arc<Mutex<Option<BackendSyncResult>>>,
    last_status_check: Option<Instant>,
}

#[derive(Default)]
pub struct SyncPollOutcome {
    pub state_changed: bool,
    pub data_changed: bool,
}

pub struct GpuiSyncService {
    db: Arc<Mutex<Database>>,
    dirty: Arc<AtomicBool>,
    backends: Vec<BackendRuntime>,
    auto_enabled: bool,
    favorites_only: bool,
    last_message: String,
}

impl GpuiSyncService {
    pub fn new(settings: &AppSettings, dirty: Arc<AtomicBool>) -> Self {
        let path = settings.resolve_db_path();
        let db = Database::open(&path.to_string_lossy())
            .unwrap_or_else(|e| panic!("Failed to open sync database at {path:?}: {e}"));
        let mut service = Self {
            db: Arc::new(Mutex::new(db)),
            dirty,
            backends: Vec::new(),
            auto_enabled: settings.sync_auto_enabled,
            favorites_only: settings.sync_favorites_only,
            last_message: String::new(),
        };
        service.reload_from_settings(settings);
        service
    }

    pub fn reload_from_settings(&mut self, settings: &AppSettings) {
        for runtime in &self.backends {
            runtime.cancel_flag.store(true, Ordering::SeqCst);
        }

        self.auto_enabled = settings.sync_auto_enabled;
        self.favorites_only = settings.sync_favorites_only;
        self.backends = settings
            .sync_backends
            .iter()
            .cloned()
            .map(Self::build_runtime)
            .collect();
    }

    pub fn trigger_backend_sync(&mut self, id: &str) {
        if let Some(index) = self
            .backends
            .iter()
            .position(|runtime| runtime.config.id == id && runtime.config.enabled)
        {
            self.start_sync_cycle(index, true);
        }
    }

    pub fn trigger_pull_all(&mut self) {
        let indices: Vec<usize> = self
            .backends
            .iter()
            .enumerate()
            .filter_map(|(index, runtime)| {
                (runtime.config.enabled && !runtime.is_running.load(Ordering::SeqCst))
                    .then_some(index)
            })
            .collect();
        for index in indices {
            self.start_sync_cycle(index, true);
        }
    }

    pub fn poll(&mut self, app: &mut AppState) -> SyncPollOutcome {
        let previous = app.sync.clone();
        let mut data_changed = false;

        let mut results = Vec::new();
        for runtime in &self.backends {
            if let Some(result) = runtime
                .pending_result
                .lock()
                .expect("sync result lock poisoned")
                .take()
            {
                results.push(result);
            }
        }
        for result in results {
            data_changed |= self.apply_result(result, app);
        }

        for runtime in &mut self.backends {
            if runtime.is_running.load(Ordering::Acquire) || runtime.config.backend_type == "webdav"
            {
                continue;
            }
            let due = runtime
                .last_status_check
                .map(|last| last.elapsed() >= Duration::from_secs(30))
                .unwrap_or(true);
            if due {
                runtime.status = runtime.backend.check_status();
                runtime.status_message = status_message(&runtime.status);
                runtime.last_status_check = Some(Instant::now());
            }
        }

        let dirty = self.dirty.load(Ordering::SeqCst);
        let due_indices: Vec<usize> = self
            .backends
            .iter()
            .enumerate()
            .filter_map(|(index, runtime)| self.should_sync(runtime, dirty).then_some(index))
            .collect();
        if !due_indices.is_empty() && dirty {
            self.dirty.store(false, Ordering::SeqCst);
        }
        for index in due_indices {
            self.start_sync_cycle(index, dirty);
        }

        app.sync = self.snapshot();
        if data_changed {
            app.reload_items();
            app.reload_tags();
        }

        SyncPollOutcome {
            state_changed: previous != app.sync,
            data_changed,
        }
    }

    fn build_runtime(config: BackendConfig) -> BackendRuntime {
        let backend: Arc<dyn SyncBackend> = match config.backend_type.as_str() {
            "webdav" => Arc::new(WebDAVBackend::new(config.clone())),
            _ => Arc::new(LocalFolderBackend::new(config.clone())),
        };
        let status = if config.backend_type == "webdav" {
            BackendStatus::Offline
        } else {
            backend.check_status()
        };
        BackendRuntime {
            config,
            status_message: status_message(&status),
            backend,
            status,
            last_sync: None,
            is_running: Arc::new(AtomicBool::new(false)),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            pending_result: Arc::new(Mutex::new(None)),
            last_status_check: None,
        }
    }

    fn should_sync(&self, runtime: &BackendRuntime, dirty: bool) -> bool {
        if !runtime.config.enabled
            || runtime.is_running.load(Ordering::SeqCst)
            || !self.auto_enabled
        {
            return false;
        }
        if dirty {
            return runtime
                .last_sync
                .map(|last| last.elapsed() >= Duration::from_secs(5))
                .unwrap_or(true);
        }
        runtime
            .last_sync
            .map(|last| {
                last.elapsed() >= Duration::from_secs(runtime.backend.sync_interval().max(1))
            })
            .unwrap_or(true)
    }

    fn start_sync_cycle(&mut self, index: usize, force_push: bool) {
        let runtime = &mut self.backends[index];
        if !runtime.config.enabled || runtime.is_running.swap(true, Ordering::SeqCst) {
            return;
        }

        runtime.cancel_flag.store(false, Ordering::SeqCst);
        runtime.last_sync = Some(Instant::now());
        runtime.status = BackendStatus::Online;
        runtime.status_message.clear();

        let db = Arc::clone(&self.db);
        let cancel = Arc::clone(&runtime.cancel_flag);
        let pending = Arc::clone(&runtime.pending_result);
        let running = Arc::clone(&runtime.is_running);
        let backend = Arc::clone(&runtime.backend);
        let backend_id = runtime.config.id.clone();
        let favorites_only = self.favorites_only;

        std::thread::spawn(move || {
            let cycle = run_sync_cycle_for_backend(
                backend.as_ref(),
                &db,
                &cancel,
                favorites_only,
                force_push,
            );
            *pending.lock().expect("sync result lock poisoned") =
                Some(BackendSyncResult { backend_id, cycle });
            running.store(false, Ordering::SeqCst);
        });
    }

    fn apply_result(&mut self, result: BackendSyncResult, app: &mut AppState) -> bool {
        let has_merge = result.cycle.stats.items_added > 0
            || result.cycle.stats.items_updated > 0
            || result.cycle.stats.items_deleted > 0
            || result.cycle.stats.tags_added > 0
            || result.cycle.stats.tags_deleted > 0;
        self.last_message = result.cycle.message.clone();

        let Some(runtime) = self
            .backends
            .iter_mut()
            .find(|runtime| runtime.config.id == result.backend_id)
        else {
            return has_merge;
        };

        if result.cycle.success {
            runtime.status = BackendStatus::Online;
            runtime.status_message.clear();
            update_backend_sync_metadata(&mut runtime.config, &result.cycle, has_merge);
        } else {
            runtime.status = BackendStatus::Error(result.cycle.message.clone());
            runtime.status_message = result.cycle.message;
        }

        if let Some(config) = app
            .settings
            .sync_backends
            .iter_mut()
            .find(|config| config.id == runtime.config.id)
        {
            config.last_sync_at = runtime.config.last_sync_at.clone();
            config.last_item_count = runtime.config.last_item_count;
            config.last_tag_count = runtime.config.last_tag_count;
            app.settings.save();
        }

        has_merge
    }

    fn snapshot(&self) -> SyncState {
        SyncState {
            backends: self
                .backends
                .iter()
                .map(|runtime| UiBackendStatus {
                    config: runtime.config.clone(),
                    status: if runtime.is_running.load(Ordering::Relaxed) {
                        "syncing".into()
                    } else {
                        match runtime.status {
                            BackendStatus::Online => "online",
                            BackendStatus::Offline => "offline",
                            BackendStatus::Error(_) => "error",
                        }
                        .into()
                    },
                    status_message: runtime.status_message.clone(),
                    syncing: runtime.is_running.load(Ordering::Relaxed),
                    service_label: service_label(&runtime.config),
                })
                .collect(),
            auto_enabled: self.auto_enabled,
            favorites_only: self.favorites_only,
            last_message: self.last_message.clone(),
        }
    }
}

fn update_backend_sync_metadata(
    config: &mut BackendConfig,
    cycle: &SyncCycleResult,
    has_merge: bool,
) {
    if let Some((items, tags)) = cycle.snapshot_counts {
        config.last_item_count = items;
        config.last_tag_count = tags;
    }
    if has_merge || cycle.did_push {
        config.last_sync_at = chrono::Utc::now().to_rfc3339();
    }
}

impl Drop for GpuiSyncService {
    fn drop(&mut self) {
        for runtime in &self.backends {
            runtime.cancel_flag.store(true, Ordering::SeqCst);
        }
    }
}

fn status_message(status: &BackendStatus) -> String {
    match status {
        BackendStatus::Online => String::new(),
        BackendStatus::Offline => "Unavailable".into(),
        BackendStatus::Error(error) => error.clone(),
    }
}

pub fn format_last_sync(rfc3339: &str) -> String {
    if rfc3339.is_empty() {
        return "Never synced".into();
    }
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|time| crate::core::types::format_relative_time(&time.to_utc()))
        .unwrap_or_else(|_| rfc3339.to_string())
}

pub fn test_webdav_connection(url: &str, username: &str, password: &str) -> bool {
    let raw = format!("{username}:{password}");
    let auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(raw)
    );
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(5))
        .build();
    crate::services::backends::webdav::check_webdav_connection(&agent, url, &auth).is_ok()
}

fn run_sync_cycle_for_backend(
    backend: &dyn SyncBackend,
    db: &Mutex<Database>,
    cancel: &AtomicBool,
    favorites_only: bool,
    force_push: bool,
) -> SyncCycleResult {
    let local_device = crate::services::backends::local_folder::hostname();
    let mut stats = MergeStats::default();
    let mut remote_hash = None;
    let mut remote_unchanged = false;

    match backend.pull(force_push) {
        Ok(mut remote) => {
            remote_hash = Some(sync::payload_semantic_hash(&remote));
            match sync::merge_remote_into_local(db, &mut remote, &local_device) {
                Ok(merge_stats) => {
                    stats = merge_stats;
                    let fetched = crate::services::url_assets::backfill_link_favicons_from_db(db);
                    if fetched > 0 {
                        log::debug!("sync: backfilled {fetched} URL favicon(s)");
                    }
                }
                Err(error) => {
                    return SyncCycleResult {
                        success: false,
                        message: format!("Remote merge failed: {error}"),
                        stats,
                        snapshot_counts: None,
                        did_push: false,
                    };
                }
            }
        }
        Err(error) if error == "@@unchanged" => remote_unchanged = true,
        Err(error) if error.contains("not found") || error.contains("不存在") => {}
        Err(error) => {
            return SyncCycleResult {
                success: false,
                message: format!("Pull failed: {error}"),
                stats,
                snapshot_counts: None,
                did_push: false,
            };
        }
    }

    if cancel.load(Ordering::Acquire) {
        return SyncCycleResult {
            success: false,
            message: "Sync cancelled".into(),
            stats,
            snapshot_counts: None,
            did_push: false,
        };
    }
    if !force_push && remote_unchanged && stats.is_empty() {
        return SyncCycleResult {
            success: true,
            message: "Up to date".into(),
            stats,
            snapshot_counts: None,
            did_push: false,
        };
    }

    let payload = match sync::build_snapshot(db, backend.name(), favorites_only) {
        Ok(payload) => payload,
        Err(error) => {
            return SyncCycleResult {
                success: false,
                message: format!("Snapshot build failed: {error}"),
                stats,
                snapshot_counts: None,
                did_push: false,
            };
        }
    };
    let snapshot_counts = (payload.items.len() as u32, payload.tags.len() as u32);

    if remote_hash.is_some_and(|hash| hash == sync::payload_semantic_hash(&payload)) {
        return SyncCycleResult {
            success: true,
            message: "Up to date".into(),
            stats,
            snapshot_counts: Some(snapshot_counts),
            did_push: false,
        };
    }
    if let Err(error) = backend.push(&payload) {
        return SyncCycleResult {
            success: false,
            message: format!("Push failed: {error}"),
            stats,
            snapshot_counts: Some(snapshot_counts),
            did_push: false,
        };
    }
    let _ = backend.post_push_cleanup();

    SyncCycleResult {
        success: true,
        message: format!(
            "Sync complete: {} items, {} tags",
            snapshot_counts.0, snapshot_counts.1
        ),
        stats,
        snapshot_counts: Some(snapshot_counts),
        did_push: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::settings::BackendConfig;
    use crate::core::sync::{SyncItem, SyncPayload};
    use std::sync::atomic::AtomicUsize;

    struct MemoryBackend {
        payload: SyncPayload,
        pushes: AtomicUsize,
    }

    impl SyncBackend for MemoryBackend {
        fn name(&self) -> &str {
            "test-backend"
        }

        fn check_status(&self) -> BackendStatus {
            BackendStatus::Online
        }

        fn pull(&self, _bypass_cache: bool) -> Result<SyncPayload, String> {
            Ok(self.payload.clone())
        }

        fn push(&self, _payload: &SyncPayload) -> Result<(), String> {
            self.pushes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn sync_interval(&self) -> u64 {
            60
        }
    }

    #[test]
    fn unchanged_snapshot_reports_backend_counts() {
        let db = Database::open(":memory:").expect("open in-memory database");
        db.insert_sync_item_raw(&SyncItem {
            content_type: "plain_text".into(),
            full_text: "same on both devices".into(),
            content_hash: 42,
            created_at: "2026-06-10T08:00:00Z".into(),
            updated_at: "2026-06-10T08:00:00Z".into(),
            rich_data: String::new(),
            is_favorite: true,
            note: String::new(),
            size: 20,
            tags: vec![],
            meta_type: String::new(),
        })
        .expect("insert sync item");
        let db = Mutex::new(db);
        let remote = sync::build_snapshot(&db, "remote-device", true).expect("build remote");
        let backend = MemoryBackend {
            payload: remote,
            pushes: AtomicUsize::new(0),
        };

        let result = run_sync_cycle_for_backend(&backend, &db, &AtomicBool::new(false), true, true);

        assert!(result.success);
        assert_eq!(result.snapshot_counts, Some((1, 0)));
        assert!(!result.did_push);
        assert_eq!(backend.pushes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unchanged_sync_refreshes_counts_without_refreshing_timestamp() {
        let mut config = BackendConfig {
            id: "backend".into(),
            enabled: true,
            backend_type: "local_folder".into(),
            name: "OneDrive".into(),
            folder_path: String::new(),
            device_name: String::new(),
            last_sync_at: "2026-06-09T08:00:00Z".into(),
            last_item_count: 6,
            last_tag_count: 0,
            sync_interval_secs: Some(60),
            webdav_url: String::new(),
            webdav_root_url: String::new(),
            webdav_path: String::new(),
            webdav_username: String::new(),
            webdav_password: String::new(),
        };
        let result = SyncCycleResult {
            success: true,
            message: "Up to date".into(),
            stats: MergeStats::default(),
            snapshot_counts: Some((7, 2)),
            did_push: false,
        };

        update_backend_sync_metadata(&mut config, &result, false);

        assert_eq!(config.last_item_count, 7);
        assert_eq!(config.last_tag_count, 2);
        assert_eq!(config.last_sync_at, "2026-06-09T08:00:00Z");
    }
}
