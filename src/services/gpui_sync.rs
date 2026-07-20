//! --- GPUI sync service. ---
//!
//! --- Keeps blocking backend work off the GPUI thread and publishes compact ---
//! --- snapshots into `AppState` from the unified WindowManager poll loop. ---

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;

use crate::core::db::Database;
use crate::core::i18n_keys::I18nKey;
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
    include_images: bool,
    compress_images: bool,
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
            include_images: settings.sync_include_images,
            compress_images: settings.sync_compress_images,
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
        self.include_images = settings.sync_include_images;
        self.compress_images = settings.sync_compress_images;
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
        let include_images = self.include_images;
        let compress_images = self.compress_images;

        std::thread::spawn(move || {
            let cycle = run_sync_cycle_for_backend(
                backend.as_ref(),
                &db,
                &cancel,
                favorites_only,
                force_push,
                include_images,
                compress_images,
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
        let ui_message = if result.cycle.success {
            result.cycle.message.clone()
        } else {
            log::warn!(
                "sync backend {} failed: {}",
                result.backend_id,
                result.cycle.message
            );
            summarize_sync_error(&result.cycle.message)
        };
        self.last_message = ui_message.clone();

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
            runtime.status = BackendStatus::Error(ui_message.clone());
            runtime.status_message = ui_message;
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
            include_images: self.include_images,
            compress_images: self.compress_images,
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
        BackendStatus::Error(error) => {
            log::warn!("sync backend status error: {error}");
            summarize_sync_error(error)
        }
    }
}

fn summarize_sync_error(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("auth")
        || lower.contains("401")
        || lower.contains("403")
        || lower.contains("credential")
        || lower.contains("permission")
    {
        return I18nKey::SyncErrAuth.text().to_string();
    }
    if lower.contains("connect")
        || lower.contains("connection")
        || lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("dns")
        || lower.contains("transport")
        || lower.contains("propfind")
        || lower.contains("mkcol")
        || lower.contains("head")
    {
        return I18nKey::SyncErrConnect.text().to_string();
    }
    if lower.contains("parse") || lower.contains("json") || lower.contains("toml") {
        return I18nKey::SyncErrParse.text().to_string();
    }
    if lower.contains("serialize") {
        return I18nKey::SyncErrSerialize.text().to_string();
    }
    if lower.contains("not found")
        || lower.contains("no such file")
        || lower.contains("404")
        || lower.contains("不存在")
    {
        return I18nKey::SyncErrNotFound.text().to_string();
    }
    if lower.contains("push")
        || lower.contains("write")
        || lower.contains("replace")
        || lower.contains("rename")
        || lower.contains("upload")
        || lower.contains("create")
    {
        return I18nKey::SyncErrPush.text().to_string();
    }
    if lower.contains("pull")
        || lower.contains("read")
        || lower.contains("download")
        || lower.contains("response")
    {
        return I18nKey::SyncErrPull.text().to_string();
    }
    if lower.contains("merge") || lower.contains("snapshot") || lower.contains("db") {
        return I18nKey::ErrDataOp.text().to_string();
    }
    I18nKey::ErrDataOp.text().to_string()
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
    include_images: bool,
    compress_images: bool,
) -> SyncCycleResult {
    let local_device = crate::services::backends::local_folder::hostname();
    let mut stats = MergeStats::default();
    let mut remote_hash = None;
    let mut remote_for_push = None;
    let mut remote_needs_rewrite = false;
    let mut remote_unchanged = false;
    let mut images_downloaded: u32 = 0;
    let mut images_uploaded: u32 = 0;

    match backend.pull(force_push) {
        Ok(mut remote) => {
            remote_needs_rewrite = remote.version < crate::core::migration::SYNC_VERSION;
            crate::core::migration::migrate_sync_payload(&mut remote);
            sync::sanitize_payload(&mut remote);
            remote_hash = Some(sync::payload_semantic_hash(&remote));
            match sync::merge_remote_into_local(db, &mut remote, &local_device) {
                Ok(merge_stats) => {
                    stats = merge_stats;
                    let fetched = crate::services::url_assets::backfill_link_favicons_from_db(db);
                    if fetched > 0 {
                        log::debug!("sync: backfilled {fetched} URL favicon(s)");
                    }

                    if include_images {
                        images_downloaded = download_missing_images(backend, db, &remote);
                    }
                    remote_for_push = Some(remote);
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
        Err(error)
            if error.contains("not found")
                || error.contains("No such file")
                || error.contains("404") => {}
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

    let mut payload = match sync::build_snapshot(db, &local_device, favorites_only, include_images)
    {
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
    if let Some(remote) = remote_for_push {
        payload = sync::merge_payloads(remote, payload);
        payload.device_name = local_device.clone();
        payload.synced_at = chrono::Utc::now().to_rfc3339();
    }

    let image_blobs = if include_images {
        prepare_image_snapshot_data(&mut payload, db, compress_images)
    } else {
        Vec::new()
    };

    let snapshot_counts = (payload.items.len() as u32, payload.tags.len() as u32);

    if include_images {
        match upload_prepared_images(backend, image_blobs) {
            Ok(count) => images_uploaded = count,
            Err(error) => {
                return SyncCycleResult {
                    success: false,
                    message: format!("Image upload failed: {error}"),
                    stats,
                    snapshot_counts: Some(snapshot_counts),
                    did_push: false,
                };
            }
        }
    }

    if !remote_needs_rewrite
        && remote_hash.is_some_and(|hash| hash == sync::payload_semantic_hash(&payload))
    {
        let _ = backend.post_push_cleanup();
        let message = if images_uploaded > 0 {
            format!("Up to date, images: up {images_uploaded}")
        } else {
            "Up to date".into()
        };
        return SyncCycleResult {
            success: true,
            message,
            stats,
            snapshot_counts: Some(snapshot_counts),
            did_push: images_uploaded > 0,
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

    let mut msg = format!(
        "Sync complete: {} items, {} tags",
        snapshot_counts.0, snapshot_counts.1
    );
    if images_downloaded > 0 || images_uploaded > 0 {
        msg.push_str(&format!(
            ", images: down {images_downloaded} up {images_uploaded}"
        ));
    }

    SyncCycleResult {
        success: true,
        message: msg,
        stats,
        snapshot_counts: Some(snapshot_counts),
        did_push: true,
    }
}

/// Download image blobs described by the remote payload and map them to local paths.
fn download_missing_images(
    backend: &dyn SyncBackend,
    db: &Mutex<Database>,
    remote: &crate::core::sync::SyncPayload,
) -> u32 {
    let images_dir = crate::core::paths::images_dir();
    let _ = std::fs::create_dir_all(&images_dir);

    let image_items = match db.lock() {
        Ok(db) => remote
            .items
            .iter()
            .filter_map(|remote_item| {
                let blob = remote_image_blob_name(remote_item)?;
                let local_item = db.get_by_hash(remote_item.content_hash).ok().flatten()?;
                (local_item.content_type.as_str() == "image").then_some((
                    remote_item.content_hash,
                    local_item.image_path,
                    blob,
                ))
            })
            .collect::<Vec<_>>(),
        Err(e) => {
            log::warn!("sync: db lock for image download failed: {e}");
            return 0;
        }
    };

    let mut count: u32 = 0;

    for (content_hash, current_image_path, blob) in &image_items {
        let Some((hash_hex, ext)) = image_blob_parts(blob, *content_hash) else {
            continue;
        };
        let dest = images_dir.join(blob);
        let dest_text = dest.to_string_lossy().to_string();

        if dest.exists() {
            if current_image_path != &dest_text {
                if let Ok(db) = db.lock() {
                    let _ = db.set_item_image_path(*content_hash, &dest_text);
                }
            }
            crate::platform::clipboard::ensure_thumbnail_for_image(&dest_text, *content_hash);
            continue;
        }

        if !current_image_path.is_empty() && std::path::Path::new(current_image_path).exists() {
            continue;
        }

        match backend.download_blob(&hash_hex, &ext) {
            Ok(data) => {
                let tmp = images_dir.join(format!(".{blob}.tmp"));
                if std::fs::write(&tmp, &data).is_ok() && std::fs::rename(&tmp, &dest).is_ok() {
                    if let Ok(db) = db.lock() {
                        let _ = db.set_item_image_path(*content_hash, &dest_text);
                    }

                    crate::platform::clipboard::ensure_thumbnail_for_image(
                        &dest_text,
                        *content_hash,
                    );

                    count += 1;
                } else {
                    let _ = std::fs::remove_file(&tmp);
                }
            }
            Err(e) => {
                // Not found or network error; retry next cycle.
                if !e.contains("not found") {
                    log::warn!("sync: download blob {hash_hex}.{ext} failed: {e}");
                }
            }
        }
    }

    if count > 0 {
        log::info!("sync: downloaded {count} image(s)");
    }
    count
}

fn remote_image_blob_name(item: &crate::core::sync::SyncItem) -> Option<String> {
    if item.content_type != "image" {
        return None;
    }

    if item.image_blob.is_empty() {
        return Some(format!("{:016x}.png", item.content_hash));
    }

    let filename = item
        .image_blob
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())?;
    image_blob_parts(filename, item.content_hash)?;
    Some(filename.to_string())
}

fn image_blob_parts(blob: &str, content_hash: u64) -> Option<(String, String)> {
    let (stem, ext) = blob.rsplit_once('.')?;
    let ext = ext.to_ascii_lowercase();
    let valid_ext = matches!(ext.as_str(), "png" | "jpg" | "jpeg");
    let expected_hash = format!("{:016x}", content_hash);
    let valid_name = stem == expected_hash
        && valid_ext
        && blob
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.');
    if !valid_name {
        return None;
    }
    Some((expected_hash, ext))
}

struct PreparedImageBlob {
    hash_hex: String,
    ext: String,
    data: Vec<u8>,
}

fn prepare_image_snapshot_data(
    payload: &mut crate::core::sync::SyncPayload,
    db: &Mutex<Database>,
    compress: bool,
) -> Vec<PreparedImageBlob> {
    let wanted_hashes: Vec<u64> = payload
        .items
        .iter()
        .filter(|item| item.content_type == "image")
        .map(|item| item.content_hash)
        .collect();
    let image_paths = match db.lock() {
        Ok(db) => wanted_hashes
            .iter()
            .filter_map(|hash| {
                db.get_by_hash(*hash)
                    .ok()
                    .flatten()
                    .map(|item| (*hash, item.image_path))
            })
            .collect::<HashMap<_, _>>(),
        Err(e) => {
            log::warn!("sync: db lock for image snapshot data failed: {e}");
            HashMap::new()
        }
    };

    let mut blobs = Vec::new();
    for item in &mut payload.items {
        if item.content_type != "image" {
            continue;
        }

        let hash_hex = format!("{:016x}", item.content_hash);
        if let Some(image_path) = image_paths.get(&item.content_hash) {
            let path = std::path::Path::new(image_path);
            if path.exists() {
                match crate::services::image_compressor::compress_for_sync(path, compress) {
                    Ok(result) => {
                        item.image_blob = format!("{hash_hex}.{}", result.ext);
                        blobs.push(PreparedImageBlob {
                            hash_hex: hash_hex.clone(),
                            ext: result.ext,
                            data: result.data,
                        });
                    }
                    Err(e) => {
                        log::warn!("sync: prepare image blob {hash_hex} failed: {e}");
                    }
                }
            }
        }

        if item.image_blob.is_empty() {
            item.image_blob = format!("{hash_hex}.png");
        }
    }
    blobs
}

fn upload_prepared_images(
    backend: &dyn SyncBackend,
    blobs: Vec<PreparedImageBlob>,
) -> Result<u32, String> {
    if blobs.is_empty() {
        return Ok(0);
    }

    let remote_blobs = match backend.list_remote_blobs() {
        Ok(blobs) => blobs,
        Err(e) => {
            log::warn!("sync: list remote blobs failed: {e}");
            return Err(e);
        }
    };

    let mut count: u32 = 0;
    for blob in blobs {
        let filename = format!("{}.{}", blob.hash_hex, blob.ext);
        if remote_blobs.iter().any(|name| name == &filename) {
            continue;
        }

        match backend.upload_blob(&blob.hash_hex, &blob.ext, &blob.data) {
            Ok(()) => count += 1,
            Err(e) => {
                log::warn!("sync: upload blob {filename} failed: {e}");
                return Err(e);
            }
        }
    }

    if count > 0 {
        log::info!("sync: uploaded {count} image(s)");
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::settings::BackendConfig;
    use crate::core::sync::{SyncItem, SyncPayload};
    use crate::core::types::ClipboardItem;
    use image::{ImageBuffer, Rgb};
    use std::collections::HashMap as StdHashMap;
    use std::sync::atomic::AtomicUsize;

    struct MemoryBackend {
        payload: SyncPayload,
        pushes: AtomicUsize,
    }

    impl SyncBackend for MemoryBackend {
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

    struct BlobMemoryBackend {
        pushed: Mutex<Option<SyncPayload>>,
        blobs: Mutex<StdHashMap<String, Vec<u8>>>,
    }

    impl BlobMemoryBackend {
        fn new() -> Self {
            Self {
                pushed: Mutex::new(None),
                blobs: Mutex::new(StdHashMap::new()),
            }
        }
    }

    impl SyncBackend for BlobMemoryBackend {
        fn check_status(&self) -> BackendStatus {
            BackendStatus::Online
        }

        fn pull(&self, _bypass_cache: bool) -> Result<SyncPayload, String> {
            Err("not found".into())
        }

        fn push(&self, payload: &SyncPayload) -> Result<(), String> {
            *self.pushed.lock().expect("payload lock") = Some(payload.clone());
            Ok(())
        }

        fn sync_interval(&self) -> u64 {
            60
        }

        fn upload_blob(&self, hash_hex: &str, ext: &str, data: &[u8]) -> Result<(), String> {
            self.blobs
                .lock()
                .expect("blob lock")
                .insert(format!("{hash_hex}.{ext}"), data.to_vec());
            Ok(())
        }

        fn list_remote_blobs(&self) -> Result<Vec<String>, String> {
            Ok(self
                .blobs
                .lock()
                .expect("blob lock")
                .keys()
                .cloned()
                .collect())
        }
    }

    struct FailingBlobBackend {
        pushed: AtomicUsize,
    }

    impl SyncBackend for FailingBlobBackend {
        fn check_status(&self) -> BackendStatus {
            BackendStatus::Online
        }

        fn pull(&self, _bypass_cache: bool) -> Result<SyncPayload, String> {
            Err("not found".into())
        }

        fn push(&self, _payload: &SyncPayload) -> Result<(), String> {
            self.pushed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn sync_interval(&self) -> u64 {
            60
        }

        fn upload_blob(&self, _hash_hex: &str, _ext: &str, _data: &[u8]) -> Result<(), String> {
            Err("simulated blob failure".into())
        }
    }

    struct RecordingDownloadBackend {
        requests: Mutex<Vec<(String, String)>>,
    }

    impl SyncBackend for RecordingDownloadBackend {
        fn check_status(&self) -> BackendStatus {
            BackendStatus::Online
        }

        fn pull(&self, _bypass_cache: bool) -> Result<SyncPayload, String> {
            Err("not found".into())
        }

        fn push(&self, _payload: &SyncPayload) -> Result<(), String> {
            Ok(())
        }

        fn sync_interval(&self) -> u64 {
            60
        }

        fn download_blob(&self, hash_hex: &str, ext: &str) -> Result<Vec<u8>, String> {
            self.requests
                .lock()
                .expect("requests lock")
                .push((hash_hex.to_string(), ext.to_string()));
            Err("blob not found".into())
        }
    }

    fn unique_temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "clippi-sync-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write_test_png(path: &std::path::Path, color: [u8; 3]) {
        let img = ImageBuffer::from_fn(32, 32, |x, y| {
            let tweak = ((x + y) % 7) as u8;
            Rgb([
                color[0].saturating_add(tweak),
                color[1].saturating_add(tweak),
                color[2].saturating_add(tweak),
            ])
        });
        img.save(path).expect("save png");
    }

    #[test]
    fn sync_errors_are_summarized_for_ui() {
        let raw = "Pull failed: Failed to parse sync file: expected value at line 1 column 1 in G:\\very\\long\\cloud\\path\\clippi_sync.json";
        let message = summarize_sync_error(raw);
        assert_eq!(message, I18nKey::SyncErrParse.text());
        assert!(!message.contains("G:\\"));
        assert!(!message.contains("line 1"));
    }

    #[test]
    fn sync_connection_status_hides_transport_details() {
        let raw = "Connection failed: timed out while connecting to https://example.invalid/webdav";
        let message = status_message(&BackendStatus::Error(raw.into()));
        assert_eq!(message, I18nKey::SyncErrConnect.text());
        assert!(!message.contains("example.invalid"));
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
            image_width: 0,
            image_height: 0,
            image_blob: String::new(),
        })
        .expect("insert sync item");
        let db = Mutex::new(db);
        let remote = sync::build_snapshot(&db, "remote-device", true, false).expect("build remote");
        let backend = MemoryBackend {
            payload: remote,
            pushes: AtomicUsize::new(0),
        };

        let result = run_sync_cycle_for_backend(
            &backend,
            &db,
            &AtomicBool::new(false),
            true,
            true,
            false,
            false,
        );

        assert!(result.success);
        assert_eq!(result.snapshot_counts, Some((1, 0)));
        assert!(!result.did_push);
        assert_eq!(backend.pushes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn image_blob_upload_uses_snapshot_scope_and_payload_filename() {
        let dir = unique_temp_dir();
        let favorite_path = dir.join("favorite.png");
        let other_path = dir.join("other.png");
        write_test_png(&favorite_path, [24, 64, 96]);
        write_test_png(&other_path, [96, 64, 24]);

        let db = Database::open(":memory:").expect("open in-memory database");
        let favorite =
            ClipboardItem::new_image(0, favorite_path.to_str().unwrap(), 0x1111, 32, 32, None);
        db.upsert(&favorite).expect("insert favorite image");
        let favorite_id = db
            .get_by_hash(0x1111)
            .expect("load favorite image")
            .expect("favorite image exists")
            .id;
        db.set_favorite(favorite_id, true).expect("favorite image");

        let other = ClipboardItem::new_image(0, other_path.to_str().unwrap(), 0x2222, 32, 32, None);
        db.upsert(&other).expect("insert nonfavorite image");

        let db = Mutex::new(db);
        let backend = BlobMemoryBackend::new();

        let result = run_sync_cycle_for_backend(
            &backend,
            &db,
            &AtomicBool::new(false),
            true,
            true,
            true,
            true,
        );

        assert!(result.success);
        assert!(result.did_push);
        assert_eq!(result.snapshot_counts, Some((1, 0)));

        let pushed = backend
            .pushed
            .lock()
            .expect("payload lock")
            .clone()
            .expect("payload pushed");
        assert_eq!(pushed.items.len(), 1);
        let image_item = &pushed.items[0];
        assert_eq!(image_item.content_hash, 0x1111);
        assert!(!image_item.image_blob.is_empty());

        let blobs = backend.blobs.lock().expect("blob lock");
        assert!(blobs.contains_key(&image_item.image_blob));
        assert!(blobs
            .keys()
            .all(|name| !name.starts_with("0000000000002222")));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn image_blob_upload_failure_prevents_metadata_push() {
        let dir = unique_temp_dir();
        let image_path = dir.join("favorite.png");
        write_test_png(&image_path, [24, 64, 96]);

        let db = Database::open(":memory:").expect("open in-memory database");
        let image = ClipboardItem::new_image(0, image_path.to_str().unwrap(), 0x3333, 32, 32, None);
        db.upsert(&image).expect("insert image");
        let image_id = db
            .get_by_hash(0x3333)
            .expect("load image")
            .expect("image exists")
            .id;
        db.set_favorite(image_id, true).expect("favorite image");

        let db = Mutex::new(db);
        let backend = FailingBlobBackend {
            pushed: AtomicUsize::new(0),
        };

        let result = run_sync_cycle_for_backend(
            &backend,
            &db,
            &AtomicBool::new(false),
            true,
            true,
            true,
            false,
        );

        assert!(!result.success);
        assert!(result.message.contains("Image upload failed"));
        assert!(!result.did_push);
        assert_eq!(backend.pushed.load(Ordering::SeqCst), 0);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn image_download_uses_remote_blob_not_local_absolute_path() {
        let db = Database::open(":memory:").expect("open in-memory database");
        let image = ClipboardItem::new_image(
            0,
            r"C:\Users\123\AppData\Local\PixPin\Temp\PixPin_20260720.png",
            0xBEEF,
            32,
            32,
            None,
        );
        db.upsert(&image).expect("insert image");
        let db = Mutex::new(db);
        let remote = SyncPayload {
            version: crate::core::migration::SYNC_VERSION,
            device_name: "remote-device".into(),
            synced_at: "2026-07-20T08:00:00Z".into(),
            items: vec![SyncItem {
                content_type: "image".into(),
                full_text: "PixPin_20260720.png".into(),
                content_hash: 0xBEEF,
                created_at: "2026-07-20T08:00:00Z".into(),
                updated_at: "2026-07-20T08:00:00Z".into(),
                rich_data: String::new(),
                is_favorite: false,
                note: String::new(),
                size: 0,
                tags: vec![],
                meta_type: String::new(),
                image_width: 32,
                image_height: 32,
                image_blob: "000000000000beef.jpg".into(),
            }],
            tags: vec![],
            deleted_items: vec![],
            deleted_tags: vec![],
            unfavorited_items: vec![],
        };
        let backend = RecordingDownloadBackend {
            requests: Mutex::new(Vec::new()),
        };

        let downloaded = download_missing_images(&backend, &db, &remote);

        assert_eq!(downloaded, 0);
        assert_eq!(
            *backend.requests.lock().expect("requests lock"),
            vec![("000000000000beef".to_string(), "jpg".to_string())]
        );
    }

    #[test]
    fn image_blob_parts_rejects_local_path_like_blob_names() {
        assert_eq!(
            image_blob_parts(
                r"C:\Users\123\AppData\Local\PixPin\Temp\PixPin_20260720.png",
                0xBEEF,
            ),
            None
        );
        assert_eq!(image_blob_parts("PixPin_20260720.png", 0xBEEF), None);
        assert_eq!(
            image_blob_parts("000000000000beef.png", 0xBEEF),
            Some(("000000000000beef".to_string(), "png".to_string()))
        );
    }

    #[test]
    fn remote_snapshot_is_merged_before_push_in_favorites_only_mode() {
        let db = Database::open(":memory:").expect("open in-memory database");
        db.insert_sync_item_raw(&SyncItem {
            content_type: "plain_text".into(),
            full_text: "same content".into(),
            content_hash: 42,
            created_at: "2026-06-10T08:00:00Z".into(),
            updated_at: "2026-06-10T08:00:00Z".into(),
            rich_data: String::new(),
            is_favorite: false,
            note: String::new(),
            size: 12,
            tags: vec![],
            meta_type: String::new(),
            image_width: 0,
            image_height: 0,
            image_blob: String::new(),
        })
        .expect("insert local item");
        let db = Mutex::new(db);
        let remote = SyncPayload {
            version: crate::core::migration::SYNC_VERSION,
            device_name: "remote-device".into(),
            synced_at: "2026-06-10T09:00:00Z".into(),
            items: vec![SyncItem {
                content_type: "plain_text".into(),
                full_text: "same content".into(),
                content_hash: 42,
                created_at: "2026-06-10T08:00:00Z".into(),
                updated_at: "2026-06-10T09:00:00Z".into(),
                rich_data: String::new(),
                is_favorite: false,
                note: String::new(),
                size: 12,
                tags: vec![],
                meta_type: String::new(),
                image_width: 0,
                image_height: 0,
                image_blob: String::new(),
            }],
            tags: vec![],
            deleted_items: vec![],
            deleted_tags: vec![],
            unfavorited_items: vec![],
        };
        let backend = MemoryBackend {
            payload: remote,
            pushes: AtomicUsize::new(0),
        };

        let result = run_sync_cycle_for_backend(
            &backend,
            &db,
            &AtomicBool::new(false),
            true,
            false,
            false,
            false,
        );

        assert!(result.success);
        assert!(!result.did_push);
        assert_eq!(backend.pushes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn old_sync_protocol_is_rewritten_even_when_semantically_unchanged() {
        let db = Mutex::new(Database::open(":memory:").expect("open in-memory database"));
        let remote = SyncPayload {
            version: crate::core::migration::SYNC_VERSION - 1,
            device_name: "remote-device".into(),
            synced_at: "2026-07-20T08:00:00Z".into(),
            items: vec![],
            tags: vec![],
            deleted_items: vec![],
            deleted_tags: vec![],
            unfavorited_items: vec![],
        };
        let backend = MemoryBackend {
            payload: remote,
            pushes: AtomicUsize::new(0),
        };

        let result = run_sync_cycle_for_backend(
            &backend,
            &db,
            &AtomicBool::new(false),
            false,
            true,
            false,
            false,
        );

        assert!(result.success);
        assert!(result.did_push);
        assert_eq!(backend.pushes.load(Ordering::SeqCst), 1);
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
