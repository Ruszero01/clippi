//! Local-folder sync backend.
//!
//! Reads/writes `clippi_sync.json` in a cloud-synced folder (OneDrive, iCloud,
//! Dropbox, etc.). The OS/cloud-provider handles the actual network sync.

use crate::core::i18n;
use crate::core::settings::BackendConfig;
use crate::core::sync::{self, BackendStatus, BackendType, SyncBackend, SyncPayload};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

const SYNC_FILENAME: &str = "clippi_sync.json";

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
                if name.starts_with("clippi_sync-") && name.ends_with(".json") {
                    // Skip files modified within the last 5 seconds
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
}

impl SyncBackend for LocalFolderBackend {
    fn id(&self) -> &str {
        &self.config.id
    }

    fn name(&self) -> &str {
        &self.config.name
    }

    fn backend_type(&self) -> BackendType {
        BackendType::LocalFolder
    }

    fn sync_interval(&self) -> u64 {
        self.config.sync_interval_secs.unwrap_or(60)
    }

    fn check_status(&self) -> BackendStatus {
        let dir = PathBuf::from(&self.config.folder_path);
        if !dir.exists() {
            return BackendStatus::Offline;
        }
        if !dir.is_dir() {
            return BackendStatus::Error(
                i18n::tr("路径不是目录", "Path is not a directory").into(),
            );
        }
        BackendStatus::Online
    }

    fn pull(&self, bypass_cache: bool) -> Result<SyncPayload, String> {
        let path = self.file_path();
        if !path.exists() {
            return Err(i18n::tr("同步文件不存在", "Sync file not found").into());
        }

        // Check if remote file has changed since last pull.
        // Only applies to the main file; conflict files always need processing.
        // When bypass_cache is true (local dirty, need to compare hashes),
        // force mtime_changed so we always read the file.
        let mut mtime_changed = bypass_cache;
        if !bypass_cache {
            if let Ok(meta) = std::fs::metadata(&path) {
                if let Ok(mtime) = meta.modified() {
                    let mut last = self.last_remote_mtime.lock().unwrap();
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
                    *self.last_remote_mtime.lock().unwrap() = Some(mtime);
                }
            }
        }

        // Read main payload
        let mut payload = if mtime_changed {
            let content = std::fs::read_to_string(&path).map_err(|e| {
                format!(
                    "{}: {e}",
                    i18n::tr("读取同步文件失败", "Failed to read sync file")
                )
            })?;
            serde_json::from_str::<SyncPayload>(&content).map_err(|e| {
                format!(
                    "{}: {e}",
                    i18n::tr("解析同步文件失败", "Failed to parse sync file")
                )
            })?
        } else {
            // Main file unchanged — return early only if no conflicts either
            let conflicts = self.find_conflicts();
            if conflicts.is_empty() {
                return Err("@@unchanged".into());
            }
            // Re-read main payload to merge with conflicts
            let content = std::fs::read_to_string(&path).map_err(|e| {
                format!(
                    "{}: {e}",
                    i18n::tr("读取同步文件失败", "Failed to read sync file")
                )
            })?;
            serde_json::from_str::<SyncPayload>(&content).map_err(|e| {
                format!(
                    "{}: {e}",
                    i18n::tr("解析同步文件失败", "Failed to parse sync file")
                )
            })?
        };

        // Merge conflict files
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
        std::fs::create_dir_all(&dir).map_err(|e| {
            format!(
                "{}: {e}",
                i18n::tr("创建目录失败", "Failed to create directory")
            )
        })?;

        let file_path = self.file_path();
        let json = serde_json::to_string_pretty(payload)
            .map_err(|e| format!("{}: {e}", i18n::tr("序列化失败", "Serialization failed")))?;

        // Atomic write: temp file + rename
        let tmp_path = dir.join(format!(".{SYNC_FILENAME}.tmp"));
        std::fs::write(&tmp_path, &json).map_err(|e| {
            format!(
                "{}: {e}",
                i18n::tr("写入临时文件失败", "Failed to write temp file")
            )
        })?;
        std::fs::rename(&tmp_path, &file_path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            format!(
                "{}: {e}",
                i18n::tr("替换同步文件失败", "Failed to replace sync file")
            )
        })?;
        // Cache new mtime so our own push doesn't trigger a changed-file
        // detection on the next pull.
        if let Ok(meta) = std::fs::metadata(&file_path) {
            if let Ok(mtime) = meta.modified() {
                *self.last_remote_mtime.lock().unwrap() = Some(mtime);
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
}

// ── Platform detection helpers ──

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
    // Method 1: Environment variables
    for var in &["OneDrive", "OneDriveConsumer", "OneDriveCommercial"] {
        if let Ok(val) = std::env::var(var) {
            let p = PathBuf::from(&val);
            if p.exists() {
                return Some(p);
            }
        }
    }

    // Method 2: Registry
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

    // Method 3: Default location
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

    // Method 2: Standalone client default path
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

    // OneDrive — available on both Windows and macOS
    if let Some(p) = detect_onedrive_path() {
        presets.push(("OneDrive", p.join("Clippi").to_string_lossy().to_string()));
    }

    #[cfg(target_os = "macos")]
    if let Some(p) = detect_icloud_path() {
        presets.push(("iCloud", p.join("Clippi").to_string_lossy().to_string()));
    }

    presets
}
