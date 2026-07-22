//! --- Local-folder sync backend. ---
//!
//! Reads/writes `clippi_sync.json` in a cloud-synced folder (OneDrive, iCloud,
//! Dropbox, etc.). The OS/cloud-provider handles the actual network sync.

use crate::core::i18n_keys::I18nKey;
use crate::core::settings::BackendConfig;
use crate::core::sync::{self, BackendStatus, SyncBackend, SyncPayload};
use crate::core::transfer_types::{
    FileManifest, ManifestEntry, ManifestSnapshot, ManifestWriteError,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

const SYNC_FILENAME: &str = "clippi_sync.json";
const MANIFEST_FILENAME: &str = "clippi_files.json";
const MANIFEST_OPS_DIR: &str = "file_ops";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManifestOperation {
    version: u32,
    id: String,
    /// Lamport clock. Unlike wall-clock timestamps, this preserves causal
    /// ordering when Windows and macOS devices have skewed system clocks.
    #[serde(default)]
    logical_clock: u64,
    created_at: String,
    device_name: String,
    changes: Vec<ManifestChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ManifestChange {
    Upsert { entry: ManifestEntry },
    Delete { hash: String },
}

fn manifest_revision(manifest: &FileManifest) -> Result<String, String> {
    use sha2::Digest;
    let data =
        serde_json::to_vec(manifest).map_err(|error| format!("serialize manifest: {error}"))?;
    let digest = sha2::Sha256::digest(data);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub struct LocalFolderBackend {
    config: BackendConfig,
    /// Track remote file's last-modified time to skip unchanged pulls.
    /// This is a read-path optimization — content hash is the authoritative
    /// push-path gate (see `run_sync_cycle_for_backend`).
    last_remote_mtime: Mutex<Option<SystemTime>>,
}

impl LocalFolderBackend {
    pub fn new(config: BackendConfig) -> Self {
        let dir = PathBuf::from(&config.folder_path);
        let _ = std::fs::create_dir_all(&dir);
        Self {
            config,
            last_remote_mtime: Mutex::new(None),
        }
    }

    fn file_path(&self) -> PathBuf {
        PathBuf::from(&self.config.folder_path).join(SYNC_FILENAME)
    }

    /// Find conflict files matching `clippi_sync-*.json` older than 5 seconds
    /// (to skip files still being written by the cloud provider).
    fn find_conflicts(&self) -> Vec<PathBuf> {
        let dir = PathBuf::from(&self.config.folder_path);
        let mut conflicts = Vec::new();
        let now = SystemTime::now();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name != SYNC_FILENAME
                    && !name.starts_with('.')
                    && name.starts_with("clippi_sync")
                    && name.ends_with(".json")
                {
                    // --- Skip files modified within the last 5 seconds ---
                    if let Ok(meta) = path.metadata() {
                        if let Ok(mtime) = meta.modified() {
                            if now
                                .duration_since(mtime)
                                .map(|d| d.as_secs() < 5)
                                .unwrap_or(false)
                            {
                                continue;
                            }
                        }
                    }
                    conflicts.push(path);
                }
            }
        }
        conflicts
    }

    fn manifest_ops_dir(&self) -> PathBuf {
        PathBuf::from(&self.config.folder_path).join(MANIFEST_OPS_DIR)
    }

    fn read_legacy_manifest(&self) -> Result<Option<FileManifest>, String> {
        let path = self.manifest_path();
        if !path.exists() {
            return Ok(None);
        }
        let data = std::fs::read(&path).map_err(|error| format!("read manifest: {error}"))?;
        let mut manifest: FileManifest =
            serde_json::from_slice(&data).map_err(|error| format!("parse manifest: {error}"))?;
        if manifest.version > crate::core::migration::TRANSFER_PROTOCOL_VERSION {
            return Err(format!(
                "unsupported transfer manifest version {}",
                manifest.version
            ));
        }
        crate::core::migration::migrate_file_manifest(&mut manifest);
        Ok(Some(manifest))
    }

    fn read_manifest_operations(&self) -> Result<Vec<ManifestOperation>, String> {
        let directory = self.manifest_ops_dir();
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let mut operations_by_id = HashMap::new();
        let entries = std::fs::read_dir(&directory)
            .map_err(|error| format!("read manifest operation directory: {error}"))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("read manifest operation: {error}"))?;
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if !path.is_file() || !name.ends_with(".json") || name.starts_with('.') {
                continue;
            }
            let data = std::fs::read(&path)
                .map_err(|error| format!("read manifest operation {name}: {error}"))?;
            let operation: ManifestOperation = serde_json::from_slice(&data)
                .map_err(|error| format!("parse manifest operation {name}: {error}"))?;
            validate_manifest_operation(&operation)?;
            if let Some(existing) = operations_by_id.insert(operation.id.clone(), operation.clone())
            {
                if existing != operation {
                    return Err(format!(
                        "conflicting manifest operations share id {}",
                        operation.id
                    ));
                }
            }
        }
        let mut operations: Vec<_> = operations_by_id.into_values().collect();
        operations.sort_by(|left, right| {
            (left.logical_clock, &left.id).cmp(&(right.logical_clock, &right.id))
        });
        Ok(operations)
    }

    fn materialize_file_manifest(&self) -> Result<(FileManifest, bool), String> {
        let legacy = self.read_legacy_manifest()?;
        let operations = self.read_manifest_operations()?;
        let has_state = legacy.is_some() || !operations.is_empty();
        let mut files: HashMap<String, ManifestEntry> = if operations.is_empty() {
            legacy
                .as_ref()
                .map(|manifest| {
                    manifest
                        .files
                        .iter()
                        .cloned()
                        .map(|entry| (entry.hash.clone(), entry))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
        let mut updated_at = legacy
            .as_ref()
            .map(|manifest| manifest.updated_at.clone())
            .unwrap_or_default();
        let mut device_name = legacy
            .as_ref()
            .map(|manifest| manifest.device_name.clone())
            .unwrap_or_default();

        for operation in operations {
            updated_at = operation.created_at;
            device_name = operation.device_name;
            for change in operation.changes {
                match change {
                    ManifestChange::Upsert { entry } => {
                        files.insert(entry.hash.clone(), entry);
                    }
                    ManifestChange::Delete { hash } => {
                        files.remove(&hash);
                    }
                }
            }
        }

        let mut files: Vec<_> = files.into_values().collect();
        files.sort_by(|left, right| left.hash.cmp(&right.hash));
        Ok((
            FileManifest {
                version: crate::core::migration::TRANSFER_PROTOCOL_VERSION,
                device_name,
                updated_at,
                files,
            },
            has_state,
        ))
    }

    fn write_manifest_operation(
        &self,
        operation: &ManifestOperation,
    ) -> Result<(), ManifestWriteError> {
        let directory = self.manifest_ops_dir();
        std::fs::create_dir_all(&directory).map_err(|error| {
            ManifestWriteError::Other(format!("create manifest operation directory: {error}"))
        })?;
        let json = serde_json::to_vec_pretty(operation).map_err(|error| {
            ManifestWriteError::Other(format!("serialize manifest operation: {error}"))
        })?;
        let filename = format!("{}.json", operation.id);
        let destination = directory.join(&filename);
        let temporary = directory.join(format!(".{filename}.tmp"));
        std::fs::write(&temporary, json).map_err(|error| {
            ManifestWriteError::Other(format!("write manifest operation: {error}"))
        })?;
        std::fs::rename(&temporary, &destination).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            ManifestWriteError::Other(format!("commit manifest operation: {error}"))
        })
    }

    fn write_protocol_marker(&self, manifest: &FileManifest) {
        let root = PathBuf::from(&self.config.folder_path);
        let destination = self.manifest_path();
        let temporary = root.join(format!(".{MANIFEST_FILENAME}.v2.tmp"));
        let result = serde_json::to_vec_pretty(manifest)
            .map_err(|error| format!("serialize protocol marker: {error}"))
            .and_then(|data| {
                std::fs::write(&temporary, data)
                    .map_err(|error| format!("write protocol marker: {error}"))
            })
            .and_then(|()| {
                crate::services::file_ops::replace_file(&temporary, &destination)
                    .map_err(|error| format!("commit protocol marker: {error}"))
            });
        if let Err(error) = result {
            let _ = std::fs::remove_file(temporary);
            log::warn!("[transfer] {error}");
        }
    }
}

fn validate_manifest_operation(operation: &ManifestOperation) -> Result<(), String> {
    if operation.version != crate::core::migration::TRANSFER_PROTOCOL_VERSION {
        return Err(format!(
            "unsupported manifest operation version {}",
            operation.version
        ));
    }
    if operation.logical_clock == u64::MAX {
        return Err("manifest operation logical clock is out of range".into());
    }
    uuid::Uuid::parse_str(&operation.id).map_err(|_| "invalid manifest operation id")?;
    let timestamp = chrono::DateTime::parse_from_rfc3339(&operation.created_at)
        .map_err(|_| "invalid manifest operation timestamp")?;
    if timestamp.offset().local_minus_utc() != 0 {
        return Err("manifest operation timestamp must use UTC".into());
    }
    if operation.changes.is_empty() {
        return Err("manifest operation must contain changes".into());
    }
    for change in &operation.changes {
        match change {
            ManifestChange::Upsert { entry } => entry.validate()?,
            ManifestChange::Delete { hash } => validate_transfer_hash(hash)?,
        }
    }
    Ok(())
}

fn validate_transfer_hash(hash: &str) -> Result<(), String> {
    if hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err("invalid transfer hash".into())
    }
}

impl SyncBackend for LocalFolderBackend {
    fn sync_interval(&self) -> u64 {
        self.config.sync_interval_secs.unwrap_or(60)
    }

    fn check_status(&self) -> BackendStatus {
        let dir = PathBuf::from(&self.config.folder_path);
        if !dir.exists() {
            return BackendStatus::Offline;
        }
        if !dir.is_dir() {
            return BackendStatus::Error(I18nKey::SyncErrNotDir.text().into());
        }
        BackendStatus::Online
    }

    fn pull(&self, bypass_cache: bool) -> Result<SyncPayload, String> {
        let path = self.file_path();
        if !path.exists() {
            return Err(sync::SYNC_PULL_NOT_FOUND.into());
        }

        // Check if remote file has changed since last pull.
        // --- Only applies to the main file; conflict files always need processing. ---
        // --- When bypass_cache is true (local dirty, need to compare hashes), ---
        // --- force mtime_changed so we always read the file. ---
        let mut mtime_changed = bypass_cache;
        if !bypass_cache {
            if let Ok(meta) = std::fs::metadata(&path) {
                if let Ok(mtime) = meta.modified() {
                    let mut last = self
                        .last_remote_mtime
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if *last == Some(mtime) {
                        mtime_changed = false;
                    } else {
                        *last = Some(mtime);
                        mtime_changed = true;
                    }
                }
            }
        } else {
            // Still update the mtime cache so the next poll cycle can use it
            if let Ok(meta) = std::fs::metadata(&path) {
                if let Ok(mtime) = meta.modified() {
                    *self
                        .last_remote_mtime
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) = Some(mtime);
                }
            }
        }

        // --- Read main payload ---
        let mut payload = if mtime_changed {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| format!("{}: {e}", I18nKey::SyncErrRead.text()))?;
            serde_json::from_str::<SyncPayload>(&content)
                .map_err(|e| format!("{}: {e}", I18nKey::SyncErrParse.text()))?
        } else {
            // Main file unchanged — return early only if no conflicts either
            let conflicts = self.find_conflicts();
            if conflicts.is_empty() {
                return Err("@@unchanged".into());
            }
            // --- Re-read main payload to merge with conflicts ---
            let content = std::fs::read_to_string(&path)
                .map_err(|e| format!("{}: {e}", I18nKey::SyncErrRead.text()))?;
            serde_json::from_str::<SyncPayload>(&content)
                .map_err(|e| format!("{}: {e}", I18nKey::SyncErrParse.text()))?
        };

        // --- Merge conflict files ---
        let conflicts = self.find_conflicts();
        for conflict_path in &conflicts {
            match std::fs::read_to_string(conflict_path) {
                Ok(json) => match serde_json::from_str::<SyncPayload>(&json) {
                    Ok(conflict) => {
                        payload = sync::merge_payloads(payload, conflict);
                    }
                    Err(e) => {
                        log::error!("[sync] 冲突文件解析失败 {}: {e}", conflict_path.display());
                    }
                },
                Err(e) => {
                    log::error!("[sync] 冲突文件读取失败 {}: {e}", conflict_path.display());
                }
            }
        }

        Ok(payload)
    }

    fn push(&self, payload: &SyncPayload) -> Result<(), String> {
        let dir = PathBuf::from(&self.config.folder_path);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("{}: {e}", I18nKey::ErrCreateDir.text()))?;

        let file_path = self.file_path();
        let json = serde_json::to_string_pretty(payload)
            .map_err(|e| format!("{}: {e}", I18nKey::SyncErrSerialize.text()))?;

        // --- Atomic write: temp file + rename ---
        let tmp_path = dir.join(format!(".{SYNC_FILENAME}.tmp"));
        std::fs::write(&tmp_path, &json)
            .map_err(|e| format!("{}: {e}", I18nKey::SyncErrWriteTemp.text()))?;
        crate::services::file_ops::replace_file(&tmp_path, &file_path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            format!("{}: {e}", I18nKey::SyncErrReplace.text())
        })?;
        // --- Cache new mtime so our own push doesn't trigger a changed-file ---
        // --- detection on the next pull. ---
        if let Ok(meta) = std::fs::metadata(&file_path) {
            if let Ok(mtime) = meta.modified() {
                *self
                    .last_remote_mtime
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(mtime);
            }
        }
        Ok(())
    }

    fn post_push_cleanup(&self) -> Result<(), String> {
        for path in self.find_conflicts() {
            if let Err(e) = std::fs::remove_file(&path) {
                log::warn!("[sync] 清理冲突文件失败 {}: {e}", path.display());
            }
        }
        Ok(())
    }

    fn upload_blob(&self, hash_hex: &str, ext: &str, data: &[u8]) -> Result<(), String> {
        let dir = PathBuf::from(&self.config.folder_path).join("images");
        std::fs::create_dir_all(&dir).map_err(|e| format!("create images dir: {e}"))?;

        let file_path = dir.join(format!("{hash_hex}.{ext}"));
        // Atomic write: temp file + rename
        let tmp_path = dir.join(format!(".{hash_hex}.{ext}.tmp"));
        std::fs::write(&tmp_path, data).map_err(|e| format!("write blob temp: {e}"))?;
        crate::services::file_ops::replace_file(&tmp_path, &file_path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            format!("rename blob: {e}")
        })
    }

    fn download_blob(&self, hash_hex: &str, ext: &str) -> Result<Vec<u8>, String> {
        let file_path = PathBuf::from(&self.config.folder_path)
            .join("images")
            .join(format!("{hash_hex}.{ext}"));
        std::fs::read(&file_path).map_err(|e| format!("read blob: {e}"))
    }

    fn list_remote_blobs(&self) -> Result<Vec<String>, String> {
        let dir = PathBuf::from(&self.config.folder_path).join("images");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                if let Some(name) = entry.file_name().to_str() {
                    // Skip temp files
                    if name.starts_with('.') && name.ends_with(".tmp") {
                        continue;
                    }
                    files.push(name.to_string());
                }
            }
        }
        Ok(files)
    }

    // --- ── Transfer station file manifest ── ---

    fn pull_file_manifest(&self) -> Result<ManifestSnapshot, String> {
        let (manifest, has_state) = self.materialize_file_manifest()?;
        let revision = has_state
            .then(|| manifest_revision(&manifest))
            .transpose()?;
        Ok(ManifestSnapshot { manifest, revision })
    }

    fn push_file_manifest(
        &self,
        manifest: &FileManifest,
        expected_revision: Option<&str>,
    ) -> Result<String, ManifestWriteError> {
        let (current, has_state) = self
            .materialize_file_manifest()
            .map_err(ManifestWriteError::Other)?;
        let current_revision = has_state
            .then(|| manifest_revision(&current))
            .transpose()
            .map_err(ManifestWriteError::Other)?;
        if current_revision.as_deref() != expected_revision {
            return Err(ManifestWriteError::Conflict);
        }

        let operations = self
            .read_manifest_operations()
            .map_err(ManifestWriteError::Other)?;
        let has_operations = !operations.is_empty();
        let logical_clock = operations
            .iter()
            .map(|operation| operation.logical_clock)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| {
                ManifestWriteError::Other("manifest operation clock exhausted".into())
            })?;

        let current_by_hash: HashMap<_, _> = current
            .files
            .iter()
            .map(|entry| (entry.hash.as_str(), entry))
            .collect();
        let requested_by_hash: HashMap<_, _> = manifest
            .files
            .iter()
            .map(|entry| (entry.hash.as_str(), entry))
            .collect();
        let mut changes = Vec::new();
        for entry in &manifest.files {
            entry.validate().map_err(ManifestWriteError::Other)?;
            if !has_operations || current_by_hash.get(entry.hash.as_str()).copied() != Some(entry) {
                changes.push(ManifestChange::Upsert {
                    entry: entry.clone(),
                });
            }
        }
        for entry in &current.files {
            if !requested_by_hash.contains_key(entry.hash.as_str()) {
                changes.push(ManifestChange::Delete {
                    hash: entry.hash.clone(),
                });
            }
        }
        if changes.is_empty() {
            return manifest_revision(&current).map_err(ManifestWriteError::Other);
        }

        let operation = ManifestOperation {
            version: crate::core::migration::TRANSFER_PROTOCOL_VERSION,
            id: uuid::Uuid::new_v4().to_string(),
            logical_clock,
            created_at: chrono::Utc::now().to_rfc3339(),
            device_name: manifest.device_name.clone(),
            changes,
        };
        self.write_manifest_operation(&operation)?;
        let (updated, _) = self
            .materialize_file_manifest()
            .map_err(ManifestWriteError::Other)?;
        self.write_protocol_marker(&updated);
        manifest_revision(&updated).map_err(ManifestWriteError::Other)
    }

    // --- ── Transfer station file blobs ── ---

    fn upload_file_blob(&self, hash: &str, _ext: &str, data: &[u8]) -> Result<(), String> {
        let dir = PathBuf::from(&self.config.folder_path).join("files");
        std::fs::create_dir_all(&dir).map_err(|e| format!("create files dir: {e}"))?;

        let file_path = dir.join(hash);
        let tmp_path = dir.join(format!(".{hash}.tmp"));
        std::fs::write(&tmp_path, data).map_err(|e| format!("write file blob tmp: {e}"))?;
        crate::services::file_ops::replace_file(&tmp_path, &file_path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            format!("rename file blob: {e}")
        })
    }

    fn download_file_blob(&self, hash: &str, _ext: &str) -> Result<Vec<u8>, String> {
        let file_path = PathBuf::from(&self.config.folder_path)
            .join("files")
            .join(hash);
        let size = std::fs::metadata(&file_path)
            .map_err(|error| format!("read file blob metadata: {error}"))?
            .len();
        if size > crate::core::transfer_types::MAX_TRANSFER_FILE_SIZE_BYTES {
            return Err("remote file exceeds the transfer size limit".into());
        }
        std::fs::read(&file_path).map_err(|e| format!("read file blob: {e}"))
    }

    fn delete_file_blob(&self, hash: &str, _ext: &str) -> Result<(), String> {
        let file_path = PathBuf::from(&self.config.folder_path)
            .join("files")
            .join(hash);
        if file_path.exists() {
            std::fs::remove_file(&file_path).map_err(|e| format!("delete file blob: {e}"))
        } else {
            Ok(())
        }
    }
}

impl LocalFolderBackend {
    fn manifest_path(&self) -> PathBuf {
        PathBuf::from(&self.config.folder_path).join(MANIFEST_FILENAME)
    }
}

// --- ── Platform detection helpers ── ---

/// Get a human-readable device name for conflict identification.
#[allow(clippy::bind_instead_of_map)]
pub fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .or_else(|_| {
            #[cfg(unix)]
            {
                let mut buf = [0u8; 256];
                let hostname =
                    unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
                if hostname == 0 {
                    if let Some(end) = buf.iter().position(|&b| b == 0) {
                        return Ok(String::from_utf8_lossy(&buf[..end]).into_owned());
                    }
                }
            }
            #[allow(unreachable_code)]
            Err(std::env::VarError::NotPresent)
        })
        .unwrap_or_else(|_| "unknown-device".into())
}

/// Try to detect the OneDrive folder path on Windows.
#[cfg(target_os = "windows")]
pub fn detect_onedrive_path() -> Option<PathBuf> {
    // --- Method 1: Environment variables ---
    for var in &["OneDrive", "OneDriveConsumer", "OneDriveCommercial"] {
        if let Ok(val) = std::env::var(var) {
            let p = PathBuf::from(&val);
            if p.exists() {
                return Some(p);
            }
        }
    }

    // --- Method 2: Registry ---
    if let Ok(hkcu) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey_with_flags(r"Software\Microsoft\OneDrive", winreg::enums::KEY_READ)
    {
        if let Ok(folder) = hkcu.get_value::<String, _>("UserFolder") {
            let p = PathBuf::from(&folder);
            if p.exists() {
                return Some(p);
            }
        }
    }

    // --- Method 3: Default location ---
    let home = dirs::home_dir()?;
    let candidate = home.join("OneDrive");
    if candidate.exists() {
        return Some(candidate);
    }

    None
}

/// Try to detect the iCloud Drive folder path on macOS.
#[cfg(target_os = "macos")]
pub fn detect_icloud_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let path = home.join("Library/Mobile Documents/com~apple~CloudDocs");
    if path.exists() {
        return Some(path);
    }
    None
}

/// Try to detect the OneDrive folder path on macOS.
/// Checks the App Store version (CloudStorage) and standalone client locations.
#[cfg(target_os = "macos")]
pub fn detect_onedrive_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;

    // Method 1: App Store version — check ~/Library/CloudStorage for OneDrive-* dirs
    let cloud_storage = home.join("Library/CloudStorage");
    if cloud_storage.exists() {
        if let Ok(entries) = std::fs::read_dir(&cloud_storage) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("OneDrive-") && entry.path().is_dir() {
                    return Some(entry.path());
                }
            }
        }
    }

    // --- Method 2: Standalone client default path ---
    let candidate = home.join("OneDrive");
    if candidate.exists() {
        return Some(candidate);
    }

    None
}

/// Try to detect the OneDrive path. Non-Windows/macOS is a no-op.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn detect_onedrive_path() -> Option<PathBuf> {
    None
}

/// Try to detect the iCloud path. On non-macOS this is a no-op.
#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
pub fn detect_icloud_path() -> Option<PathBuf> {
    None
}

/// Detect cloud folder presets and return a list of (name, path) pairs.
pub fn detect_presets() -> Vec<(&'static str, String)> {
    let mut presets = Vec::new();

    // --- OneDrive — available on both Windows and macOS ---
    if let Some(p) = detect_onedrive_path() {
        presets.push(("OneDrive", p.join("Clippi").to_string_lossy().to_string()));
    }

    #[cfg(target_os = "macos")]
    if let Some(p) = detect_icloud_path() {
        presets.push(("iCloud", p.join("Clippi").to_string_lossy().to_string()));
    }

    presets
}

#[cfg(test)]
mod transfer_tests {
    use super::*;

    fn temp_backend(label: &str) -> (PathBuf, LocalFolderBackend) {
        let root = std::env::temp_dir().join(format!(
            "clippi-local-transfer-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let backend = LocalFolderBackend::new(BackendConfig {
            id: label.into(),
            enabled: true,
            backend_type: "local_folder".into(),
            name: label.into(),
            folder_path: root.to_string_lossy().into_owned(),
            device_name: label.into(),
            last_sync_at: String::new(),
            last_item_count: 0,
            last_tag_count: 0,
            sync_interval_secs: None,
            webdav_url: String::new(),
            webdav_root_url: String::new(),
            webdav_path: String::new(),
            webdav_username: String::new(),
            webdav_password: String::new(),
        });
        (root, backend)
    }

    fn entry(hash_byte: char, name: &str) -> ManifestEntry {
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
        }
    }

    fn manifest(files: Vec<ManifestEntry>, device: &str) -> FileManifest {
        FileManifest {
            version: crate::core::migration::TRANSFER_PROTOCOL_VERSION,
            device_name: device.into(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            files,
        }
    }

    #[test]
    fn independent_cloud_replicas_merge_unique_manifest_operations() {
        let (left_root, left) = temp_backend("left");
        let (right_root, right) = temp_backend("right");
        let left_entry = entry('a', "left.bin");
        let right_entry = entry('b', "right.bin");

        left.push_file_manifest(&manifest(vec![left_entry.clone()], "left"), None)
            .unwrap();
        right
            .push_file_manifest(&manifest(vec![right_entry.clone()], "right"), None)
            .unwrap();

        for item in std::fs::read_dir(right_root.join(MANIFEST_OPS_DIR)).unwrap() {
            let item = item.unwrap();
            std::fs::copy(
                item.path(),
                left_root.join(MANIFEST_OPS_DIR).join(item.file_name()),
            )
            .unwrap();
        }

        let snapshot = left.pull_file_manifest().unwrap();
        assert_eq!(snapshot.manifest.files.len(), 2);
        assert!(snapshot.manifest.files.contains(&left_entry));
        assert!(snapshot.manifest.files.contains(&right_entry));

        std::fs::remove_dir_all(left_root).unwrap();
        std::fs::remove_dir_all(right_root).unwrap();
    }

    #[test]
    fn logical_clock_preserves_delete_order_across_clock_skew() {
        let (root, backend) = temp_backend("clock-skew");
        let uploaded = entry('a', "future.bin");
        backend
            .push_file_manifest(&manifest(vec![uploaded], "future-device"), None)
            .unwrap();

        let operation_path = std::fs::read_dir(root.join(MANIFEST_OPS_DIR))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let mut operation: ManifestOperation =
            serde_json::from_slice(&std::fs::read(&operation_path).unwrap()).unwrap();
        operation.created_at = "2099-01-01T00:00:00Z".into();
        std::fs::write(
            &operation_path,
            serde_json::to_vec_pretty(&operation).unwrap(),
        )
        .unwrap();

        let snapshot = backend.pull_file_manifest().unwrap();
        backend
            .push_file_manifest(
                &manifest(Vec::new(), "present-device"),
                snapshot.revision.as_deref(),
            )
            .unwrap();

        assert!(backend
            .pull_file_manifest()
            .unwrap()
            .manifest
            .files
            .is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transfer_blob_upload_replaces_an_existing_object() {
        let (root, backend) = temp_backend("blob-replace");
        let key = format!("{}-{}", "a".repeat(64), uuid::Uuid::new_v4());
        backend.upload_file_blob(&key, "bin", b"old").unwrap();
        backend.upload_file_blob(&key, "bin", b"new").unwrap();
        assert_eq!(backend.download_file_blob(&key, "bin").unwrap(), b"new");
        std::fs::remove_dir_all(root).unwrap();
    }
}
