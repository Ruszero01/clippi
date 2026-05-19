//! Local-folder sync backend.
//!
//! Reads/writes `clippi_sync.json` in a cloud-synced folder (OneDrive, iCloud,
//! Dropbox, etc.). The OS/cloud-provider handles the actual network sync.

use crate::core::settings::BackendConfig;
use crate::core::sync::{BackendStatus, BackendType, SyncBackend, SyncPayload};
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

    fn check_status(&self) -> BackendStatus {
        let dir = PathBuf::from(&self.config.folder_path);
        if !dir.exists() {
            return BackendStatus::Offline;
        }
        if !dir.is_dir() {
            return BackendStatus::Error("路径不是目录".into());
        }
        BackendStatus::Online
    }

    fn pull(&self) -> Result<SyncPayload, String> {
        let path = self.file_path();
        if !path.exists() {
            return Err("同步文件不存在".into());
        }

        // Check if remote file has changed since last pull.
        // This avoids unnecessary reads — content hash comparison in
        // `run_sync_cycle_for_backend` is the authoritative gate for push.
        if let Ok(meta) = std::fs::metadata(&path) {
            if let Ok(mtime) = meta.modified() {
                let mut last = self.last_remote_mtime.lock().unwrap();
                if *last == Some(mtime) {
                    return Err("@@unchanged".into());
                }
                *last = Some(mtime);
            }
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("读取同步文件失败: {e}"))?;
        serde_json::from_str::<SyncPayload>(&content)
            .map_err(|e| format!("解析同步文件失败: {e}"))
    }

    fn push(&self, payload: &SyncPayload) -> Result<(), String> {
        let dir = PathBuf::from(&self.config.folder_path);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("创建目录失败: {e}"))?;

        let file_path = self.file_path();
        let json = serde_json::to_string_pretty(payload)
            .map_err(|e| format!("序列化失败: {e}"))?;

        // Atomic write: temp file + rename
        let tmp_path = dir.join(format!(".{SYNC_FILENAME}.tmp"));
        std::fs::write(&tmp_path, &json)
            .map_err(|e| format!("写入临时文件失败: {e}"))?;
        std::fs::rename(&tmp_path, &file_path)
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp_path);
                format!("替换同步文件失败: {e}")
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
}

// ── Platform detection helpers ──

/// Get a human-readable device name for conflict identification.
pub fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
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
