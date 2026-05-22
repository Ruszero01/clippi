//! Sync manager — orchestrates multiple sync backends.
//!
//! Each backend (local folder, future WebDAV, etc.) implements `SyncBackend`
//! and is managed here with its own status, schedule, and background thread.

use crate::core::db::Database;
use crate::core::i18n;
use crate::core::settings::{generate_id, AppSettings, BackendConfig};
use crate::core::sync::{self, BackendStatus, BackendType, MergeStats, SyncBackend};
use crate::looper::Pollable;
use crate::services::backends::local_folder::LocalFolderBackend;
use crate::App;
use crate::SyncBackendInfo;
use slint::{Model, ModelRc, SharedString, VecModel};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Result of one backend's sync cycle (passed from bg thread to main).
#[derive(Debug, Clone)]
struct BackendSyncResult {
    backend_id: String,
    success: bool,
    message: String,
    stats: MergeStats,
    pushed_items: u32,
    pushed_tags: u32,
}

struct BackendState {
    backend: Arc<dyn SyncBackend>,
    status: BackendStatus,
    last_sync: Option<Instant>,
    last_sync_at: String,
    last_item_count: u32,
    last_tag_count: u32,
    folder_path: String,
    service_label: String,
    is_running: Arc<AtomicBool>,
    cancel_flag: Arc<AtomicBool>,
    pending_result: Arc<Mutex<Option<BackendSyncResult>>>,
}

impl BackendState {
    fn to_slint_info(&self) -> SyncBackendInfo {
        let status_str: String = match &self.status {
            BackendStatus::Online => {
                if self.is_running.load(Ordering::Relaxed) {
                    "syncing"
                } else {
                    "online"
                }
            }
            BackendStatus::Offline => "offline",
            BackendStatus::Error(_) => "error",
        }
        .into();

        let status_msg: String = match &self.status {
            BackendStatus::Online => String::new(),
            BackendStatus::Offline => i18n::tr("目录不存在", "Dir not found").into(),
            BackendStatus::Error(e) => e.clone(),
        };

        SyncBackendInfo {
            id: SharedString::from(self.backend.id()),
            name: SharedString::from(self.backend.name()),
            backend_type: SharedString::from(match self.backend.backend_type() {
                BackendType::LocalFolder => "local_folder",
            }),
            status: SharedString::from(&status_str),
            status_message: SharedString::from(&status_msg),
            enabled: true,
            folder_path: SharedString::from(&self.folder_path),
            last_sync_at: SharedString::from(&format_relative_time(&self.last_sync_at)),
            item_count: self.last_item_count as i32,
            tag_count: self.last_tag_count as i32,
            service_label: SharedString::from(&self.service_label),
        }
    }
}

fn detect_service_label(folder_path: &str) -> String {
    let lower = folder_path.to_lowercase();
    if lower.contains("onedrive") {
        "OneDrive".into()
    } else if lower.contains("icloud") {
        "iCloud".into()
    } else {
        i18n::tr("本地", "Local").into()
    }
}

fn format_relative_time(rfc3339: &str) -> String {
    if rfc3339.is_empty() {
        return i18n::tr("从未同步", "Never synced").into();
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(rfc3339) {
        crate::core::types::format_relative_time(&dt.to_utc())
    } else {
        rfc3339.to_string()
    }
}

pub struct SyncManager {
    db: Arc<Mutex<Database>>,
    settings: Arc<Mutex<AppSettings>>,
    app: slint::Weak<App>,

    /// Shared with ClipboardService — true when local data has changed.
    pub dirty: Arc<AtomicBool>,

    /// Shared with ClipboardService — true when sync externally modified the DB.
    needs_model_refresh: Arc<AtomicBool>,

    /// Backend states (only enabled backends are live here).
    backends: Vec<BackendState>,

    /// Slint model for backend list UI.
    model: Rc<VecModel<SyncBackendInfo>>,

    /// Manual "Sync Now" trigger from UI.
    manual_trigger: Arc<AtomicBool>,

    /// Last sync result summary for global UI display.
    last_sync_message: String,
    last_sync_items_added: u32,
    last_sync_items_updated: u32,
    last_sync_tags_added: u32,
}

impl SyncManager {
    pub fn new(
        db: Arc<Mutex<Database>>,
        settings: Arc<Mutex<AppSettings>>,
        app: slint::Weak<App>,
        dirty: Arc<AtomicBool>,
        needs_model_refresh: Arc<AtomicBool>,
    ) -> Self {
        let model: Rc<VecModel<SyncBackendInfo>> = Rc::new(VecModel::default());

        // Set the model on the app so Slint picks it up
        if let Some(app_ref) = app.upgrade() {
            app_ref.set_sync_backends(ModelRc::from(model.clone()));
        }

        let mut mgr = Self {
            db,
            settings,
            app,
            dirty,
            needs_model_refresh,
            backends: Vec::new(),
            model,
            manual_trigger: Arc::new(AtomicBool::new(false)),
            last_sync_message: String::new(),
            last_sync_items_added: 0,
            last_sync_items_updated: 0,
            last_sync_tags_added: 0,
        };

        // Populate backends from settings
        mgr.reload_backends();
        mgr
    }

    // ── Public API (called from app.rs callbacks) ──

    /// Add a new local-folder backend and persist.
    pub fn add_local_folder_backend(&mut self, name: String, folder_path: String) {
        let config = BackendConfig {
            id: generate_id(),
            enabled: true,
            backend_type: "local_folder".into(),
            name,
            folder_path,
            device_name: String::new(),
            last_sync_at: String::new(),
            last_item_count: 0,
            last_tag_count: 0,
        };

        // Persist
        {
            let mut s = self.settings.lock().expect("settings lock");
            s.sync_backends.push(config.clone());
            s.save();
        }

        self.add_state_for_config(config);
        self.refresh_model();
    }

    /// Remove a backend by id and persist.
    pub fn remove_backend(&mut self, id: &str) {
        self.backends.retain(|b| {
            if b.backend.id() == id {
                b.cancel_flag.store(true, Ordering::SeqCst);
                false
            } else {
                true
            }
        });

        {
            let mut s = self.settings.lock().expect("settings lock");
            s.sync_backends.retain(|c| c.id != id);
            s.save();
        }

        self.refresh_model();
    }

    /// Toggle a backend enabled/disabled and persist.
    pub fn toggle_backend(&mut self, id: &str) {
        {
            let mut s = self.settings.lock().expect("settings lock");
            for cfg in &mut s.sync_backends {
                if cfg.id == id {
                    cfg.enabled = !cfg.enabled;
                    s.save();
                    break;
                }
            }
        }
        self.reload_backends();
        self.refresh_model();
    }

    /// Get backend name and folder path by ID.
    pub fn get_backend_info(&self, id: &str) -> Option<(String, String)> {
        self.backends
            .iter()
            .find(|b| b.backend.id() == id)
            .map(|b| (b.backend.name().to_string(), b.folder_path.clone()))
    }

    /// Edit a backend — updates name and folder path, then persists.
    pub fn edit_backend(&mut self, id: &str, new_name: &str, new_path: &str) {
        {
            let mut s = self.settings.lock().expect("settings lock");
            for cfg in &mut s.sync_backends {
                if cfg.id == id {
                    cfg.name = new_name.to_string();
                    cfg.folder_path = new_path.to_string();
                    s.save();
                    break;
                }
            }
        }
        self.reload_backends();
        self.refresh_model();
    }

    /// Trigger an immediate sync cycle for all enabled backends.
    pub fn trigger_sync_now(&self) {
        self.manual_trigger.store(true, Ordering::SeqCst);
    }

    // ── Internal ──

    fn reload_backends(&mut self) {
        for state in &self.backends {
            state.cancel_flag.store(true, Ordering::SeqCst);
        }

        let configs: Vec<BackendConfig> = {
            let s = self.settings.lock().expect("settings lock");
            s.sync_backends
                .iter()
                .filter(|c| c.enabled)
                .cloned()
                .collect()
        };

        let mut new_states: Vec<BackendState> = Vec::new();
        for config in &configs {
            let existing = self.backends.iter().find(|s| s.backend.id() == config.id);
            if let Some(old) = existing {
                new_states.push(BackendState {
                    backend: Arc::new(LocalFolderBackend::new(config.clone())),
                    status: old.status.clone(),
                    last_sync: old.last_sync,
                    last_sync_at: old.last_sync_at.clone(),
                    last_item_count: old.last_item_count,
                    last_tag_count: old.last_tag_count,
                    folder_path: old.folder_path.clone(),
                    service_label: old.service_label.clone(),
                    is_running: Arc::clone(&old.is_running),
                    cancel_flag: Arc::clone(&old.cancel_flag),
                    pending_result: Arc::clone(&old.pending_result),
                });
            } else {
                new_states.push(Self::build_state(config.clone()));
            }
        }

        self.backends = new_states;
    }

    fn add_state_for_config(&mut self, config: BackendConfig) {
        if !config.enabled {
            return;
        }
        if self.backends.iter().any(|b| b.backend.id() == config.id) {
            return;
        }
        self.backends.push(Self::build_state(config));
    }

    fn build_state(config: BackendConfig) -> BackendState {
        let folder_path = config.folder_path.clone();
        let service_label = detect_service_label(&folder_path);
        let last_sync_at = config.last_sync_at.clone();
        let last_item_count = config.last_item_count;
        let last_tag_count = config.last_tag_count;
        let backend: Arc<dyn SyncBackend> = Arc::new(LocalFolderBackend::new(config));

        BackendState {
            status: backend.check_status(),
            backend,
            last_sync: None,
            last_sync_at,
            last_item_count,
            last_tag_count,
            folder_path,
            service_label,
            is_running: Arc::new(AtomicBool::new(false)),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            pending_result: Arc::new(Mutex::new(None)),
        }
    }

    /// Whether auto-sync is enabled (controlled by UI toggle).
    fn auto_sync_enabled(&self) -> bool {
        self.settings
            .lock()
            .map(|s| s.sync_auto_enabled)
            .unwrap_or(false)
    }

    /// Whether sync-favorites-only is enabled.
    fn sync_favorites_only(&self) -> bool {
        self.settings
            .lock()
            .map(|s| s.sync_favorites_only)
            .unwrap_or(false)
    }

    fn should_sync(&self, state: &BackendState, interval_secs: u64, is_manual: bool) -> bool {
        if state.is_running.load(Ordering::SeqCst) {
            return false;
        }
        if is_manual {
            return true;
        }
        if !self.auto_sync_enabled() {
            return false;
        }
        // Dirty: sync immediately with a 5 s cooldown to avoid thrashing
        if self.dirty.load(Ordering::SeqCst) {
            if let Some(last) = state.last_sync {
                return last.elapsed() >= Duration::from_secs(5);
            }
            return true;
        }
        // Interval polling to check for remote changes
        let interval = Duration::from_secs(interval_secs);
        if let Some(last) = state.last_sync {
            return last.elapsed() >= interval;
        }
        true
    }

    fn start_sync_cycle(&mut self, state_index: usize, _interval_secs: u64, is_manual: bool) {
        let state = &mut self.backends[state_index];

        // Capture dirty before spawning thread — if local data changed since last push,
        // this cycle must push regardless of remote state.
        let was_dirty = self.dirty.swap(false, Ordering::SeqCst);
        let force_push = was_dirty || is_manual;

        state.cancel_flag.store(false, Ordering::SeqCst);
        state.is_running.store(true, Ordering::SeqCst);
        state.last_sync = Some(Instant::now());
        state.status = BackendStatus::Online; // optimistic

        let db = Arc::clone(&self.db);
        let cancel = Arc::clone(&state.cancel_flag);
        let pending = Arc::clone(&state.pending_result);
        let running = Arc::clone(&state.is_running);
        let backend = Arc::clone(&state.backend);
        let backend_id = state.backend.id().to_string();
        let favorites_only = self.sync_favorites_only();

        std::thread::spawn(move || {
            let (success, message, stats, pushed_items, pushed_tags) =
                run_sync_cycle_for_backend(backend.as_ref(), &db, &cancel, favorites_only, force_push);

            *pending.lock().expect("pending lock") = Some(BackendSyncResult {
                backend_id,
                success,
                message,
                stats,
                pushed_items,
                pushed_tags,
            });
            running.store(false, Ordering::SeqCst);
        });
    }

    fn apply_backend_result(&mut self, result: BackendSyncResult) {
        let msg = result.message.clone();

        // Show merge stats, or push counts if no remote data was merged
        let has_merge = result.stats.items_added > 0
            || result.stats.items_updated > 0
            || result.stats.items_deleted > 0;
        let items_added = if has_merge {
            result.stats.items_added
        } else {
            result.pushed_items
        };
        let items_updated = result.stats.items_updated;
        let tags_added = if result.stats.tags_added > 0 || result.stats.tags_deleted > 0 {
            result.stats.tags_added
        } else {
            result.pushed_tags
        };

        // Update global UI
        if let Some(app) = self.app.upgrade() {
            app.set_sync_status(SharedString::from(&result.message));
            app.set_sync_items_added(items_added as i32);
            app.set_sync_items_updated(items_updated as i32);
            app.set_sync_tags_added(tags_added as i32);
        }
        self.last_sync_message = result.message;
        self.last_sync_items_added = items_added;
        self.last_sync_items_updated = items_updated;
        self.last_sync_tags_added = tags_added;

        // Notify ClipboardService to reload its model if the DB was modified
        if has_merge {
            self.needs_model_refresh.store(true, Ordering::SeqCst);
        }

        // Update the specific backend's state
        if let Some(state) = self
            .backends
            .iter_mut()
            .find(|b| b.backend.id() == result.backend_id)
        {
            if result.success {
                state.last_sync_at = chrono::Utc::now().to_rfc3339();
                state.status = state.backend.check_status();
                // Only update counts when a real push happened (fast path returns 0,0)
                if result.pushed_items > 0 || result.pushed_tags > 0 {
                    state.last_item_count = result.pushed_items;
                    state.last_tag_count = result.pushed_tags;
                }
                // Persist stats so they survive app restart
                if let Ok(mut s) = self.settings.lock() {
                    if let Some(cfg) = s.sync_backends.iter_mut().find(|b| b.id == state.backend.id()) {
                        cfg.last_sync_at = state.last_sync_at.clone();
                        cfg.last_item_count = state.last_item_count;
                        cfg.last_tag_count = state.last_tag_count;
                        s.save();
                    }
                }
            } else {
                state.status = BackendStatus::Error(msg);
            }
        }
    }

    fn refresh_model(&self) {
        let infos: Vec<SyncBackendInfo> = self.backends.iter().map(|b| b.to_slint_info()).collect();
        // Rebuild model
        self.model.set_vec(infos);
    }

    fn refresh_model_incremental(&self) {
        // Update existing entries in-place when possible
        let count = self.backends.len();
        if self.model.row_count() != count {
            let infos: Vec<SyncBackendInfo> =
                self.backends.iter().map(|b| b.to_slint_info()).collect();
            self.model.set_vec(infos);
        } else {
            for (i, state) in self.backends.iter().enumerate() {
                let info = state.to_slint_info();
                self.model.set_row_data(i, info);
            }
        }
    }
}

impl Pollable for SyncManager {
    fn poll(&mut self) {
        let interval_secs = self
            .settings
            .lock()
            .map(|s| s.sync_interval_secs)
            .unwrap_or(60);

        // Phase 1: Check for completed background tasks
        let mut results: Vec<BackendSyncResult> = Vec::new();
        for state in &self.backends {
            if let Some(result) = state.pending_result.lock().expect("pending lock").take() {
                results.push(result);
            }
        }
        for result in results {
            self.apply_backend_result(result);
        }

        // Phase 2: Check all backends' online status
        for state in &mut self.backends {
            if !state.is_running.load(Ordering::Relaxed) {
                state.status = state.backend.check_status();
            }
        }

        // Phase 3: Start new sync cycles for due backends.
        // Manual trigger bypasses auto-sync toggle and cooldown.
        let manual = self.manual_trigger.swap(false, Ordering::SeqCst);
        let count = self.backends.len();
        for i in 0..count {
            if self.should_sync(&self.backends[i], interval_secs, manual) {
                self.start_sync_cycle(i, interval_secs, manual);
            }
        }

        // Push backend status list to Slint UI
        self.refresh_model_incremental();
    }

    fn stop(&mut self) {
        for state in &self.backends {
            state.cancel_flag.store(true, Ordering::SeqCst);
        }
    }
}

// ── Background sync logic for a single backend ──

fn run_sync_cycle_for_backend(
    backend: &dyn SyncBackend,
    db: &Mutex<Database>,
    cancel: &AtomicBool,
    favorites_only: bool,
    force_push: bool,
) -> (bool, String, MergeStats, u32, u32) {
    let device_name = backend.name().to_string();
    let local_device = crate::services::backends::local_folder::hostname();

    // Phase 1: Pull — read remote data and merge into local DB
    let mut stats = MergeStats::default();
    let mut remote_hash: Option<u64> = None;
    let mut remote_unchanged = false;
    match backend.pull(force_push) {
        Ok(mut remote) => {
            remote_hash = Some(sync::payload_semantic_hash(&remote));
            match sync::merge_remote_into_local(db, &mut remote, &local_device) {
                Ok(s) => stats = s,
                Err(e) => {
                    return (false, i18n::tr("合并远程数据失败: ", "Remote merge failed: ").to_string() + &e.to_string(), stats, 0, 0);
                }
            }
        }
        Err(e) => {
            if e == "@@unchanged" {
                remote_unchanged = true;
            } else if !e.contains("不存在") && !e.contains("not found") {
                return (false, i18n::tr("拉取失败: ", "Pull failed: ").to_string() + &e.to_string(), stats, 0, 0);
            }
        }
    }

    if cancel.load(Ordering::Relaxed) {
        return (false, i18n::tr("同步已取消", "Sync cancelled").into(), stats, 0, 0);
    }

    // Fast path: if remote file wasn't even touched (same mtime) and there's
    // nothing new to push, skip building the snapshot entirely.
    if !force_push && remote_unchanged && stats.is_empty() {
        return (true, i18n::tr("已是最新", "Up to date").into(), stats, 0, 0);
    }

    // Phase 2: Push — build local snapshot and write to backend
    let payload = match sync::build_snapshot(db, &device_name, favorites_only) {
        Ok(p) => p,
        Err(e) => {
            return (false, i18n::tr("构建快照失败: ", "Snapshot build failed: ").to_string() + &e.to_string(), stats, 0, 0);
        }
    };

    let pushed_items = payload.items.len() as u32;
    let pushed_tags = payload.tags.len() as u32;

    // Content-hash gate: if the local snapshot is semantically identical to
    // the remote payload we just pulled from, skip the push. This prevents
    // self-perpetuating sync loops when cloud providers change file mtime
    // after sync without changing content, and also avoids rewrites when a
    // local clipboard change is filtered out by favorites_only or other
    // sync criteria (the dirty flag was set, but nothing sync-worthy changed).
    if let Some(rh) = remote_hash {
        let local_hash = sync::payload_semantic_hash(&payload);
        if rh == local_hash {
            return (true, i18n::tr("已是最新", "Up to date").into(), stats, pushed_items, pushed_tags);
        }
    }

    if let Err(e) = backend.push(&payload) {
        return (false, i18n::tr("推送失败: ", "Push failed: ").to_string() + &e.to_string(), stats, pushed_items, pushed_tags);
    }

    // Best-effort cleanup of cloud conflict files (e.g. clippi_sync-*.json)
    let _ = backend.post_push_cleanup();

    let mut parts: Vec<String> = Vec::new();
    if stats.items_added > 0 {
        parts.push(if i18n::is_en() { format!("Added {}", stats.items_added) } else { format!("新增{}条", stats.items_added) });
    }
    if stats.items_updated > 0 {
        parts.push(if i18n::is_en() { format!("Updated {}", stats.items_updated) } else { format!("更新{}条", stats.items_updated) });
    }
    if stats.items_deleted > 0 {
        parts.push(if i18n::is_en() { format!("Deleted {}", stats.items_deleted) } else { format!("删除{}条", stats.items_deleted) });
    }
    if stats.tags_added > 0 {
        parts.push(if i18n::is_en() { format!("Tags +{}", stats.tags_added) } else { format!("标签+{}", stats.tags_added) });
    }
    if stats.tags_deleted > 0 {
        parts.push(if i18n::is_en() { format!("Tags -{}", stats.tags_deleted) } else { format!("标签-{}", stats.tags_deleted) });
    }
    let msg = if parts.is_empty() && pushed_items == 0 {
        i18n::tr("同步完成，本地无数据", "Sync done, no local data").to_string()
    } else if parts.is_empty() {
        i18n::tr("同步完成: 已推送 ", "Sync done: pushed ").to_string() + &format!("{pushed_items} ") + i18n::tr("条记录, ", "items, ") + &format!("{pushed_tags} ") + i18n::tr("个标签", "tags")
    } else {
        i18n::tr("同步完成: ", "Sync done: ").to_string() + &parts.join(", ")
    };

    (true, msg, stats, pushed_items, pushed_tags)
}
