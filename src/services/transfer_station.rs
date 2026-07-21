//! Background transfer-station runtime and backend-independent operations.
//!
//! All blocking filesystem, database, hashing, and network work runs on a
//! worker thread. The GPUI poll loop only queues commands and applies compact
//! results to AppState.

use crate::core::db::Database;
use crate::core::i18n_keys::I18nKey;
use crate::core::settings::{AppSettings, BackendConfig};
use crate::core::sync::SyncBackend;
use crate::core::transfer_types::{FileManifest, ManifestEntry, ManifestWriteError, ResolvedEntry};
use crate::core::types::{ClipboardItem, ContentType, FileData, FileInfo};
use crate::core::{migration, paths};
use crate::services::backends::local_folder::LocalFolderBackend;
use crate::services::backends::webdav::WebDAVBackend;
use crate::state::app::AppState;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);
const MANIFEST_UPDATE_RETRIES: usize = 5;

#[derive(Debug, Clone)]
pub enum TransferCommand {
    Refresh,
    Upload {
        source_item_id: i64,
        source_path: String,
        file_name: String,
    },
    Download {
        entry: ManifestEntry,
    },
    Delete {
        entry: ManifestEntry,
    },
    Cleanup,
}

#[derive(Debug, Clone)]
enum TransferAction {
    Refresh,
    Upload(String),
    Download(ManifestEntry, String),
    Delete(String),
    Cleanup(u32),
}

#[derive(Debug, Clone)]
struct TransferJobResult {
    action: Result<TransferAction, (TransferCommand, String)>,
    entries: Option<Vec<ResolvedEntry>>,
    markers_cleared: bool,
}

#[derive(Default)]
pub struct TransferPollOutcome {
    pub state_changed: bool,
    pub data_changed: bool,
}

pub struct GpuiTransferService {
    backend_key: String,
    backend: Option<Arc<dyn SyncBackend>>,
    db_path: PathBuf,
    running: Arc<AtomicBool>,
    pending_result: Arc<Mutex<Option<TransferJobResult>>>,
    last_refresh: Option<Instant>,
    last_cleanup: Option<Instant>,
}

impl GpuiTransferService {
    pub fn new(settings: &AppSettings) -> Self {
        let mut service = Self {
            backend_key: String::new(),
            backend: None,
            db_path: settings.resolve_db_path(),
            running: Arc::new(AtomicBool::new(false)),
            pending_result: Arc::new(Mutex::new(None)),
            last_refresh: None,
            last_cleanup: None,
        };
        service.reload_from_settings(settings);
        service
    }

    pub fn reload_from_settings(&mut self, settings: &AppSettings) {
        self.db_path = settings.resolve_db_path();
        let selected = selected_backend(settings);
        let next_key = selected
            .and_then(|config| serde_json::to_string(config).ok())
            .unwrap_or_default();
        if next_key == self.backend_key {
            return;
        }
        self.backend_key = next_key;
        self.backend = selected.map(build_backend);
        self.last_refresh = None;
        self.last_cleanup = None;
    }

    pub fn poll(&mut self, app: &mut AppState) -> TransferPollOutcome {
        self.reload_from_settings(&app.settings);
        let mut outcome = TransferPollOutcome::default();

        if let Some(result) = self
            .pending_result
            .lock()
            .expect("transfer result lock poisoned")
            .take()
        {
            outcome = apply_result(app, result);
        }

        if !app.settings.transfer_station_enabled || self.backend.is_none() {
            app.transfer_busy = false;
            app.pending_transfer_commands.clear();
            app.pending_transfer_downloads.clear();
            return outcome;
        }

        if self.running.load(Ordering::Acquire) {
            app.transfer_busy = true;
            return outcome;
        }

        let now = Instant::now();
        let command = app.pending_transfer_commands.pop_front().or_else(|| {
            let cleanup_due = self
                .last_cleanup
                .is_none_or(|last| now.duration_since(last) >= CLEANUP_INTERVAL);
            if cleanup_due {
                Some(TransferCommand::Cleanup)
            } else if self
                .last_refresh
                .is_none_or(|last| now.duration_since(last) >= REFRESH_INTERVAL)
            {
                Some(TransferCommand::Refresh)
            } else {
                None
            }
        });

        if let Some(command) = command {
            if matches!(command, TransferCommand::Refresh) {
                self.last_refresh = Some(now);
            }
            if matches!(command, TransferCommand::Cleanup) {
                self.last_cleanup = Some(now);
            }
            app.transfer_busy = true;
            self.start(command, &app.settings);
            outcome.state_changed = true;
        } else {
            app.transfer_busy = false;
        }
        outcome
    }

    fn start(&mut self, command: TransferCommand, settings: &AppSettings) {
        if self.running.swap(true, Ordering::AcqRel) {
            return;
        }
        let backend = Arc::clone(self.backend.as_ref().expect("backend checked before start"));
        let db_path = self.db_path.clone();
        let pending = Arc::clone(&self.pending_result);
        let running = Arc::clone(&self.running);
        let retention_days = settings.transfer_retention_days;
        let device_name = selected_backend(settings)
            .map(|config| config.device_name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(crate::services::backends::local_folder::hostname);

        std::thread::spawn(move || {
            let result = run_command(
                backend.as_ref(),
                &db_path,
                command.clone(),
                &device_name,
                retention_days,
            );
            *pending.lock().expect("transfer result lock poisoned") = Some(match result {
                Ok((action, entries, markers_cleared)) => TransferJobResult {
                    action: Ok(action),
                    entries: Some(entries),
                    markers_cleared,
                },
                Err(error) => TransferJobResult {
                    action: Err((command, error)),
                    entries: None,
                    markers_cleared: false,
                },
            });
            running.store(false, Ordering::Release);
        });
    }
}

fn selected_backend(settings: &AppSettings) -> Option<&BackendConfig> {
    settings
        .sync_backends
        .iter()
        .find(|config| {
            config.enabled
                && !settings.transfer_backend_id.is_empty()
                && config.id == settings.transfer_backend_id
        })
        .or_else(|| settings.sync_backends.iter().find(|config| config.enabled))
}

fn build_backend(config: &BackendConfig) -> Arc<dyn SyncBackend> {
    match config.backend_type.as_str() {
        "webdav" => Arc::new(WebDAVBackend::new(config.clone())),
        _ => Arc::new(LocalFolderBackend::new(config.clone())),
    }
}

fn run_command(
    backend: &dyn SyncBackend,
    db_path: &Path,
    command: TransferCommand,
    device_name: &str,
    retention_days: u32,
) -> Result<(TransferAction, Vec<ResolvedEntry>, bool), String> {
    let db = Database::open(&db_path.to_string_lossy())
        .map_err(|error| format!("open transfer database: {error}"))?;
    let action = match command {
        TransferCommand::Refresh => TransferAction::Refresh,
        TransferCommand::Upload {
            source_item_id,
            source_path,
            file_name,
        } => {
            let name = upload_file(
                backend,
                &db,
                source_item_id,
                &source_path,
                &file_name,
                device_name,
                retention_days,
            )?;
            TransferAction::Upload(name)
        }
        TransferCommand::Download { entry } => {
            let path = download_file(backend, &db, &entry)?;
            TransferAction::Download(entry, path)
        }
        TransferCommand::Delete { entry } => {
            let name = entry.name.clone();
            delete_file(backend, &db, &entry)?;
            TransferAction::Delete(name)
        }
        TransferCommand::Cleanup => {
            let count = cleanup_expired(backend, &db, retention_days)?;
            TransferAction::Cleanup(count)
        }
    };
    let (entries, cleared_markers) = fetch_and_resolve(backend, &db)?;
    Ok((action, entries, cleared_markers > 0))
}

fn apply_result(app: &mut AppState, result: TransferJobResult) -> TransferPollOutcome {
    app.transfer_busy = false;
    if let Some(entries) = result.entries {
        app.has_transfer_files = !entries.is_empty();
        app.transfer_entries = entries;
    }

    let mut data_changed = result.markers_cleared;
    if result.markers_cleared {
        app.reload_items();
    }
    match result.action {
        Ok(TransferAction::Refresh) => {}
        Ok(TransferAction::Upload(name)) => {
            app.reload_items();
            app.toast_message = Some(I18nKey::TransferUploaded.text().replace("{0}", &name));
            data_changed = true;
        }
        Ok(TransferAction::Download(entry, path)) => {
            app.pending_transfer_downloads.remove(&entry.hash);
            app.reload_items();
            app.toast_message = Some(
                I18nKey::TransferDownloaded
                    .text()
                    .replace("{0}", &entry.name),
            );
            let item = transfer_clipboard_item(&entry, &path);
            crate::services::clipboard_ops::write_item_to_clipboard(&item, false);
            data_changed = true;
        }
        Ok(TransferAction::Delete(name)) => {
            app.reload_items();
            app.toast_message = Some(I18nKey::TransferDeleted.text().replace("{0}", &name));
            data_changed = true;
        }
        Ok(TransferAction::Cleanup(count)) => {
            if count > 0 {
                app.reload_items();
                data_changed = true;
            }
        }
        Err((command, error)) => {
            log::warn!("[transfer] command failed: {error}");
            if let TransferCommand::Download { ref entry } = command {
                app.pending_transfer_downloads.remove(&entry.hash);
            }
            let key = match command {
                TransferCommand::Upload { .. } => I18nKey::TransferUploadFailed,
                TransferCommand::Download { .. } => I18nKey::TransferDownloadFailed,
                TransferCommand::Delete { .. } => I18nKey::TransferDeleteFailed,
                TransferCommand::Refresh | TransferCommand::Cleanup => I18nKey::SyncErrPull,
            };
            app.toast_message = Some(key.text().replace("{0}", &error));
            app.toast_is_warning = true;
        }
    }

    TransferPollOutcome {
        state_changed: true,
        data_changed,
    }
}

pub fn fetch_and_resolve(
    backend: &dyn SyncBackend,
    db: &Database,
) -> Result<(Vec<ResolvedEntry>, usize), String> {
    let mut snapshot = backend.pull_file_manifest()?;
    if snapshot.manifest.version > migration::TRANSFER_PROTOCOL_VERSION {
        return Err(format!(
            "unsupported transfer manifest version {}",
            snapshot.manifest.version
        ));
    }
    migration::migrate_file_manifest(&mut snapshot.manifest);
    let local_paths = get_local_transfer_paths(db);
    for entry in &snapshot.manifest.files {
        entry.validate()?;
    }
    let active_hashes = snapshot
        .manifest
        .files
        .iter()
        .map(|entry| entry.hash.clone())
        .collect();
    let cleared_markers = db
        .clear_stale_file_transfer_hashes(&active_hashes)
        .map_err(|error| format!("clear stale transfer markers: {error}"))?;

    let entries = snapshot
        .manifest
        .files
        .into_iter()
        .map(|entry| {
            let local_path = local_paths.get(&entry.hash).cloned();
            Ok(ResolvedEntry {
                is_local: local_path.is_some(),
                local_path,
                entry,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((entries, cleared_markers))
}

fn upload_file(
    backend: &dyn SyncBackend,
    db: &Database,
    source_item_id: i64,
    local_path: &str,
    file_name: &str,
    device_name: &str,
    retention_days: u32,
) -> Result<String, String> {
    let source = Path::new(local_path);
    if !source.is_file() {
        return Err(I18nKey::TransferInvalidPath.text().into());
    }
    let name = portable_file_name(file_name)?;
    let data = std::fs::read(source).map_err(|error| format!("read file: {error}"))?;
    let hash = compute_file_hash(&data);
    let ext = Path::new(&name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let now = chrono::Utc::now();
    let expires_at = if retention_days == 0 {
        String::new()
    } else {
        (now + chrono::Duration::days(retention_days as i64)).to_rfc3339()
    };
    let entry = ManifestEntry {
        hash: hash.clone(),
        name: name.clone(),
        ext: ext.clone(),
        size: data.len() as u64,
        uploaded_at: now.to_rfc3339(),
        expires_at,
        uploaded_by: device_name.to_string(),
    };
    entry.validate()?;

    backend.upload_file_blob(&hash, &ext, &data)?;
    mutate_manifest(backend, |manifest| {
        manifest.files.retain(|existing| existing.hash != hash);
        manifest.files.push(entry.clone());
        manifest.device_name = device_name.to_string();
    })?;

    let canonical_path = std::fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    upsert_transfer_item(db, &entry, &canonical_path.to_string_lossy(), &data)?;
    db.set_file_transfer_hash(source_item_id, &hash)
        .map_err(|error| format!("mark original transfer item: {error}"))?;
    Ok(name)
}

fn download_file(
    backend: &dyn SyncBackend,
    db: &Database,
    entry: &ManifestEntry,
) -> Result<String, String> {
    entry.validate()?;
    let data = backend.download_file_blob(&entry.hash, &entry.ext)?;
    if data.len() as u64 != entry.size || compute_file_hash(&data) != entry.hash {
        return Err("downloaded file failed integrity verification".into());
    }

    let directory = paths::transfer_cache_dir().join(&entry.hash);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create transfer cache: {error}"))?;
    let destination = directory.join(&entry.name);
    let temporary = directory.join(format!(".{}.tmp", entry.name));
    std::fs::write(&temporary, &data).map_err(|error| format!("write cache: {error}"))?;
    crate::services::file_ops::replace_file(&temporary, &destination).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("replace cache file: {error}")
    })?;
    upsert_transfer_item(db, entry, &destination.to_string_lossy(), &data)?;
    Ok(destination.to_string_lossy().into_owned())
}

fn delete_file(
    backend: &dyn SyncBackend,
    db: &Database,
    entry: &ManifestEntry,
) -> Result<(), String> {
    entry.validate()?;
    let hash = entry.hash.clone();
    mutate_manifest(backend, |manifest| {
        manifest.files.retain(|existing| existing.hash != hash);
    })?;
    if let Err(error) = backend.delete_file_blob(&entry.hash, &entry.ext) {
        log::warn!("[transfer] orphaned blob cleanup failed after manifest deletion: {error}");
    }
    clean_local_transfer(db, &entry.hash);
    Ok(())
}

fn cleanup_expired(
    backend: &dyn SyncBackend,
    db: &Database,
    retention_days: u32,
) -> Result<u32, String> {
    if retention_days == 0 {
        return Ok(0);
    }
    let now = chrono::Utc::now();
    let snapshot = backend.pull_file_manifest()?;
    let expired: Vec<ManifestEntry> = snapshot
        .manifest
        .files
        .into_iter()
        .filter(|entry| entry_expired(entry, now, retention_days))
        .collect();
    if expired.is_empty() {
        return Ok(0);
    }
    let hashes: Vec<String> = expired.iter().map(|entry| entry.hash.clone()).collect();
    mutate_manifest(backend, |manifest| {
        manifest
            .files
            .retain(|entry| !hashes.iter().any(|hash| hash == &entry.hash));
    })?;
    for entry in &expired {
        if let Err(error) = backend.delete_file_blob(&entry.hash, &entry.ext) {
            log::warn!("[transfer] expired blob cleanup failed: {error}");
        }
        clean_local_transfer(db, &entry.hash);
    }
    Ok(expired.len() as u32)
}

fn entry_expired(
    entry: &ManifestEntry,
    now: chrono::DateTime<chrono::Utc>,
    retention_days: u32,
) -> bool {
    let expires_at = chrono::DateTime::parse_from_rfc3339(&entry.expires_at)
        .map(|value| value.with_timezone(&chrono::Utc))
        .or_else(|_| {
            chrono::DateTime::parse_from_rfc3339(&entry.uploaded_at).map(|value| {
                value.with_timezone(&chrono::Utc) + chrono::Duration::days(retention_days as i64)
            })
        });
    expires_at.is_ok_and(|value| value <= now)
}

fn mutate_manifest(
    backend: &dyn SyncBackend,
    mutate: impl Fn(&mut FileManifest),
) -> Result<(), String> {
    for _ in 0..MANIFEST_UPDATE_RETRIES {
        let mut snapshot = backend.pull_file_manifest()?;
        if snapshot.manifest.version > migration::TRANSFER_PROTOCOL_VERSION {
            return Err(format!(
                "unsupported transfer manifest version {}",
                snapshot.manifest.version
            ));
        }
        migration::migrate_file_manifest(&mut snapshot.manifest);
        for entry in &snapshot.manifest.files {
            entry.validate()?;
        }
        mutate(&mut snapshot.manifest);
        snapshot.manifest.version = migration::TRANSFER_PROTOCOL_VERSION;
        snapshot.manifest.updated_at = chrono::Utc::now().to_rfc3339();
        match backend.push_file_manifest(&snapshot.manifest, snapshot.revision.as_deref()) {
            Ok(_) => return Ok(()),
            Err(ManifestWriteError::Conflict) => continue,
            Err(ManifestWriteError::Other(error)) => return Err(error),
        }
    }
    Err("manifest changed repeatedly; retry the operation".into())
}

fn portable_file_name(value: &str) -> Result<String, String> {
    let name = Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| I18nKey::TransferInvalidPath.text().to_string())?;
    if name != value
        || name.is_empty()
        || name == "."
        || name == ".."
        || name.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        })
        || name.ends_with([' ', '.'])
    {
        return Err("file name is not portable across supported platforms".into());
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) {
        return Err("reserved file name is not portable".into());
    }
    Ok(name.to_string())
}

fn get_local_transfer_paths(db: &Database) -> HashMap<String, String> {
    db.get_transfer_items()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            let file_data = FileData::from_json(&item.file_data);
            let path = file_data.files.first()?.path.clone();
            (!file_data.remote_hash.is_empty() && Path::new(&path).is_file())
                .then_some((file_data.remote_hash, path))
        })
        .collect()
}

fn upsert_transfer_item(
    db: &Database,
    entry: &ManifestEntry,
    local_path: &str,
    data: &[u8],
) -> Result<(), String> {
    let now = chrono::Utc::now();
    let item = ClipboardItem {
        id: 0,
        content_type: ContentType::File,
        full_text: entry.name.clone(),
        content_hash: default_hash(data),
        created_at: now,
        updated_at: now,
        image_path: String::new(),
        image_width: 0,
        image_height: 0,
        rich_data: String::new(),
        file_data: FileData {
            files: vec![FileInfo {
                name: entry.name.clone(),
                path: local_path.to_string(),
                is_dir: false,
            }],
            transfer: true,
            remote_hash: entry.hash.clone(),
        }
        .to_json(),
        is_favorite: false,
        note: String::new(),
        source_app_name: String::new(),
        source_app_icon: String::new(),
        size: entry.size as i64,
        tags: Vec::new(),
        meta_type: "transfer".to_string(),
        custom_hotkey: String::new(),
        custom_hotkey_format: String::new(),
    };
    db.upsert(&item)
        .map_err(|error| format!("save transfer item: {error}"))
}

fn transfer_clipboard_item(entry: &ManifestEntry, local_path: &str) -> ClipboardItem {
    let now = chrono::Utc::now();
    ClipboardItem {
        id: 0,
        content_type: ContentType::File,
        full_text: entry.name.clone(),
        content_hash: 0,
        created_at: now,
        updated_at: now,
        image_path: String::new(),
        image_width: 0,
        image_height: 0,
        rich_data: String::new(),
        file_data: FileData {
            files: vec![FileInfo {
                name: entry.name.clone(),
                path: local_path.to_string(),
                is_dir: false,
            }],
            transfer: true,
            remote_hash: entry.hash.clone(),
        }
        .to_json(),
        is_favorite: false,
        note: String::new(),
        source_app_name: String::new(),
        source_app_icon: String::new(),
        size: entry.size as i64,
        tags: Vec::new(),
        meta_type: "transfer".to_string(),
        custom_hotkey: String::new(),
        custom_hotkey_format: String::new(),
    }
}

fn clean_local_transfer(db: &Database, hash: &str) {
    let cache_root = paths::transfer_cache_dir();
    if let Ok(items) = db.get_transfer_items() {
        for item in items {
            let file_data = FileData::from_json(&item.file_data);
            if file_data.remote_hash != hash {
                continue;
            }
            for file in file_data.files {
                let path = PathBuf::from(file.path);
                if path.starts_with(&cache_root) {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
    let hash_dir = cache_root.join(hash);
    if hash_dir.is_dir() {
        let _ = std::fs::remove_dir(&hash_dir);
    }
    if let Err(error) = db.clear_file_transfer_hash(hash) {
        log::warn!("[transfer] clear original upload marker failed: {error}");
    }
    let _ = db.delete_transfer_by_hash(hash);
}

fn compute_file_hash(data: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(data);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn default_hash(data: &[u8]) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(data);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_file_names_reject_paths_and_windows_reserved_names() {
        assert!(portable_file_name("report.pdf").is_ok());
        assert!(portable_file_name("中文 报告.final.pdf").is_ok());
        assert!(portable_file_name("README").is_ok());
        assert!(portable_file_name("../report.pdf").is_err());
        assert!(portable_file_name("folder\\report.pdf").is_err());
        assert!(portable_file_name("CON.txt").is_err());
        assert!(portable_file_name("bad?.txt").is_err());
        assert!(portable_file_name("trailing. ").is_err());
    }

    #[test]
    fn manifest_entry_rejects_path_traversal() {
        let entry = ManifestEntry {
            hash: "a".repeat(64),
            name: "../file.txt".into(),
            ext: "txt".into(),
            size: 1,
            uploaded_at: chrono::Utc::now().to_rfc3339(),
            expires_at: String::new(),
            uploaded_by: String::new(),
        };
        assert!(entry.validate().is_err());
    }

    #[test]
    fn expiry_prefers_explicit_timestamp_and_supports_legacy_entries() {
        let now = chrono::Utc::now();
        let mut entry = ManifestEntry {
            hash: "a".repeat(64),
            name: "file.txt".into(),
            ext: "txt".into(),
            size: 1,
            uploaded_at: (now - chrono::Duration::days(2)).to_rfc3339(),
            expires_at: (now + chrono::Duration::hours(1)).to_rfc3339(),
            uploaded_by: String::new(),
        };
        assert!(!entry_expired(&entry, now, 1));

        entry.expires_at.clear();
        assert!(entry_expired(&entry, now, 1));
        assert!(!entry_expired(&entry, now, 3));
    }
}
