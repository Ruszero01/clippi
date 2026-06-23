//! --- Settings persistence - loads and saves app settings ---

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::filters::BUILTIN_TYPE_KEYS;
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

/// User-configurable entry for a single content-type filter button.
/// Order in the Vec determines display order in the filter bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeFilterEntry {
    pub key: String,
    pub visible: bool,
}

/// Per-process paste shortcut mapping entry.
/// When the foreground app matches `app_name`, use `shortcut` instead of Ctrl+V.
/// Example: `PasteShortcutEntry { app_name: "WindowsTerminal".into(), shortcut: "Shift+Insert".into() }`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasteShortcutEntry {
    pub app_name: String,
    pub shortcut: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub theme: String,
    pub hotkey: String,
    #[serde(default = "default_quick_hotkey")]
    pub quick_hotkey: String,
    #[serde(default)]
    pub quick_hotkey_enabled: bool,
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
    #[serde(default)]
    pub auto_focus_search: bool, // auto-focus search bar when the window opens
    #[serde(default)]
    pub type_filter_config: Vec<TypeFilterEntry>, // custom type filter visibility & order
    #[serde(default)]
    pub paste_shortcuts: Vec<PasteShortcutEntry>,
    #[serde(default = "default_auto_check_updates")]
    pub auto_check_updates: bool,
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

fn default_auto_check_updates() -> bool {
    true
}

fn default_quick_hotkey() -> String {
    "Alt+C".to_string()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: "system".to_string(),
            hotkey: "Alt+V".to_string(),
            quick_hotkey: default_quick_hotkey(),
            quick_hotkey_enabled: false,
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
            auto_focus_search: false,
            type_filter_config: Vec::new(),
            paste_shortcuts: Vec::new(),
            auto_check_updates: true,
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
        // --- Migrate type filter config (seed from BUILTIN_TYPE_KEYS) ---
        settings.migrate_type_filter_config();
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

    /// Seed or merge type filter config from `BUILTIN_TYPE_KEYS`.
    /// - First run (empty config): seed all built-in types as visible.
    /// - Subsequent runs: append any new built-in keys that aren't in config yet.
    fn migrate_type_filter_config(&mut self) {
        if self.type_filter_config.is_empty() {
            // First run: seed from BUILTIN_TYPE_KEYS, all visible
            for key in BUILTIN_TYPE_KEYS {
                self.type_filter_config.push(TypeFilterEntry {
                    key: key.to_string(),
                    visible: true,
                });
            }
            self.save();
            return;
        }
        // Merge: append new built-in keys not yet in config
        let known_keys: Vec<String> = self
            .type_filter_config
            .iter()
            .map(|e| e.key.clone())
            .collect();
        let mut changed = false;
        for key in BUILTIN_TYPE_KEYS {
            if !known_keys.iter().any(|k| k == key) {
                self.type_filter_config.push(TypeFilterEntry {
                    key: key.to_string(),
                    visible: true,
                });
                changed = true;
            }
        }
        if changed {
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
        .map_err(|e| format!("{}: {e}", I18nKey::ErrRegistryOpen.text()))?;

    if enable {
        let exe_path = std::env::current_exe()
            .map_err(|e| format!("{}: {e}", I18nKey::ErrGetExePath.text()))?;
        key.set_value(APP_NAME, &exe_path.to_string_lossy().as_ref())
            .map_err(|e| format!("{}: {e}", I18nKey::ErrRegistryWrite.text()))?;
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
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("{}: {e}", I18nKey::ErrCreateLaunchAgents.text()))?;
    }

    if enable {
        let exe_path = std::env::current_exe()
            .map_err(|e| format!("{}: {e}", I18nKey::ErrGetExePath.text()))?;
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

        std::fs::write(&plist_path, plist_content)
            .map_err(|e| format!("{}: {e}", I18nKey::ErrWritePlist.text()))?;
    } else {
        if plist_path.exists() {
            std::fs::remove_file(&plist_path)
                .map_err(|e| format!("{}: {e}", I18nKey::ErrDeletePlist.text()))?;
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

pub fn migrate_database(old_path: &Path, new_path: &Path) -> Result<(), String> {
    if *new_path == *old_path {
        return Err(I18nKey::ErrSamePath.text().into());
    }

    if let Some(parent) = new_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("{}: {e}", I18nKey::ErrCreateDir.text()))?;
    }

    std::fs::copy(old_path, new_path).map_err(|e| format!("{}: {e}", I18nKey::ErrCopyDb.text()))?;

    Ok(())
}

pub fn spawn_new_process() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = Command::new(exe).spawn();
    }
}

/// Merge two `AppSettings` instances for the data-directory reset flow.
///
/// Rules:
/// - Scalar fields (theme, hotkey, etc.): keep `source` values (current user
///   preferences).
/// - List fields (`sync_backends`, `type_filter_config`, etc.): union from both
///   configs, deduplicating by natural key.
/// - `db_path`: set to `new_db_path` (may be empty for portable mode).
pub fn merge_configs(source: &AppSettings, target: &AppSettings, new_db_path: &str) -> AppSettings {
    let mut merged = source.clone();
    merged.db_path = new_db_path.to_string();

    // ── sync_backends: merge by id (source takes precedence) ──
    let source_ids: Vec<&str> = source.sync_backends.iter().map(|b| b.id.as_str()).collect();
    for tb in &target.sync_backends {
        if !source_ids.contains(&tb.id.as_str()) {
            merged.sync_backends.push(tb.clone());
        }
    }

    // ── type_filter_config: merge by key (source takes precedence) ──
    let source_keys: Vec<&str> = source
        .type_filter_config
        .iter()
        .map(|e| e.key.as_str())
        .collect();
    for te in &target.type_filter_config {
        if !source_keys.contains(&te.key.as_str()) {
            merged.type_filter_config.push(te.clone());
        }
    }

    // ── hotkey_blacklist: set union ──
    let mut blacklist = source.hotkey_blacklist.clone();
    for app in &target.hotkey_blacklist {
        if !blacklist.contains(app) {
            blacklist.push(app.clone());
        }
    }
    merged.hotkey_blacklist = blacklist;

    // ── pinned_tag_ids: set union (ids may differ across DBs, but config merge
    //    is best-effort — stale IDs are silently ignored on load) ──
    let mut pinned = source.pinned_tag_ids.clone();
    for id in &target.pinned_tag_ids {
        if !pinned.contains(id) {
            pinned.push(*id);
        }
    }
    merged.pinned_tag_ids = pinned;

    // ── paste_shortcuts: merge by app_name (source takes precedence) ──
    let source_apps: Vec<&str> = source
        .paste_shortcuts
        .iter()
        .map(|p| p.app_name.as_str())
        .collect();
    for tp in &target.paste_shortcuts {
        if !source_apps.contains(&tp.app_name.as_str()) {
            merged.paste_shortcuts.push(tp.clone());
        }
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_configs_scalar_from_source() {
        let source = AppSettings {
            theme: "dark".into(),
            hotkey: "Alt+V".into(),
            auto_hide: true,
            max_items: 500,
            ..Default::default()
        };
        let target = AppSettings {
            theme: "light".into(),
            hotkey: "Ctrl+Shift+V".into(),
            auto_hide: false,
            max_items: 100,
            ..Default::default()
        };

        let merged = merge_configs(&source, &target, "/new/path/clippi.db");
        assert_eq!(merged.theme, "dark"); // source wins
        assert_eq!(merged.hotkey, "Alt+V"); // source wins
        assert!(merged.auto_hide); // source wins
        assert_eq!(merged.max_items, 500); // source wins
        assert_eq!(merged.db_path, "/new/path/clippi.db"); // explicit override
    }

    fn bk(id: &str, name: &str) -> BackendConfig {
        BackendConfig {
            id: id.into(),
            enabled: true,
            backend_type: "local_folder".into(),
            name: name.into(),
            folder_path: String::new(),
            device_name: name.into(),
            last_sync_at: String::new(),
            last_item_count: 0,
            last_tag_count: 0,
            sync_interval_secs: None,
            webdav_url: String::new(),
            webdav_username: String::new(),
            webdav_password: String::new(),
        }
    }

    #[test]
    fn merge_configs_union_sync_backends_by_id() {
        let mut source = AppSettings::default();
        source.sync_backends.push(bk("a", "Source"));

        let mut target = AppSettings::default();
        target.sync_backends.push(bk("a", "Target"));
        target.sync_backends.push(bk("b", "TargetOnly"));

        let merged = merge_configs(&source, &target, "");
        assert_eq!(merged.sync_backends.len(), 2);
        // Source's "a" wins (same id).
        assert_eq!(merged.sync_backends[0].name, "Source");
        // Target's "b" appended (new id).
        assert_eq!(merged.sync_backends[1].name, "TargetOnly");
    }

    #[test]
    fn merge_configs_union_type_filter_by_key() {
        let source = AppSettings {
            type_filter_config: vec![TypeFilterEntry {
                key: "plain_text".into(),
                visible: false,
            }],
            ..Default::default()
        };

        let target = AppSettings {
            type_filter_config: vec![
                TypeFilterEntry {
                    key: "plain_text".into(),
                    visible: true,
                },
                TypeFilterEntry {
                    key: "image".into(),
                    visible: true,
                },
            ],
            ..Default::default()
        };

        let merged = merge_configs(&source, &target, "");
        assert_eq!(merged.type_filter_config.len(), 2);
        // Source's plain_text wins (same key, visible=false from source).
        assert!(!merged.type_filter_config[0].visible);
        // Target's image appended (new key).
        assert_eq!(merged.type_filter_config[1].key, "image");
    }

    #[test]
    fn merge_configs_union_blacklist() {
        let source = AppSettings {
            hotkey_blacklist: vec!["app1".into(), "app2".into()],
            ..Default::default()
        };
        let target = AppSettings {
            hotkey_blacklist: vec!["app2".into(), "app3".into()],
            ..Default::default()
        };

        let merged = merge_configs(&source, &target, "");
        assert_eq!(merged.hotkey_blacklist.len(), 3);
        assert!(merged.hotkey_blacklist.contains(&"app1".into()));
        assert!(merged.hotkey_blacklist.contains(&"app2".into()));
        assert!(merged.hotkey_blacklist.contains(&"app3".into()));
    }

    #[test]
    fn merge_configs_union_pinned_tags() {
        let source = AppSettings {
            pinned_tag_ids: vec![1, 2],
            ..Default::default()
        };
        let target = AppSettings {
            pinned_tag_ids: vec![2, 3],
            ..Default::default()
        };

        let merged = merge_configs(&source, &target, "");
        assert_eq!(merged.pinned_tag_ids.len(), 3);
        assert!(merged.pinned_tag_ids.contains(&1));
        assert!(merged.pinned_tag_ids.contains(&2));
        assert!(merged.pinned_tag_ids.contains(&3));
    }

    #[test]
    fn merge_configs_union_paste_shortcuts_by_app_name() {
        let mut source = AppSettings::default();
        source.paste_shortcuts.push(PasteShortcutEntry {
            app_name: "Terminal".into(),
            shortcut: "Shift+Insert".into(),
        });

        let mut target = AppSettings::default();
        target.paste_shortcuts.push(PasteShortcutEntry {
            app_name: "Terminal".into(),
            shortcut: "Ctrl+Shift+V".into(),
        });
        target.paste_shortcuts.push(PasteShortcutEntry {
            app_name: "Notepad".into(),
            shortcut: "Ctrl+V".into(),
        });

        let merged = merge_configs(&source, &target, "");
        assert_eq!(merged.paste_shortcuts.len(), 2);
        // Source's Terminal wins (same app_name).
        assert_eq!(merged.paste_shortcuts[0].shortcut, "Shift+Insert");
        // Target's Notepad appended (new app_name).
        assert_eq!(merged.paste_shortcuts[1].app_name, "Notepad");
    }
}
