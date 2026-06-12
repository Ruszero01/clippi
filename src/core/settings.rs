//! --- Settings persistence - loads and saves app settings ---

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

use super::i18n_keys::I18nKey;

#[cfg(target_os = "windows")]
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
#[cfg(target_os = "windows")]
use winreg::RegKey;

#[cfg(target_os = "windows")]
const AUTOSTART_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(target_os = "windows")]
const APP_NAME: &str = "Clippi";

#[cfg(target_os = "macos")]
const LAUNCH_AGENT_ID: &str = "com.clippi.launcher";

/// Configuration for a single sync backend (persisted in settings).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendConfig {
    pub id: String,
    pub enabled: bool,
    pub backend_type: String, // "local_folder" | "webdav"
    pub name: String,
    pub folder_path: String,
    pub device_name: String,
    #[serde(default)]
    pub last_sync_at: String,
    #[serde(default)]
    pub last_item_count: u32,
    #[serde(default)]
    pub last_tag_count: u32,
    #[serde(default)]
    pub sync_interval_secs: Option<u64>, // None = use global default
    // --- WebDAV fields ---
    #[serde(default)]
    pub webdav_url: String,
    #[serde(default)]
    pub webdav_username: String,
    #[serde(default)]
    pub webdav_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub theme: String,
    pub hotkey: String,
    pub auto_start: bool,
    pub auto_hide: bool,
    pub db_path: String,
    pub sort_by_created: bool, // true=sort by creation time, false=sort by update time
    pub window_position_mode: String, // "center" | "follow" | "remember"
    pub saved_window_x: i32,
    pub saved_window_y: i32,
    pub card_height_mode: String, // "low" | "medium" | "high" | "auto"
    #[serde(default)]
    pub silent_start: bool,
    #[serde(default)]
    pub show_source_app: bool,
    #[serde(default)]
    pub auto_scroll_to_top: bool, // auto-scroll to top when the window opens
    #[serde(default)]
    pub copy_as_plain_text: bool, // copy as plain text: strip formatting tags when enabled
    #[serde(default)]
    pub show_original_on_hover: bool, // show original content on hover when a note is set
    #[serde(default)]
    pub saved_window_width: f32, // user-adjusted window width (0=use default)
    #[serde(default)]
    pub saved_window_height: f32, // user-adjusted window height (0=use default)
    // --- ── Cloud sync ── ---
    #[serde(default)]
    pub sync_enabled: bool,
    #[serde(default)]
    pub sync_data_dir: String, // cloud sync directory path (OneDrive/iCloud) — deprecated, use sync_backends
    #[serde(default)]
    pub sync_device_name: String, // device name — deprecated, use sync_backends
    #[serde(default)]
    pub sync_last_at: String, // last sync time RFC3339 — deprecated
    #[serde(default = "default_sync_interval")]
    pub sync_interval_secs: u64, // sync interval in seconds (default 60)
    #[serde(default)]
    pub sync_backends: Vec<BackendConfig>, // multi-backend config list (new)
    #[serde(default)]
    pub sync_auto_enabled: bool, // auto-sync toggle (dirty flag + interval)
    #[serde(default)]
    pub sync_favorites_only: bool, // only sync favorited items
    #[serde(default)]
    pub max_items: u32, // max saved items (0=unlimited, default 0)
    #[serde(default)]
    pub hotkey_blacklist: Vec<String>, // hotkey blacklist app name list
    #[serde(default)]
    pub language: String, // "zh_CN" or "en", empty = follow system
    #[serde(default)]
    pub pinned_tag_ids: Vec<i64>, // tag IDs pinned to sidebar
    #[serde(default = "default_ocr_enabled")]
    pub ocr_enabled: bool, // image OCR auto-detection toggle
    #[serde(default = "default_qr_enabled")]
    pub qr_enabled: bool, // image QR auto-detection toggle
    #[serde(default)]
    pub hide_taskbar_icon: bool, // hide taskbar icon when window is shown (Windows only)
    #[serde(default)]
    pub block_system_window_behaviors: bool, // block system window behaviors (double-click maximize, Aero Snap)
}

fn default_qr_enabled() -> bool {
    true
}

fn default_ocr_enabled() -> bool {
    false
}

fn default_sync_interval() -> u64 {
    60
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: "system".to_string(),
            hotkey: "Alt+V".to_string(),
            auto_start: false,
            auto_hide: true,
            db_path: String::new(),
            sort_by_created: false,
            window_position_mode: "center".to_string(),
            saved_window_x: -1,
            saved_window_y: -1,
            card_height_mode: "auto".to_string(),
            silent_start: true,
            show_source_app: false,
            auto_scroll_to_top: false,
            copy_as_plain_text: false,
            show_original_on_hover: false,
            saved_window_width: 0.0,
            saved_window_height: 0.0,
            sync_enabled: false,
            sync_data_dir: String::new(),
            sync_device_name: String::new(),
            sync_last_at: String::new(),
            sync_interval_secs: 60,
            sync_backends: Vec::new(),
            sync_auto_enabled: false,
            sync_favorites_only: true,
            max_items: 0,
            hotkey_blacklist: Vec::new(),
            language: String::new(),
            pinned_tag_ids: Vec::new(),
            ocr_enabled: false,
            qr_enabled: true,
            hide_taskbar_icon: false,
            block_system_window_behaviors: false,
        }
    }
}

impl AppSettings {
    pub fn load() -> Self {
        let path = Self::config_path();
        let mut settings: Self = if path.exists() {
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    log::error!("无法读取配置文件 {}: {e}", path.display());
                    return Self::default();
                }
            };
            match toml::from_str(&content) {
                Ok(s) => s,
                Err(e) => {
                    log::error!("配置文件解析失败: {e}");
                    let backup = path.with_extension("toml.bak");
                    let _ = std::fs::copy(&path, &backup);
                    log::warn!("已备份损坏的配置文件到 {}", backup.display());
                    Self::default()
                }
            }
        } else {
            Self::default()
        };
        // --- Migrate old flat sync fields → BackendConfig list ---
        settings.migrate_sync_fields();
        settings
    }

    /// One-time migration: old `sync_enabled` + `sync_data_dir` → `sync_backends` entry.
    fn migrate_sync_fields(&mut self) {
        if self.sync_enabled && !self.sync_data_dir.is_empty() && self.sync_backends.is_empty() {
            let device_name = if self.sync_device_name.is_empty() {
                hostname::get()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "unknown".to_string())
            } else {
                self.sync_device_name.clone()
            };
            self.sync_backends.push(BackendConfig {
                id: generate_id(),
                enabled: true,
                backend_type: "local_folder".into(),
                name: device_name.clone(),
                folder_path: self.sync_data_dir.clone(),
                device_name,
                last_sync_at: String::new(),
                last_item_count: 0,
                last_tag_count: 0,
                sync_interval_secs: None,
                webdav_url: String::new(),
                webdav_username: String::new(),
                webdav_password: String::new(),
            });
            // --- Clear old fields ---
            self.sync_enabled = false;
            self.sync_data_dir.clear();
            self.sync_device_name.clear();
            self.sync_last_at.clear();
            // --- Save migrated state ---
            self.save();
        }
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Ok(content) = toml::to_string_pretty(self) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, content);
        }
    }

    fn config_path() -> PathBuf {
        super::paths::config_path()
    }

    pub fn resolve_db_path(&self) -> PathBuf {
        super::paths::resolve_db_path(&self.db_path)
    }
}

#[cfg(target_os = "windows")]
pub fn is_system_dark_mode() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = hkcu.open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
        KEY_READ,
    ) else {
        return false;
    };
    key.get_value::<u32, _>("AppsUseLightTheme").ok() == Some(0)
}

#[cfg(target_os = "macos")]
pub fn is_system_dark_mode() -> bool {
    let mtm = match objc2::MainThreadMarker::new() {
        Some(mtm) => mtm,
        None => return false,
    };
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let appearance = app.effectiveAppearance();
    let name = appearance.name();
    name.to_string().contains("Dark")
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn is_system_dark_mode() -> bool {
    false
}

/// Detect system UI language. Returns "zh_CN" for Chinese systems, "en" otherwise.
pub fn detect_system_language() -> String {
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(key) = hkcu.open_subkey_with_flags(r"Control Panel\International", KEY_READ) {
            if let Ok(locale) = key.get_value::<String, _>("LocaleName") {
                if locale.starts_with("zh") {
                    return "zh_CN".to_string();
                }
            }
        }
        "en".to_string()
    }
    #[cfg(target_os = "macos")]
    {
        // --- Safety: ensure we're on the main thread before calling currentLocale ---
        if objc2::MainThreadMarker::new().is_none() {
            return "en".to_string();
        }
        let locale = objc2_foundation::NSLocale::currentLocale();
        let lang = locale.languageCode().to_string();
        if lang.starts_with("zh") {
            "zh_CN".to_string()
        } else {
            "en".to_string()
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        "en".to_string()
    }
}

#[cfg(target_os = "windows")]
pub fn set_auto_start(enable: bool) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(AUTOSTART_KEY_PATH, KEY_WRITE)
        .map_err(|e| {
            format!(
                "{}: {e}",
                I18nKey::ErrRegistryOpen.text()
            )
        })?;

    if enable {
        let exe_path = std::env::current_exe().map_err(|e| {
            format!(
                "{}: {e}",
                I18nKey::ErrGetExePath.text()
            )
        })?;
        key.set_value(APP_NAME, &exe_path.to_string_lossy().as_ref())
            .map_err(|e| {
                format!(
                    "{}: {e}",
                    I18nKey::ErrRegistryWrite.text()
                )
            })?;
    } else {
        let _ = key.delete_value(APP_NAME);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn launch_agent_plist_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| {
        h.join("Library/LaunchAgents")
            .join(format!("{LAUNCH_AGENT_ID}.plist"))
    })
}

#[cfg(target_os = "macos")]
pub fn set_auto_start(enable: bool) -> Result<(), String> {
    let plist_path = launch_agent_plist_path().ok_or(I18nKey::ErrLaunchAgentsPath.text())?;

    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "{}: {e}",
                I18nKey::ErrCreateLaunchAgents.text()
            )
        })?;
    }

    if enable {
        let exe_path = std::env::current_exe().map_err(|e| {
            format!(
                "{}: {e}",
                I18nKey::ErrGetExePath.text()
            )
        })?;
        let exe_str = exe_path.to_string_lossy();

        let plist_content = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCH_AGENT_ID}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe_str}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
</dict>
</plist>"#
        );

        std::fs::write(&plist_path, plist_content).map_err(|e| {
            format!(
                "{}: {e}",
                I18nKey::ErrWritePlist.text()
            )
        })?;
    } else {
        if plist_path.exists() {
            std::fs::remove_file(&plist_path).map_err(|e| {
                format!(
                    "{}: {e}",
                    I18nKey::ErrDeletePlist.text()
                )
            })?;
        }
    }

    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn set_auto_start(_enable: bool) -> Result<(), String> {
    Ok(())
}

/// Generate a unique ID using splitmix64 mixing for better bit distribution.
pub(crate) fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut x: u64 = ts as u64;
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x = x ^ (x >> 31);
    format!("{:016x}", x)
}

pub fn migrate_database(old_path: &PathBuf, new_path: &PathBuf) -> Result<(), String> {
    if *new_path == *old_path {
        return Err(I18nKey::ErrSamePath.text().into());
    }

    if let Some(parent) = new_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "{}: {e}",
                I18nKey::ErrCreateDir.text()
            )
        })?;
    }

    std::fs::copy(old_path, new_path).map_err(|e| {
        format!(
            "{}: {e}",
            I18nKey::ErrCopyDb.text()
        )
    })?;

    Ok(())
}

pub fn spawn_new_process() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = Command::new(exe).spawn();
    }
}
