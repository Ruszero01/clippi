//! Background transfer-station runtime and backend-independent operations.
//!
//! All blocking filesystem, database, hashing, and network work runs on a
//! worker thread. The GPUI poll loop only queues commands and applies compact
//! results to AppState.

use crate::core::db::Database;
use crate::core::i18n_keys::I18nKey;
use crate::core::settings::{AppSettings, BackendConfig};
use crate::core::sync::SyncBackend;
use crate::core::transfer_types::{
    effective_expiration, validate_portable_file_name, FileManifest, ManifestEntry,
    ManifestWriteError, ResolvedEntry, MAX_TRANSFER_FILE_SIZE_BYTES,
};
use crate::core::types::{ClipboardItem, ContentType, FileData, FileInfo};
use crate::core::{migration, paths};
use crate::services::backends::local_folder::LocalFolderBackend;
use crate::services::backends::webdav::WebDAVBackend;
use crate::state::app::AppState;
use sha2::Digest;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

const ACTIVE_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const MANIFEST_UPDATE_RETRIES: usize = 5;
const STREAM_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileDigest {
    size: u64,
    sha256: String,
    content_hash: u64,
}

struct DigestState {
    size: u64,
    sha256: sha2::Sha256,
    content_hash: std::collections::hash_map::DefaultHasher,
}

impl DigestState {
    fn new() -> Self {
        Self {
            size: 0,
            sha256: sha2::Sha256::new(),
            content_hash: std::collections::hash_map::DefaultHasher::new(),
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.size = self.size.saturating_add(data.len() as u64);
        self.sha256.update(data);
        self.content_hash.write(data);
    }

    fn finish(self) -> FileDigest {
        FileDigest {
            size: self.size,
            sha256: format!("{:x}", self.sha256.finalize()),
            content_hash: self.content_hash.finish(),
        }
    }
}

struct DigestingReader<R> {
    inner: R,
    digest: DigestState,
}

impl<R> DigestingReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            digest: DigestState::new(),
        }
    }

    fn finish(self) -> FileDigest {
        self.digest.finish()
    }
}

impl<R: Read> Read for DigestingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(buffer)?;
        self.digest.update(&buffer[..count]);
        Ok(count)
    }
}

struct DigestingWriter<W> {
    inner: W,
    digest: DigestState,
}

impl<W> DigestingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            digest: DigestState::new(),
        }
    }
}

impl<W: Write> DigestingWriter<W> {
    fn finish(mut self) -> std::io::Result<(W, FileDigest)> {
        self.inner.flush()?;
        Ok((self.inner, self.digest.finish()))
    }
}

impl<W: Write> Write for DigestingWriter<W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let count = self.inner.write(data)?;
        self.digest.update(&data[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[derive(Clone)]
struct CachedFileHash {
    length: u64,
    modified: Option<SystemTime>,
    hash: String,
}

static ORIGINAL_FILE_HASH_CACHE: LazyLock<Mutex<HashMap<PathBuf, CachedFileHash>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone)]
pub enum TransferCommand {
    Refresh,
    Upload {
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
    SetPinned {
        entry: ManifestEntry,
        pinned: bool,
    },
}

#[derive(Debug, Clone)]
enum TransferAction {
    Refresh,
    Upload { source_path: String, name: String },
    Download(ManifestEntry, String),
    Delete(String),
    Cleanup(u32),
    SetPinned(String),
}

#[derive(Debug, Clone)]
struct TransferJobResult {
    generation: u64,
    job_id: u64,
    action: Result<TransferAction, (TransferCommand, String)>,
    entries: Option<Vec<ResolvedEntry>>,
    markers_changed: bool,
}

#[derive(Debug, Clone, Copy)]
struct TransferMarkerResult {
    generation: u64,
    job_id: u64,
    changed: bool,
}

struct ResolvedManifest {
    entries: Vec<ResolvedEntry>,
    active_hashes: HashSet<String>,
    active_sizes: HashSet<u64>,
    records_changed: bool,
}

#[derive(Default)]
pub struct TransferPollOutcome {
    pub state_changed: bool,
    pub data_changed: bool,
}

pub struct GpuiTransferService {
    backend_key: String,
    backend: Option<Arc<dyn SyncBackend>>,
    backend_generation: u64,
    next_job_id: u64,
    applied_job_id: u64,
    db_path: PathBuf,
    running: Arc<AtomicBool>,
    pending_result: Arc<Mutex<Option<TransferJobResult>>>,
    marker_scan_running: Arc<AtomicBool>,
    pending_marker_result: Arc<Mutex<Option<TransferMarkerResult>>>,
    last_refresh: Option<Instant>,
}

impl GpuiTransferService {
    pub fn new(settings: &AppSettings) -> Self {
        let mut service = Self {
            backend_key: String::new(),
            backend: None,
            backend_generation: 0,
            next_job_id: 0,
            applied_job_id: 0,
            db_path: settings.resolve_db_path(),
            running: Arc::new(AtomicBool::new(false)),
            pending_result: Arc::new(Mutex::new(None)),
            marker_scan_running: Arc::new(AtomicBool::new(false)),
            pending_marker_result: Arc::new(Mutex::new(None)),
            last_refresh: None,
        };
        service.reload_from_settings(settings);
        service
    }

    pub fn reload_from_settings(&mut self, settings: &AppSettings) {
        let next_db_path = settings.resolve_db_path();
        let db_key = next_db_path.to_string_lossy().into_owned();
        let selected = selected_backend(settings);
        let next_key = selected
            .and_then(|config| {
                serde_json::to_string(&(
                    settings.transfer_station_enabled,
                    &config.id,
                    &config.backend_type,
                    &config.folder_path,
                    &config.device_name,
                    &config.webdav_url,
                    &config.webdav_root_url,
                    &config.webdav_path,
                    &config.webdav_username,
                    &config.webdav_password,
                    &db_key,
                ))
                .ok()
            })
            .unwrap_or_default();
        if next_key == self.backend_key {
            self.db_path = next_db_path;
            return;
        }
        self.db_path = next_db_path;
        self.backend_key = next_key;
        self.backend_generation = self.backend_generation.wrapping_add(1);
        self.backend = selected.map(build_backend);
        self.last_refresh = None;
    }

    pub fn poll(&mut self, app: &mut AppState, window_visible: bool) -> TransferPollOutcome {
        self.reload_from_settings(&app.settings);
        let mut outcome = TransferPollOutcome::default();

        if let Some(result) = self
            .pending_result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            app.transfer_refreshing = false;
            if result.generation == self.backend_generation {
                self.applied_job_id = result.job_id;
                // Every completed command counts as a refresh attempt: successful
                // commands resolve the latest manifest, while failed commands need
                // a bounded retry delay. Base the next poll on completion time so
                // slow requests do not trigger an immediate duplicate request.
                self.last_refresh = Some(Instant::now());
                let result_outcome = apply_result(app, result);
                outcome.state_changed |= result_outcome.state_changed;
                outcome.data_changed |= result_outcome.data_changed;
            } else {
                clear_pending_for_stale_result(app, &result);
                app.transfer_busy = false;
                outcome.state_changed = true;
            }
        }

        if let Some(marker_result) = self
            .pending_marker_result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            if marker_result.generation == self.backend_generation
                && marker_result.job_id == self.applied_job_id
                && marker_result.changed
            {
                app.reload_items();
                outcome.state_changed = true;
                outcome.data_changed = true;
            }
        }

        if !app.settings.transfer_station_enabled || self.backend.is_none() {
            let data_changed = app.transfer_filter_active
                || app.has_transfer_files
                || !app.transfer_entries.is_empty();
            if data_changed {
                outcome.state_changed = true;
                outcome.data_changed = true;
            }
            app.transfer_busy = false;
            app.transfer_refreshing = false;
            app.transfer_filter_active = false;
            app.has_transfer_files = false;
            app.transfer_entries.clear();
            app.pending_transfer_commands.clear();
            app.pending_transfer_downloads.clear();
            app.pending_transfer_uploads.clear();
            return outcome;
        }

        if self.running.load(Ordering::Acquire) {
            app.transfer_busy = true;
            return outcome;
        }

        let now = Instant::now();
        let refresh_interval =
            automatic_refresh_interval(window_visible && app.transfer_filter_active);
        let command = app.pending_transfer_commands.pop_front().or_else(|| {
            let refresh_interval = refresh_interval?;
            if self
                .last_refresh
                .is_none_or(|last| now.duration_since(last) >= refresh_interval)
            {
                Some(TransferCommand::Refresh)
            } else {
                None
            }
        });

        if let Some(command) = command {
            app.transfer_busy = true;
            // toggle_transfer_filter arms this flag for the entry refresh. An
            // automatic Refresh must not turn it back on every two seconds.
            app.transfer_refreshing = refresh_indicator_visible(&command, app.transfer_refreshing);
            self.start(command, &app.settings);
            outcome.state_changed = true;
        } else {
            app.transfer_busy = false;
            app.transfer_refreshing = false;
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
        let marker_scan_running = Arc::clone(&self.marker_scan_running);
        let pending_marker_result = Arc::clone(&self.pending_marker_result);
        let retention_days = settings.transfer_retention_days;
        let generation = self.backend_generation;
        self.next_job_id = self.next_job_id.wrapping_add(1);
        let job_id = self.next_job_id;
        let device_name = selected_backend(settings)
            .map(|config| config.device_name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(crate::services::backends::local_folder::hostname);

        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_command(
                    backend.as_ref(),
                    &db_path,
                    command.clone(),
                    &device_name,
                    retention_days,
                )
            }))
            .unwrap_or_else(|_| Err("transfer worker terminated unexpectedly".into()));
            let marker_scan = match result {
                Ok((action, resolved)) => {
                    let marker_scan = Some((resolved.active_hashes, resolved.active_sizes));
                    *pending
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some(TransferJobResult {
                            generation,
                            job_id,
                            action: Ok(action),
                            entries: Some(resolved.entries),
                            markers_changed: resolved.records_changed,
                        });
                    marker_scan
                }
                Err(error) => {
                    *pending
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some(TransferJobResult {
                            generation,
                            job_id,
                            action: Err((command, error)),
                            entries: None,
                            markers_changed: false,
                        });
                    None
                }
            };
            running.store(false, Ordering::Release);

            // Local content matching can touch slow or offline paths and hash large
            // files. Publish the manifest first, then fill ordinary-file markers
            // independently so first paint and subsequent commands are not blocked.
            if let Some((active_hashes, active_sizes)) = marker_scan {
                if !marker_scan_running.swap(true, Ordering::AcqRel) {
                    let changed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let db = Database::open(&db_path.to_string_lossy()).ok()?;
                        reconcile_original_file_markers(&db, &active_hashes, &active_sizes).ok()
                    }))
                    .ok()
                    .flatten()
                    .is_some_and(|changed| changed > 0);
                    let mut marker_result = pending_marker_result
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    match marker_result.as_mut() {
                        Some(existing) if existing.generation == generation => {
                            if existing.job_id == job_id {
                                existing.changed |= changed;
                            } else {
                                *existing = TransferMarkerResult {
                                    generation,
                                    job_id,
                                    changed,
                                };
                            }
                        }
                        _ => {
                            *marker_result = Some(TransferMarkerResult {
                                generation,
                                job_id,
                                changed,
                            });
                        }
                    }
                    marker_scan_running.store(false, Ordering::Release);
                }
            }
        });
    }
}

fn automatic_refresh_interval(transfer_view_visible: bool) -> Option<Duration> {
    transfer_view_visible.then_some(ACTIVE_REFRESH_INTERVAL)
}

fn refresh_indicator_visible(command: &TransferCommand, armed_on_entry: bool) -> bool {
    armed_on_entry && matches!(command, TransferCommand::Refresh)
}

fn clear_pending_for_stale_result(app: &mut AppState, result: &TransferJobResult) {
    match &result.action {
        Ok(TransferAction::Upload { source_path, .. }) => {
            app.pending_transfer_uploads.remove(source_path);
        }
        Ok(TransferAction::Download(entry, _)) => {
            app.pending_transfer_downloads.remove(&entry.hash);
        }
        Ok(TransferAction::SetPinned(hash)) => {
            app.pending_transfer_pin_updates.remove(hash);
        }
        Err((TransferCommand::Upload { source_path, .. }, _)) => {
            app.pending_transfer_uploads.remove(source_path);
        }
        Err((TransferCommand::Download { entry }, _)) => {
            app.pending_transfer_downloads.remove(&entry.hash);
        }
        Err((TransferCommand::SetPinned { entry, .. }, _)) => {
            app.pending_transfer_pin_updates.remove(&entry.hash);
        }
        _ => {}
    }
}

fn selected_backend(settings: &AppSettings) -> Option<&BackendConfig> {
    if settings.transfer_backend_id.is_empty() {
        settings.sync_backends.iter().find(|config| config.enabled)
    } else {
        settings
            .sync_backends
            .iter()
            .find(|config| config.enabled && config.id == settings.transfer_backend_id)
    }
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
) -> Result<(TransferAction, ResolvedManifest), String> {
    let db = Database::open(&db_path.to_string_lossy())
        .map_err(|error| format!("open transfer database: {error}"))?;
    let action = match command {
        TransferCommand::Refresh => TransferAction::Refresh,
        TransferCommand::Upload {
            source_path,
            file_name,
        } => {
            let name = upload_file(
                backend,
                &db,
                &source_path,
                &file_name,
                device_name,
                retention_days,
            )?;
            TransferAction::Upload { source_path, name }
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
        TransferCommand::SetPinned { entry, pinned } => {
            let hash = entry.hash.clone();
            set_pinned_file(backend, &entry, pinned, retention_days)?;
            TransferAction::SetPinned(hash)
        }
    };
    let resolved = fetch_and_resolve_manifest(backend, &db)?;
    Ok((action, resolved))
}

fn apply_result(app: &mut AppState, result: TransferJobResult) -> TransferPollOutcome {
    app.transfer_busy = false;
    let mut data_changed = result.markers_changed;
    if let Some(entries) = result.entries {
        data_changed |= update_resolved_entries(
            &mut app.transfer_entries,
            &mut app.has_transfer_files,
            entries,
        );
    }

    if result.markers_changed {
        app.reload_items();
    }
    match result.action {
        Ok(TransferAction::Refresh) => {}
        Ok(TransferAction::Upload { source_path, name }) => {
            app.pending_transfer_uploads.remove(&source_path);
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
        Ok(TransferAction::SetPinned(hash)) => {
            app.pending_transfer_pin_updates.remove(&hash);
            data_changed = true;
        }
        Err((command, error)) => {
            log::warn!("[transfer] command failed: {error}");
            if let TransferCommand::Download { ref entry } = command {
                app.pending_transfer_downloads.remove(&entry.hash);
            }
            if let TransferCommand::Upload {
                ref source_path, ..
            } = command
            {
                app.pending_transfer_uploads.remove(source_path);
            }
            if let TransferCommand::SetPinned { ref entry, .. } = command {
                app.pending_transfer_pin_updates.remove(&entry.hash);
            }
            let key = match command {
                TransferCommand::Upload { .. } => I18nKey::TransferUploadFailed,
                TransferCommand::Download { .. } => I18nKey::TransferDownloadFailed,
                TransferCommand::Delete { .. } => I18nKey::TransferDeleteFailed,
                TransferCommand::SetPinned { .. } => I18nKey::TransferPinFailed,
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

fn update_resolved_entries(
    current: &mut Vec<ResolvedEntry>,
    has_transfer_files: &mut bool,
    entries: Vec<ResolvedEntry>,
) -> bool {
    let entries_changed = *current != entries;
    let availability = !entries.is_empty();
    let availability_changed = *has_transfer_files != availability;
    if entries_changed {
        *current = entries;
    }
    if availability_changed {
        *has_transfer_files = availability;
    }
    entries_changed || availability_changed
}

fn fetch_and_resolve_manifest(
    backend: &dyn SyncBackend,
    db: &Database,
) -> Result<ResolvedManifest, String> {
    let mut snapshot = backend.pull_file_manifest()?;
    if snapshot.manifest.version > migration::TRANSFER_PROTOCOL_VERSION {
        return Err(I18nKey::TransferProtocolUnsupported.text().into());
    }
    migration::migrate_file_manifest(&mut snapshot.manifest);
    validate_manifest(&snapshot.manifest)?;
    let active_hashes: HashSet<String> = snapshot
        .manifest
        .files
        .iter()
        .map(|entry| entry.hash.clone())
        .collect();
    let active_sizes: HashSet<u64> = snapshot
        .manifest
        .files
        .iter()
        .map(|entry| entry.size)
        .collect();
    let records_changed = reconcile_transfer_records(db, &active_hashes) > 0;
    let local_paths = get_local_transfer_paths(db);

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
    Ok(ResolvedManifest {
        entries,
        active_hashes,
        active_sizes,
        records_changed,
    })
}

fn reconcile_original_file_markers(
    db: &Database,
    active_hashes: &HashSet<String>,
    active_sizes: &HashSet<u64>,
) -> Result<usize, String> {
    let items = db
        .get_original_file_items()
        .map_err(|error| format!("load original file items: {error}"))?;
    let mut changed = 0;
    let mut seen_paths = HashSet::new();

    for item in items {
        let file_data = FileData::from_json(&item.file_data);
        let derived_hash = if let [file] = file_data.files.as_slice() {
            if file.is_dir {
                None
            } else {
                match cached_file_hash(Path::new(&file.path), active_sizes) {
                    Ok(Some((cache_key, hash))) => {
                        seen_paths.insert(cache_key);
                        active_hashes.contains(&hash).then_some(hash)
                    }
                    Ok(None) | Err(_) => None,
                }
            }
        } else {
            None
        };

        changed += usize::from(
            db.set_derived_file_transfer_hash(item.id, derived_hash.as_deref())
                .map_err(|error| format!("update derived transfer marker: {error}"))?,
        );
    }

    ORIGINAL_FILE_HASH_CACHE
        .lock()
        .expect("transfer file hash cache lock poisoned")
        .retain(|path, _| seen_paths.contains(path));
    Ok(changed)
}

fn cached_file_hash(
    path: &Path,
    active_sizes: &HashSet<u64>,
) -> Result<Option<(PathBuf, String)>, String> {
    let cache_key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let metadata = std::fs::metadata(&cache_key)
        .map_err(|error| format!("read local file metadata: {error}"))?;
    if !metadata.is_file() || !active_sizes.contains(&metadata.len()) {
        return Ok(None);
    }

    let modified = metadata.modified().ok();
    if modified.is_some() {
        let cache = ORIGINAL_FILE_HASH_CACHE
            .lock()
            .expect("transfer file hash cache lock poisoned");
        if let Some(cached) = cache.get(&cache_key) {
            if cached.length == metadata.len() && cached.modified == modified {
                return Ok(Some((cache_key, cached.hash.clone())));
            }
        }
    }

    use sha2::Digest;
    let file = File::open(&cache_key).map_err(|error| format!("open local file: {error}"))?;
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let mut digest = sha2::Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("hash local file: {error}"))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }

    let current_metadata = std::fs::metadata(&cache_key)
        .map_err(|error| format!("recheck local file metadata: {error}"))?;
    if current_metadata.len() != metadata.len() || current_metadata.modified().ok() != modified {
        return Err("local file changed while hashing".into());
    }

    let hash = format!("{:x}", digest.finalize());
    if modified.is_some() {
        ORIGINAL_FILE_HASH_CACHE
            .lock()
            .expect("transfer file hash cache lock poisoned")
            .insert(
                cache_key.clone(),
                CachedFileHash {
                    length: metadata.len(),
                    modified,
                    hash: hash.clone(),
                },
            );
    }
    Ok(Some((cache_key, hash)))
}
fn upload_file(
    backend: &dyn SyncBackend,
    db: &Database,
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
    let source_size = std::fs::metadata(source)
        .map_err(|error| format!("read file metadata: {error}"))?
        .len();
    if source_size > MAX_TRANSFER_FILE_SIZE_BYTES {
        return Err(format!(
            "file exceeds the {} MiB transfer limit",
            MAX_TRANSFER_FILE_SIZE_BYTES / 1024 / 1024
        ));
    }
    let source_modified = std::fs::metadata(source)
        .and_then(|metadata| metadata.modified())
        .ok();
    let digest = digest_file(source, MAX_TRANSFER_FILE_SIZE_BYTES)?;
    if digest.size != source_size || !file_metadata_matches(source, source_size, source_modified) {
        return Err("local file changed while preparing the upload".into());
    }
    let hash = digest.sha256.clone();
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
        blob_id: format!("{hash}-{}", uuid::Uuid::new_v4()),
        name: name.clone(),
        ext: ext.clone(),
        size: digest.size,
        uploaded_at: now.to_rfc3339(),
        expires_at,
        uploaded_by: device_name.to_string(),
        pinned: false,
    };
    entry.validate()?;

    let source_file = File::open(source).map_err(|error| format!("open file: {error}"))?;
    let mut upload_reader =
        DigestingReader::new(BufReader::with_capacity(STREAM_BUFFER_BYTES, source_file));
    let upload_result =
        backend.upload_file_blob(entry.blob_key(), &ext, &mut upload_reader, digest.size);
    let uploaded_digest = upload_reader.finish();
    if let Err(error) = upload_result {
        if let Err(cleanup_error) = backend.delete_file_blob(entry.blob_key(), &entry.ext) {
            log::warn!("[transfer] failed partial upload cleanup: {cleanup_error}");
        }
        return Err(error);
    }
    if uploaded_digest != digest || !file_metadata_matches(source, source_size, source_modified) {
        if let Err(cleanup_error) = backend.delete_file_blob(entry.blob_key(), &entry.ext) {
            log::warn!("[transfer] changed upload cleanup failed: {cleanup_error}");
        }
        return Err("local file changed while uploading".into());
    }
    let replaced = match mutate_manifest(backend, |manifest| {
        let replaced = manifest
            .files
            .iter()
            .find(|existing| existing.hash == hash)
            .cloned();
        manifest.files.retain(|existing| existing.hash != hash);
        manifest.files.push(entry.clone());
        manifest.device_name = device_name.to_string();
        replaced
    }) {
        Ok(replaced) => replaced,
        Err(error) => {
            if let Err(cleanup_error) = backend.delete_file_blob(entry.blob_key(), &entry.ext) {
                log::warn!("[transfer] failed upload rollback cleanup: {cleanup_error}");
            }
            return Err(error);
        }
    };
    if let Some(replaced) = replaced.filter(|old| old.blob_key() != entry.blob_key()) {
        if let Err(error) = backend.delete_file_blob(replaced.blob_key(), &replaced.ext) {
            log::warn!("[transfer] replaced blob cleanup failed: {error}");
        }
    }

    let canonical_path = std::fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    upsert_transfer_item(
        db,
        &entry,
        &canonical_path.to_string_lossy(),
        digest.content_hash,
    )?;
    Ok(name)
}

fn download_file(
    backend: &dyn SyncBackend,
    db: &Database,
    entry: &ManifestEntry,
) -> Result<String, String> {
    entry.validate()?;
    let directory = paths::transfer_cache_dir().join(&entry.hash);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create transfer cache: {error}"))?;
    let destination = directory.join(&entry.name);
    let temporary = directory.join(format!(".download-{}.tmp", uuid::Uuid::new_v4()));
    let download_result = stream_blob_to_file(backend, entry, &temporary);
    let digest = match download_result {
        Ok(digest) => digest,
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
    };
    if let Err(error) = validate_download_digest(entry, &digest) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    crate::services::file_ops::replace_file(&temporary, &destination).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("replace cache file: {error}")
    })?;
    remove_stale_cache_files(&directory, &destination);
    upsert_transfer_item(
        db,
        entry,
        &destination.to_string_lossy(),
        digest.content_hash,
    )?;
    Ok(destination.to_string_lossy().into_owned())
}

fn delete_file(
    backend: &dyn SyncBackend,
    db: &Database,
    entry: &ManifestEntry,
) -> Result<(), String> {
    entry.validate()?;
    let blob_key = entry.blob_key().to_string();
    let removed = mutate_manifest(backend, |manifest| {
        let before = manifest.files.len();
        manifest.files.retain(|existing| {
            existing.hash != entry.hash || existing.blob_key() != entry.blob_key()
        });
        before != manifest.files.len()
    })?;
    if !removed {
        return Err(I18nKey::TransferEntryExpired.text().into());
    }
    if let Err(error) = backend.delete_file_blob(&blob_key, &entry.ext) {
        log::warn!("[transfer] orphaned blob cleanup failed after manifest deletion: {error}");
    }
    detach_local_transfer(db, &entry.hash);
    Ok(())
}

/// Flip the pinned flag on the manifest entry that matches both `hash` and
/// the upload generation (`blob_key()`), so a stale click can never modify a
/// newer blob with identical content. Unpinning resets `expires_at` to a full
/// retention window measured from now.
fn set_pinned_file(
    backend: &dyn SyncBackend,
    entry: &ManifestEntry,
    pinned: bool,
    retention_days: u32,
) -> Result<(), String> {
    entry.validate()?;
    let target_hash = entry.hash.clone();
    let target_blob_key = entry.blob_key().to_string();
    let updated = mutate_manifest(backend, |manifest| {
        let Some(target) = manifest.files.iter_mut().find(|existing| {
            existing.hash == target_hash && existing.blob_key() == target_blob_key
        }) else {
            return false;
        };
        target.pinned = pinned;
        if !pinned {
            target.expires_at = if retention_days == 0 {
                String::new()
            } else {
                (chrono::Utc::now() + chrono::Duration::days(retention_days as i64)).to_rfc3339()
            };
        }
        true
    })?;
    if !updated {
        return Err(I18nKey::TransferEntryExpired.text().into());
    }
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
    let expired_keys: HashSet<String> = expired
        .iter()
        .map(|entry| entry.blob_key().to_string())
        .collect();
    let removed = mutate_manifest(backend, |manifest| {
        let mut removed = Vec::new();
        manifest.files.retain(|entry| {
            if expired_keys.contains(entry.blob_key()) && entry_expired(entry, now, retention_days)
            {
                removed.push(entry.clone());
                false
            } else {
                true
            }
        });
        removed
    })?;
    for entry in &removed {
        if let Err(error) = backend.delete_file_blob(entry.blob_key(), &entry.ext) {
            log::warn!("[transfer] expired blob cleanup failed: {error}");
        }
        clean_local_transfer(db, &entry.hash);
    }
    Ok(removed.len() as u32)
}

fn entry_expired(
    entry: &ManifestEntry,
    now: chrono::DateTime<chrono::Utc>,
    retention_days: u32,
) -> bool {
    // Pinned entries never expire automatically. Everything else shares the
    // same `effective_expiration` rules as the UI remaining-time projection.
    !entry.pinned
        && effective_expiration(entry, retention_days).is_some_and(|expires| expires <= now)
}

fn mutate_manifest<T>(
    backend: &dyn SyncBackend,
    mutate: impl Fn(&mut FileManifest) -> T,
) -> Result<T, String> {
    for _ in 0..MANIFEST_UPDATE_RETRIES {
        let mut snapshot = backend.pull_file_manifest()?;
        if snapshot.manifest.version > migration::TRANSFER_PROTOCOL_VERSION {
            return Err(I18nKey::TransferProtocolUnsupported.text().into());
        }
        migration::migrate_file_manifest(&mut snapshot.manifest);
        validate_manifest(&snapshot.manifest)?;
        let value = mutate(&mut snapshot.manifest);
        validate_manifest(&snapshot.manifest)?;
        snapshot.manifest.version = migration::TRANSFER_PROTOCOL_VERSION;
        snapshot.manifest.updated_at = chrono::Utc::now().to_rfc3339();
        match backend.push_file_manifest(&snapshot.manifest, snapshot.revision.as_deref()) {
            Ok(_) => return Ok(value),
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
    if name != value {
        return Err("file name is not portable across supported platforms".into());
    }
    validate_portable_file_name(name)?;
    Ok(name.to_string())
}

fn get_local_transfer_paths(db: &Database) -> HashMap<String, String> {
    db.get_transfer_items()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            let file_data = FileData::from_json(&item.file_data);
            let path = file_data.files.first()?.path.clone();
            // The database owns local-item validity. Avoid touching the path here:
            // cloud placeholders, offline volumes, and network shares can make a
            // simple existence check block manifest refresh for seconds.
            (!file_data.remote_hash.is_empty()).then_some((file_data.remote_hash, path))
        })
        .collect()
}

fn validate_manifest(manifest: &FileManifest) -> Result<(), String> {
    let mut hashes = HashSet::new();
    for entry in &manifest.files {
        entry.validate()?;
        if !hashes.insert(entry.hash.as_str()) {
            return Err(format!("duplicate transfer hash {}", entry.hash));
        }
    }
    Ok(())
}

fn reconcile_transfer_records(db: &Database, active_hashes: &HashSet<String>) -> usize {
    reconcile_transfer_records_in(db, active_hashes, &paths::transfer_cache_dir())
}

fn reconcile_transfer_records_in(
    db: &Database,
    active_hashes: &HashSet<String>,
    cache_root: &Path,
) -> usize {
    let stale_hashes: HashSet<String> = db
        .get_transfer_items()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            let hash = FileData::from_json(&item.file_data).remote_hash;
            (!hash.is_empty() && !active_hashes.contains(&hash)).then_some(hash)
        })
        .collect();
    for hash in &stale_hashes {
        detach_local_transfer_in(db, hash, cache_root);
    }
    stale_hashes.len()
}

fn remove_stale_cache_files(directory: &Path, destination: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path != destination && path.is_file() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn file_metadata_matches(
    path: &Path,
    expected_size: u64,
    expected_modified: Option<SystemTime>,
) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| {
        metadata.is_file()
            && metadata.len() == expected_size
            && metadata.modified().ok() == expected_modified
    })
}

fn digest_file(path: &Path, max_bytes: u64) -> Result<FileDigest, String> {
    let file = File::open(path).map_err(|error| format!("open file for hashing: {error}"))?;
    let mut reader = DigestingReader::new(BufReader::with_capacity(STREAM_BUFFER_BYTES, file));
    let copied = std::io::copy(
        &mut reader.by_ref().take(max_bytes.saturating_add(1)),
        &mut std::io::sink(),
    )
    .map_err(|error| format!("hash file: {error}"))?;
    let digest = reader.finish();
    if copied > max_bytes {
        return Err(format!(
            "file exceeds the {} MiB transfer limit",
            MAX_TRANSFER_FILE_SIZE_BYTES / 1024 / 1024
        ));
    }
    Ok(digest)
}

fn stream_blob_to_file(
    backend: &dyn SyncBackend,
    entry: &ManifestEntry,
    destination: &Path,
) -> Result<FileDigest, String> {
    let file = File::create(destination).map_err(|error| format!("create cache temp: {error}"))?;
    let mut writer = DigestingWriter::new(file);
    let backend_result = backend.download_file_blob(
        entry.blob_key(),
        &entry.ext,
        &mut writer,
        MAX_TRANSFER_FILE_SIZE_BYTES,
    );
    let finish_result = writer
        .finish()
        .map_err(|error| format!("flush cache temp: {error}"));
    let copied = backend_result?;
    let (file, digest) = finish_result?;
    if copied != digest.size {
        return Err(format!(
            "download length mismatch: backend reported {copied}, wrote {}",
            digest.size
        ));
    }
    file.sync_all()
        .map_err(|error| format!("sync cache temp: {error}"))?;
    Ok(digest)
}

fn validate_download_digest(entry: &ManifestEntry, digest: &FileDigest) -> Result<(), String> {
    if digest.size != entry.size || digest.sha256 != entry.hash {
        Err("downloaded file failed integrity verification".into())
    } else {
        Ok(())
    }
}

fn upsert_transfer_item(
    db: &Database,
    entry: &ManifestEntry,
    local_path: &str,
    content_hash: u64,
) -> Result<(), String> {
    let now = chrono::Utc::now();
    let item = ClipboardItem {
        id: 0,
        content_type: ContentType::File,
        full_text: entry.name.clone(),
        content_hash,
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

fn transfer_hash_directory_in(cache_root: &Path, hash: &str) -> Option<PathBuf> {
    let safe_hash = hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    safe_hash.then(|| cache_root.join(hash))
}

fn transfer_hash_directory(hash: &str) -> Option<PathBuf> {
    transfer_hash_directory_in(&paths::transfer_cache_dir(), hash)
}

/// Remove the remote association while preserving a downloaded local file.
/// Upload backing rows point at the user's original file and are simply
/// discarded because the ordinary clipboard row already owns that path.
fn detach_local_transfer(db: &Database, hash: &str) {
    detach_local_transfer_in(db, hash, &paths::transfer_cache_dir());
}

fn detach_local_transfer_in(db: &Database, hash: &str, cache_root: &Path) {
    let hash_dir = transfer_hash_directory_in(cache_root, hash);
    if let Ok(items) = db.get_transfer_items() {
        for item in items {
            let mut file_data = FileData::from_json(&item.file_data);
            if file_data.remote_hash != hash || file_data.files.len() != 1 {
                continue;
            }
            let local_path = PathBuf::from(&file_data.files[0].path);
            let downloaded_cache_exists = hash_dir.as_ref().is_some_and(|directory| {
                local_path.parent() == Some(directory.as_path()) && local_path.is_file()
            });
            if !downloaded_cache_exists {
                continue;
            }

            let mut content_hasher = std::collections::hash_map::DefaultHasher::new();
            file_data.files[0].path.hash(&mut content_hasher);
            file_data.transfer = false;
            file_data.remote_hash.clear();
            if let Err(error) =
                db.detach_transfer_item(item.id, content_hasher.finish(), &file_data.to_json())
            {
                log::warn!("[transfer] failed to preserve downloaded file record: {error}");
            }
        }
    }
    let _ = db.delete_transfer_by_hash(hash);
}

fn clean_local_transfer(db: &Database, hash: &str) {
    let hash_dir = transfer_hash_directory(hash);
    if let Ok(items) = db.get_transfer_items() {
        for item in items {
            let file_data = FileData::from_json(&item.file_data);
            if file_data.remote_hash != hash {
                continue;
            }
            for file in file_data.files {
                let path = PathBuf::from(file.path);
                if hash_dir
                    .as_ref()
                    .is_some_and(|directory| path.parent() == Some(directory.as_path()))
                {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
    if let Some(hash_dir) = hash_dir.filter(|directory| directory.is_dir()) {
        let _ = std::fs::remove_dir(hash_dir);
    }
    let _ = db.delete_transfer_by_hash(hash);
}

#[cfg(test)]
fn compute_file_hash(data: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(data);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
fn default_hash(data: &[u8]) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(data);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::sync::BackendStatus;
    use std::sync::atomic::AtomicUsize;

    struct ConflictManifestBackend {
        manifest: Mutex<FileManifest>,
        conflicts_remaining: AtomicUsize,
        pushes: AtomicUsize,
    }

    impl ConflictManifestBackend {
        fn new(conflicts: usize) -> Self {
            Self {
                manifest: Mutex::new(FileManifest {
                    version: migration::TRANSFER_PROTOCOL_VERSION,
                    device_name: String::new(),
                    updated_at: String::new(),
                    files: Vec::new(),
                }),
                conflicts_remaining: AtomicUsize::new(conflicts),
                pushes: AtomicUsize::new(0),
            }
        }
    }

    impl SyncBackend for ConflictManifestBackend {
        fn check_status(&self) -> BackendStatus {
            BackendStatus::Online
        }

        fn pull(&self, _bypass_cache: bool) -> Result<crate::core::sync::SyncPayload, String> {
            Err("unused".into())
        }

        fn push(&self, _payload: &crate::core::sync::SyncPayload) -> Result<(), String> {
            Err("unused".into())
        }

        fn sync_interval(&self) -> u64 {
            60
        }

        fn pull_file_manifest(
            &self,
        ) -> Result<crate::core::transfer_types::ManifestSnapshot, String> {
            Ok(crate::core::transfer_types::ManifestSnapshot {
                manifest: self.manifest.lock().expect("manifest lock").clone(),
                revision: Some(format!("r{}", self.pushes.load(Ordering::SeqCst))),
            })
        }

        fn push_file_manifest(
            &self,
            manifest: &FileManifest,
            _expected_revision: Option<&str>,
        ) -> Result<String, ManifestWriteError> {
            if self
                .conflicts_remaining
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    if remaining > 0 {
                        Some(remaining - 1)
                    } else {
                        None
                    }
                })
                .is_ok()
            {
                return Err(ManifestWriteError::Conflict);
            }
            *self.manifest.lock().expect("manifest lock") = manifest.clone();
            let push = self.pushes.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(format!("r{push}"))
        }
    }

    fn valid_entry(hash_byte: char, name: &str) -> ManifestEntry {
        let hash = hash_byte.to_string().repeat(64);
        ManifestEntry {
            blob_id: format!("{hash}-{}", uuid::Uuid::new_v4()),
            hash,
            name: name.into(),
            ext: "bin".into(),
            size: 4,
            uploaded_at: chrono::Utc::now().to_rfc3339(),
            expires_at: String::new(),
            uploaded_by: "test".into(),
            pinned: false,
        }
    }

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
    fn automatic_refresh_runs_only_for_a_visible_transfer_view() {
        assert_eq!(
            automatic_refresh_interval(true),
            Some(Duration::from_secs(2))
        );
        assert_eq!(automatic_refresh_interval(false), None);
    }

    #[test]
    fn refresh_indicator_is_only_visible_for_the_entry_refresh() {
        assert!(refresh_indicator_visible(&TransferCommand::Refresh, true));
        assert!(!refresh_indicator_visible(&TransferCommand::Refresh, false));
        assert!(!refresh_indicator_visible(&TransferCommand::Cleanup, true));
    }

    #[test]
    fn unchanged_refresh_keeps_the_existing_resolved_list() {
        let resolved = ResolvedEntry {
            entry: valid_entry('a', "same.bin"),
            is_local: false,
            local_path: None,
        };
        let mut current = vec![resolved.clone()];
        let mut has_transfer_files = true;

        assert!(!update_resolved_entries(
            &mut current,
            &mut has_transfer_files,
            vec![resolved.clone()],
        ));
        assert_eq!(current, vec![resolved]);
        assert!(has_transfer_files);
    }

    #[test]
    fn streaming_digest_matches_whole_buffer_hashes_and_enforces_limit() {
        let directory = std::env::temp_dir().join(format!(
            "clippi-stream-digest-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("large.bin");
        let data: Vec<u8> = (0..STREAM_BUFFER_BYTES * 2 + 137)
            .map(|index| (index % 251) as u8)
            .collect();
        std::fs::write(&path, &data).unwrap();

        let digest = digest_file(&path, data.len() as u64).unwrap();
        assert_eq!(digest.size, data.len() as u64);
        assert_eq!(digest.sha256, compute_file_hash(&data));
        assert_eq!(digest.content_hash, default_hash(&data));
        assert!(digest_file(&path, data.len() as u64 - 1).is_err());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn streamed_download_digest_rejects_corrupt_content_and_wrong_length() {
        let data = b"verified streamed download";
        let mut entry = valid_entry('a', "download.bin");
        entry.hash = compute_file_hash(data);
        entry.blob_id = format!("{}-{}", entry.hash, uuid::Uuid::new_v4());
        entry.size = data.len() as u64;
        let digest = FileDigest {
            size: data.len() as u64,
            sha256: compute_file_hash(data),
            content_hash: default_hash(data),
        };
        assert!(validate_download_digest(&entry, &digest).is_ok());

        let mut corrupt = digest.clone();
        corrupt.sha256 = compute_file_hash(b"different content");
        assert!(validate_download_digest(&entry, &corrupt).is_err());

        let mut truncated = digest;
        truncated.size -= 1;
        assert!(validate_download_digest(&entry, &truncated).is_err());
    }

    #[test]
    fn manifest_entry_rejects_path_traversal() {
        let entry = ManifestEntry {
            hash: "a".repeat(64),
            blob_id: String::new(),
            name: "../file.txt".into(),
            ext: "txt".into(),
            size: 1,
            uploaded_at: chrono::Utc::now().to_rfc3339(),
            expires_at: String::new(),
            uploaded_by: String::new(),
            pinned: false,
        };
        assert!(entry.validate().is_err());
    }

    #[test]
    fn manifest_validation_rejects_noncanonical_hashes_timestamps_and_duplicates() {
        let mut entry = valid_entry('a', "file.bin");
        entry.hash = "A".repeat(64);
        assert!(entry.validate().is_err());

        let mut entry = valid_entry('a', "file.bin");
        entry.uploaded_at = "not-a-time".into();
        assert!(entry.validate().is_err());

        let mut entry = valid_entry('a', "file.bin");
        entry.size = MAX_TRANSFER_FILE_SIZE_BYTES + 1;
        assert!(entry.validate().is_err());

        let entry = valid_entry('a', "file.bin");
        let manifest = FileManifest {
            version: migration::TRANSFER_PROTOCOL_VERSION,
            device_name: "test".into(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            files: vec![entry.clone(), entry],
        };
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn manifest_mutation_retries_conflicts_and_stops_at_the_limit() {
        let backend = ConflictManifestBackend::new(2);
        mutate_manifest(&backend, |manifest| {
            manifest.files = vec![valid_entry('a', "file.bin")];
        })
        .unwrap();
        assert_eq!(backend.pushes.load(Ordering::SeqCst), 1);
        assert_eq!(backend.manifest.lock().unwrap().files.len(), 1);

        let backend = ConflictManifestBackend::new(MANIFEST_UPDATE_RETRIES);
        let error = mutate_manifest(&backend, |manifest| {
            manifest.files = vec![valid_entry('b', "other.bin")];
        })
        .unwrap_err();
        assert!(error.contains("changed repeatedly"));
        assert_eq!(backend.pushes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn expiry_prefers_explicit_timestamp_and_supports_legacy_entries() {
        let now = chrono::Utc::now();
        let mut entry = ManifestEntry {
            hash: "a".repeat(64),
            blob_id: String::new(),
            name: "file.txt".into(),
            ext: "txt".into(),
            size: 1,
            uploaded_at: (now - chrono::Duration::days(2)).to_rfc3339(),
            expires_at: (now + chrono::Duration::hours(1)).to_rfc3339(),
            uploaded_by: String::new(),
            pinned: false,
        };
        assert!(!entry_expired(&entry, now, 1));

        entry.expires_at.clear();
        assert!(entry_expired(&entry, now, 1));
        assert!(!entry_expired(&entry, now, 3));
    }

    #[test]
    fn pinned_entries_never_expire_even_after_their_timestamp() {
        let now = chrono::Utc::now();
        let mut entry = valid_entry('a', "pinned.bin");
        entry.pinned = true;
        entry.uploaded_at = (now - chrono::Duration::days(10)).to_rfc3339();
        entry.expires_at = (now - chrono::Duration::days(3)).to_rfc3339();
        assert!(!entry_expired(&entry, now, 3));

        entry.pinned = false;
        assert!(entry_expired(&entry, now, 3));
    }

    #[test]
    fn set_pinned_only_touches_matching_hash_and_generation() {
        let backend = ConflictManifestBackend::new(0);
        let current = valid_entry('a', "current.bin");
        let hash = current.hash.clone();
        let current_blob = current.blob_key().to_string();

        // A stale click referencing an older upload generation of the same
        // content must not modify the current entry.
        let mut stale = valid_entry('b', "stale.bin");
        stale.hash = hash.clone();
        stale.blob_id = format!("{}-{}", stale.hash, uuid::Uuid::new_v4());
        mutate_manifest(&backend, |manifest| {
            manifest.files = vec![current.clone()];
        })
        .unwrap();

        let error = set_pinned_file(&backend, &stale, true, 3).unwrap_err();
        assert_eq!(error, I18nKey::TransferEntryExpired.text());
        assert!(!backend.manifest.lock().unwrap().files[0].pinned);

        // The exact current generation can be pinned.
        set_pinned_file(&backend, &current, true, 3).unwrap();
        let stored = backend.manifest.lock().unwrap().files[0].clone();
        assert!(stored.pinned);
        assert_eq!(stored.blob_key(), current_blob);
    }

    #[test]
    fn set_pinned_on_missing_entry_reports_expired() {
        let backend = ConflictManifestBackend::new(0);
        mutate_manifest(&backend, |manifest| {
            manifest.files = vec![valid_entry('a', "kept.bin")];
        })
        .unwrap();
        let missing = valid_entry('b', "gone.bin");
        let error = set_pinned_file(&backend, &missing, true, 3).unwrap_err();
        assert_eq!(error, I18nKey::TransferEntryExpired.text());
    }

    #[test]
    fn unpin_resets_expiration_from_now_and_respects_retention_off() {
        let now = chrono::Utc::now();
        let mut entry = valid_entry('a', "unpin.bin");
        entry.expires_at = (now - chrono::Duration::days(30)).to_rfc3339();

        let backend = ConflictManifestBackend::new(0);
        mutate_manifest(&backend, |manifest| {
            manifest.files = vec![entry.clone()];
        })
        .unwrap();

        set_pinned_file(&backend, &entry, true, 3).unwrap();
        set_pinned_file(&backend, &entry, false, 3).unwrap();
        let stored = backend
            .manifest
            .lock()
            .unwrap()
            .files
            .iter()
            .find(|existing| existing.hash == entry.hash)
            .unwrap()
            .clone();
        assert!(!stored.pinned);
        let expires = chrono::DateTime::parse_from_rfc3339(&stored.expires_at)
            .unwrap()
            .with_timezone(&chrono::Utc);
        let remaining = expires - now;
        assert!(remaining >= chrono::Duration::days(2));
        assert!(remaining <= chrono::Duration::days(3) + chrono::Duration::minutes(5));

        // Retention disabled: unpinning leaves the expiration empty (forever).
        set_pinned_file(&backend, &entry, true, 0).unwrap();
        set_pinned_file(&backend, &entry, false, 0).unwrap();
        let stored = backend
            .manifest
            .lock()
            .unwrap()
            .files
            .iter()
            .find(|existing| existing.hash == entry.hash)
            .unwrap()
            .clone();
        assert!(stored.expires_at.is_empty());
    }

    #[test]
    fn original_file_marker_is_derived_from_manifest_content_hash() {
        let unique = format!(
            "clippi-transfer-marker-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let directory = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&directory).unwrap();
        let file_path = directory.join("report.bin");
        let contents = b"passive transfer marker";
        std::fs::write(&file_path, contents).unwrap();

        let db_path = directory.join("clipboard.db");
        let db = Database::open(&db_path.to_string_lossy()).unwrap();
        let now = chrono::Utc::now();
        let content_hash = default_hash(contents);
        db.upsert(&ClipboardItem {
            id: 0,
            content_type: ContentType::File,
            full_text: "report.bin".into(),
            content_hash,
            created_at: now,
            updated_at: now,
            image_path: String::new(),
            image_width: 0,
            image_height: 0,
            rich_data: String::new(),
            file_data: FileData {
                files: vec![FileInfo {
                    name: "report.bin".into(),
                    path: file_path.to_string_lossy().into_owned(),
                    is_dir: false,
                }],
                transfer: false,
                remote_hash: "legacy-active-marker".into(),
            }
            .to_json(),
            is_favorite: false,
            note: String::new(),
            source_app_name: String::new(),
            source_app_icon: String::new(),
            size: contents.len() as i64,
            tags: Vec::new(),
            meta_type: String::new(),
            custom_hotkey: String::new(),
            custom_hotkey_format: String::new(),
        })
        .unwrap();

        let sha256 = compute_file_hash(contents);
        let active_hashes = HashSet::from([sha256.clone()]);
        let active_sizes = HashSet::from([contents.len() as u64]);
        assert_eq!(
            reconcile_original_file_markers(&db, &active_hashes, &active_sizes).unwrap(),
            1
        );
        let item = db.get_by_hash(content_hash).unwrap().unwrap();
        assert_eq!(FileData::from_json(&item.file_data).remote_hash, sha256);
        assert_eq!(
            reconcile_original_file_markers(&db, &active_hashes, &active_sizes).unwrap(),
            0
        );

        assert_eq!(
            reconcile_original_file_markers(&db, &HashSet::new(), &HashSet::new()).unwrap(),
            1
        );
        let item = db.get_by_hash(content_hash).unwrap().unwrap();
        assert!(FileData::from_json(&item.file_data).remote_hash.is_empty());

        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn refresh_removes_backing_records_for_remote_entries_that_disappeared() {
        let directory = std::env::temp_dir().join(format!(
            "clippi-transfer-stale-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let source = directory.join("source.bin");
        let data = b"kept user source";
        std::fs::write(&source, data).unwrap();
        let db_path = directory.join("clipboard.db");
        let db = Database::open(&db_path.to_string_lossy()).unwrap();
        let mut entry = valid_entry('a', "source.bin");
        entry.hash = compute_file_hash(data);
        entry.blob_id = format!("{}-{}", entry.hash, uuid::Uuid::new_v4());
        entry.size = data.len() as u64;
        upsert_transfer_item(&db, &entry, &source.to_string_lossy(), default_hash(data)).unwrap();

        assert_eq!(db.get_transfer_items().unwrap().len(), 1);
        assert_eq!(reconcile_transfer_records(&db, &HashSet::new()), 1);
        assert!(db.get_transfer_items().unwrap().is_empty());
        assert!(source.is_file());

        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn local_transfer_resolution_trusts_the_database_without_touching_the_path() {
        let directory = std::env::temp_dir().join(format!(
            "clippi-transfer-db-resolution-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let db_path = directory.join("clipboard.db");
        let db = Database::open(&db_path.to_string_lossy()).unwrap();
        let missing_path = directory.join("offline-volume").join("file.bin");
        let entry = valid_entry('a', "file.bin");
        upsert_transfer_item(&db, &entry, &missing_path.to_string_lossy(), 123).unwrap();

        let paths = get_local_transfer_paths(&db);
        assert_eq!(
            paths.get(&entry.hash).map(String::as_str),
            Some(missing_path.to_string_lossy().as_ref())
        );

        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn remote_deletion_preserves_downloaded_cache_as_an_ordinary_file() {
        let directory = std::env::temp_dir().join(format!(
            "clippi-transfer-detach-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let data = uuid::Uuid::new_v4().to_string().into_bytes();
        let hash = compute_file_hash(&data);
        let cache_root = directory.join("file_cache");
        let hash_dir = cache_root.join(&hash);
        std::fs::create_dir_all(&hash_dir).unwrap();
        let cached_file = hash_dir.join("download.bin");
        std::fs::write(&cached_file, &data).unwrap();

        let db_path = directory.join("clipboard.db");
        let db = Database::open(&db_path.to_string_lossy()).unwrap();
        let mut entry = valid_entry('a', "download.bin");
        entry.hash = hash.clone();
        entry.blob_id = format!("{}-{}", entry.hash, uuid::Uuid::new_v4());
        entry.size = data.len() as u64;
        upsert_transfer_item(
            &db,
            &entry,
            &cached_file.to_string_lossy(),
            default_hash(&data),
        )
        .unwrap();
        let item_id = db.get_transfer_items().unwrap()[0].id;

        assert_eq!(
            reconcile_transfer_records_in(&db, &HashSet::new(), &cache_root),
            1
        );
        assert!(cached_file.is_file());
        assert!(db.get_transfer_items().unwrap().is_empty());
        let local_item = db.get_by_id(item_id).unwrap().unwrap();
        let local_file_data = FileData::from_json(&local_item.file_data);
        assert!(local_item.meta_type.is_empty());
        assert!(!local_file_data.transfer);
        assert!(local_file_data.remote_hash.is_empty());
        assert_eq!(local_file_data.files[0].path, cached_file.to_string_lossy());

        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
