//! --- Settings persistence - loads and saves app settings ---

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::filters::BUILTIN_TYPE_KEYS;
use super::i18n_keys::I18nKey;

#[cfg(target_os = "windows")]
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE};
#[cfg(target_os = "windows")]
use winreg::RegKey;

#[cfg(target_os = "windows")]
const AUTOSTART_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(target_os = "windows")]
const APP_NAME: &str = "Clippi";
/// Executable compatibility flags. Ticking "以管理员身份运行" / "Run as
/// administrator" in the file properties writes `~ RUNASADMIN` for the exe path.
#[cfg(target_os = "windows")]
const AUTOSTART_LAYERS_PATH: &str =
    r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers";
/// Logon scheduled task used when the Run key cannot elevate the process.
#[cfg(target_os = "windows")]
const AUTOSTART_TASK_NAME: &str = "Clippi AutoStart (Ruszero01)";

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
    pub webdav_root_url: String,
    #[serde(default)]
    pub webdav_path: String,
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

/// One slot in the "latest 10" hotkey configuration (0 = most recent item).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatestHotkeyEntry {
    pub hotkey: String,
    #[serde(default)]
    pub paste_format: String, // serialized HotkeyPasteFormat, empty = Default
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub theme: String,
    pub hotkey: String,
    #[serde(default)]
    pub replace_system_win_v: bool,
    #[serde(default = "default_quick_hotkey")]
    pub quick_hotkey: String,
    #[serde(default)]
    pub quick_hotkey_enabled: bool,
    /// 「同步筛选状态」：快速粘贴窗口与主窗口共用标签、类型与收藏筛选。
    ///
    /// 关闭后快速窗口单独维护自己的筛选，即选中了哪些标签、类型以及是否只看收藏；
    /// 类型栏配置、置顶标签等设置项两个窗口仍然共用。默认开启，与旧行为一致。
    #[serde(default = "default_quick_sync_filters")]
    pub quick_window_sync_filters: bool,
    /// 「快捷粘贴纯文本」全局热键：空 = 禁用（不注册、不占用、触发无效），
    /// 非空 = 注册并生效（spec §2）。TOML 标量，旧配置缺失时按空值加载。
    #[serde(default)]
    pub paste_plain_hotkey: String,
    pub auto_start: bool,
    pub auto_hide: bool,
    pub db_path: String,
    pub sort_by_created: bool, // true=sort by creation time, false=sort by update time
    #[serde(default)]
    pub search_favorites_first: bool, // prioritize favorite items in keyword search results
    pub window_position_mode: String, // "center" | "follow" | "remember"
    pub saved_window_x: i32,
    pub saved_window_y: i32,
    pub card_height_mode: String, // "low" | "medium" | "high" | "auto"
    /// Single font-size entry: one multiplier (see `ui::font`) applied to every
    /// text role so the title/body/meta hierarchy stays intact.
    /// Written as an exact multiplier by the font-size slider.
    #[serde(default)]
    pub font_size_scale: Option<f32>,
    /// Legacy preset name ("compact" | "standard" | "large" | "xlarge"), kept
    /// only so configs written before `font_size_scale` still resolve. Never
    /// written once the numeric scale is present.
    #[serde(default = "default_font_size_level")]
    pub font_size_level: String,
    /// Custom UI font family; empty = `.SystemUIFont`. Chosen from the system
    /// font list so the value is always resolvable; missing glyphs (e.g. a
    /// Latin-only font on Chinese text) fall back via the platform chain.
    #[serde(default)]
    pub font_family: String,
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
    pub saved_enabled_backend_ids: Vec<String>, // remember state before master off
    #[serde(default)]
    pub sync_favorites_only: bool, // only sync favorited items
    #[serde(default)]
    pub sync_include_images: bool, // include image items in sync
    #[serde(default)]
    pub sync_compress_images: bool, // compress images before syncing (JPEG quality 85)
    #[serde(default)]
    pub transfer_station_enabled: bool, // transfer station feature gate
    #[serde(default)]
    pub transfer_backend_id: String, // explicitly selected backend for transfer station
    #[serde(default = "default_transfer_retention_days")]
    pub transfer_retention_days: u32, // transfer file retention days (default 3)
    #[serde(default)]
    pub max_items: u32, // max saved items (0=unlimited, default 0)
    #[serde(default)]
    pub retention_days: u32, // auto-delete items not updated within N days (0=forever)
    #[serde(default)]
    pub hotkey_blacklist: Vec<String>, // hotkey blacklist app name list
    #[serde(default)]
    pub clipboard_app_blacklist: Vec<String>, // apps whose clipboard content is not recorded
    #[serde(default)]
    pub language: String, // "zh_CN" or "en", empty = follow system
    #[serde(default)]
    pub pinned_tag_ids: Vec<i64>, // tag IDs pinned to sidebar
    /// Local display order shared by tag panels and the sidebar. Stable UIDs
    /// avoid assigning a saved position to a different tag after a DB switch.
    #[serde(default)]
    pub tag_order: Vec<String>,
    #[serde(default = "default_ocr_enabled")]
    pub ocr_enabled: bool, // image OCR auto-detection toggle
    #[serde(default = "default_qr_enabled")]
    pub qr_enabled: bool, // image QR auto-detection toggle
    #[serde(default)]
    pub hide_taskbar_icon: bool, // hide taskbar icon on Windows, Dock icon on macOS
    #[serde(default)]
    pub block_system_window_behaviors: bool, // block system window behaviors (double-click maximize, Aero Snap)
    #[serde(default)]
    pub auto_focus_search: bool, // auto-focus search bar when the window opens
    #[serde(default)]
    pub clear_search_on_show: bool, // clear search bar text when the window opens
    #[serde(default)]
    pub type_filter_config: Vec<TypeFilterEntry>, // custom type filter visibility & order
    #[serde(default)]
    pub paste_shortcuts: Vec<PasteShortcutEntry>,
    #[serde(default = "default_latest_hotkeys")]
    pub latest_hotkeys: Vec<LatestHotkeyEntry>,
    #[serde(default = "default_auto_check_updates")]
    pub auto_check_updates: bool,
    #[serde(default)]
    pub update_last_check_at: String,
    /// Update channel: "auto" (query the official mirror and GitHub, then use
    /// the newest version either can serve) | "oss" | "github".
    /// Advanced override only — not exposed in the settings UI and defaults to
    /// "auto". Local-only on purpose: network conditions differ per device, so
    /// this is intentionally not part of `PortableSettingsV1`.
    #[serde(default = "default_update_channel")]
    pub update_channel: String,
    #[serde(default = "default_auto_fetch_url_title")]
    pub auto_fetch_url_title: bool, // auto-fetch page title for link items
    #[serde(default = "default_copy_sound_enabled")]
    pub copy_sound_enabled: bool, // play a subtle sound on copy detection
    #[serde(default = "default_copy_sound_file")]
    pub copy_sound_file: String, // which sound file to play
    #[serde(default)]
    pub filter_foreign_paths: bool, // hide non-native platform paths
    #[serde(default = "default_cleanup_interval")]
    pub cleanup_interval: String, // cache cleanup frequency: "daily" | "weekly" | "never"
    #[serde(default)]
    pub cleanup_stale_items: bool, // auto-cleanup stale (missing source) file/path items
    #[serde(default)]
    pub cleanup_last_date: String, // last cleanup date "YYYY-MM-DD" for periodic scheduling
    #[serde(default)]
    pub retention_cleanup_last_date: String, // last retention cleanup date "YYYY-MM-DD"
    #[serde(default)]
    pub transfer_cleanup_last_date: String, // last transfer expiration cleanup date "YYYY-MM-DD"
    #[serde(default)]
    pub always_reset_to_clipboard: bool, // always switch to clipboard history when window is shown
    #[serde(default = "default_image_alt_mode")]
    pub image_alt_mode: String, // advanced paste mode for images: "bitmap" | "path" | "ocr"
    #[serde(default = "default_paste_click_mode")]
    pub paste_click_mode: String, // main-window paste gesture: "double_click" | "single_click"
}

fn default_image_alt_mode() -> String {
    "bitmap".to_string()
}

fn default_paste_click_mode() -> String {
    "double_click".to_string()
}

fn default_qr_enabled() -> bool {
    true
}

fn default_ocr_enabled() -> bool {
    false
}

fn default_cleanup_interval() -> String {
    "daily".to_string()
}

fn default_sync_interval() -> u64 {
    60
}

fn default_transfer_retention_days() -> u32 {
    3
}

fn default_auto_check_updates() -> bool {
    true
}

fn default_update_channel() -> String {
    "auto".to_string()
}

fn default_auto_fetch_url_title() -> bool {
    true
}

fn default_copy_sound_enabled() -> bool {
    true
}

fn default_copy_sound_file() -> String {
    "copy_penclick.wav".to_string()
}

fn default_latest_hotkeys() -> Vec<LatestHotkeyEntry> {
    (0..10)
        .map(|_| LatestHotkeyEntry {
            hotkey: String::new(),
            paste_format: String::new(),
        })
        .collect()
}

/// 快速窗口默认与主窗口共用筛选状态，与引入该开关之前的行为一致。
fn default_quick_sync_filters() -> bool {
    true
}

fn default_quick_hotkey() -> String {
    "Alt+C".to_string()
}

fn default_font_size_level() -> String {
    "standard".to_string()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: "system".to_string(),
            hotkey: "Alt+V".to_string(),
            replace_system_win_v: false,
            quick_hotkey: default_quick_hotkey(),
            quick_hotkey_enabled: false,
            quick_window_sync_filters: default_quick_sync_filters(),
            paste_plain_hotkey: String::new(),
            auto_start: false,
            auto_hide: true,
            db_path: String::new(),
            sort_by_created: false,
            search_favorites_first: false,
            window_position_mode: "center".to_string(),
            saved_window_x: -1,
            saved_window_y: -1,
            card_height_mode: "auto".to_string(),
            font_size_level: default_font_size_level(),
            font_size_scale: None,
            font_family: String::new(),
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
            saved_enabled_backend_ids: Vec::new(),
            sync_favorites_only: true,
            sync_include_images: false,
            sync_compress_images: false,
            transfer_station_enabled: false,
            transfer_backend_id: String::new(),
            transfer_retention_days: 3,
            max_items: 0,
            retention_days: 0,
            hotkey_blacklist: Vec::new(),
            clipboard_app_blacklist: Vec::new(),
            language: String::new(),
            pinned_tag_ids: Vec::new(),
            tag_order: Vec::new(),
            ocr_enabled: false,
            qr_enabled: true,
            hide_taskbar_icon: false,
            block_system_window_behaviors: false,
            auto_focus_search: false,
            clear_search_on_show: false,
            type_filter_config: Vec::new(),
            paste_shortcuts: Vec::new(),
            latest_hotkeys: default_latest_hotkeys(),
            auto_check_updates: true,
            update_last_check_at: String::new(),
            update_channel: default_update_channel(),
            auto_fetch_url_title: true,
            copy_sound_enabled: true,
            copy_sound_file: default_copy_sound_file(),
            filter_foreign_paths: false,
            cleanup_interval: default_cleanup_interval(),
            cleanup_stale_items: false,
            cleanup_last_date: String::new(),
            retention_cleanup_last_date: String::new(),
            transfer_cleanup_last_date: String::new(),
            always_reset_to_clipboard: false,
            image_alt_mode: default_image_alt_mode(),
            paste_click_mode: default_paste_click_mode(),
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
        // --- Migrate old single WebDAV URL configs to split form fields ---
        if settings.migrate_webdav_fields() {
            settings.save();
        }
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
                webdav_root_url: String::new(),
                webdav_path: String::new(),
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

    fn migrate_webdav_fields(&mut self) -> bool {
        let mut changed = false;
        for backend in &mut self.sync_backends {
            if backend.backend_type != "webdav" {
                continue;
            }

            let has_split_fields = !backend.webdav_root_url.trim().is_empty()
                || !backend.webdav_path.trim().is_empty();
            if has_split_fields {
                let root = backend
                    .webdav_root_url
                    .trim()
                    .trim_end_matches('/')
                    .to_string();
                let path = backend.webdav_path.trim().trim_matches('/').to_string();
                let url = compose_webdav_url(&root, &path);
                if backend.webdav_root_url != root {
                    backend.webdav_root_url = root;
                    changed = true;
                }
                if backend.webdav_path != path {
                    backend.webdav_path = path;
                    changed = true;
                }
                if backend.webdav_url != url {
                    backend.webdav_url = url;
                    changed = true;
                }
                continue;
            }

            let legacy_url = backend.webdav_url.trim();
            if !legacy_url.is_empty() {
                backend.webdav_root_url = legacy_url.to_string();
                backend.webdav_path.clear();
                if backend.webdav_url != legacy_url {
                    backend.webdav_url = legacy_url.to_string();
                }
                changed = true;
            }
        }

        changed
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
        if let Err(error) = self.save_result() {
            log::error!("Failed to save settings: {error}");
        }
    }

    /// Atomically persist settings to the live config path.
    ///
    /// Returns the outcome so debounced callers (window-geometry flush) can
    /// keep their dirty flag and retry with backoff on failure. Unit tests
    /// must never overwrite the user's real `clippi.toml` — persistence under
    /// test is exercised via `save_atomic_to` with temp paths instead.
    pub fn save_result(&self) -> Result<(), String> {
        if cfg!(test) {
            return Ok(());
        }
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
        }
        self.save_atomic_to(&path)
    }

    /// Atomically save settings to an arbitrary path.
    ///
    /// Writes to a temporary file, then renames it onto `path`.  The caller
    /// is responsible for ensuring the parent directory exists.
    /// Used by config-sync to write merged settings without risking
    /// truncation of the live `clippi.toml`.
    pub fn save_atomic_to(&self, path: &Path) -> Result<(), String> {
        let content = toml::to_string_pretty(self).map_err(|e| format!("serialize TOML: {e}"))?;
        let tmp = path.with_extension(format!("toml.tmp.{}", std::process::id()));
        std::fs::write(&tmp, &content)
            .map_err(|e| format!("write temp file {}: {e}", tmp.display()))?;
        if let Err(e) = crate::services::file_ops::replace_file(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!(
                "replace {} -> {}: {e}",
                tmp.display(),
                path.display()
            ));
        }
        Ok(())
    }

    fn config_path() -> PathBuf {
        super::paths::config_path()
    }

    pub fn resolve_db_path(&self) -> PathBuf {
        super::paths::resolve_db_path(&self.db_path)
    }

    /// Unknown persisted values fall back to the default gesture.
    pub fn paste_click_mode_normalized(&self) -> String {
        if self.paste_click_mode == "single_click" {
            "single_click".to_string()
        } else {
            default_paste_click_mode()
        }
    }
}

pub fn compose_webdav_url(root: impl AsRef<str>, path: impl AsRef<str>) -> String {
    let root = root.as_ref().trim();
    let path = path.as_ref().trim().trim_matches('/');
    if root.is_empty() || path.is_empty() {
        root.to_string()
    } else {
        format!("{}/{}", root.trim_end_matches('/'), path)
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

/// How auto-start has to be registered on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoStartMethod {
    /// Per-user registration launched with the logged-on user's filtered token:
    /// the `HKCU\...\Run` value on Windows, a LaunchAgent plist on macOS.
    UserSession,
    /// Windows logon scheduled task with `RunLevel=HighestAvailable`. Required
    /// when Clippi runs elevated: Explorer starts `Run` values with the filtered
    /// token and cannot elevate them at logon, so an elevated executable
    /// registered there never starts.
    ElevatedTask,
}

/// Outcome of a requested auto-start change, so the UI can explain a partial
/// success instead of silently leaving the user with a dead registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoStartChange {
    /// Auto-start is registered exactly as requested.
    Applied,
    /// Auto-start is on, but through the fallback `Run` value, which cannot
    /// elevate at logon: Clippi may not start (or start unelevated) at logon.
    AppliedWithoutElevation,
    /// The exe no longer requests elevation, but the existing elevated task
    /// could not be removed without administrator rights.
    KeptElevatedTask,
    /// Auto-start is off, but a logon task registered by an elevated session is
    /// still in place: deleting it needs administrator rights.
    LeftoverElevatedTask,
}

/// Whether the current process holds an elevated (administrator) token.
#[cfg(target_os = "windows")]
pub fn is_process_elevated() -> bool {
    process_elevation().unwrap_or(false)
}

/// Token elevation state of the current process; `None` when the token query
/// itself failed, which must not be mistaken for "not elevated".
#[cfg(target_os = "windows")]
fn process_elevation() -> Option<bool> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // SAFETY: the pseudo handle from `GetCurrentProcess` must not be closed; the
    // token handle is closed on every path; `TOKEN_ELEVATION` is the exact
    // structure `TokenElevation` expects and its size is passed along.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned: u32 = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut TOKEN_ELEVATION as *mut core::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        CloseHandle(token);
        (ok != 0).then_some(elevation.TokenIsElevated != 0)
    }
}

/// Whether the current process holds an elevated token (always false elsewhere).
/// Kept on every platform so callers do not need their own `cfg` split.
#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
pub fn is_process_elevated() -> bool {
    false
}

/// How auto-start must be registered for the running executable.
pub fn auto_start_method() -> AutoStartMethod {
    #[cfg(target_os = "windows")]
    {
        // Only the persistent "run as administrator" flag counts. The privilege
        // level of the current session is not a signal: the installer and the
        // updater start Clippi elevated once, which must not change how it
        // auto-starts for the rest of the installation's life.
        if std::env::current_exe().is_ok_and(|exe| exe_requests_elevation(&exe)) {
            return AutoStartMethod::ElevatedTask;
        }
    }
    AutoStartMethod::UserSession
}

/// Whether the exe carries the "Run as administrator" compatibility layer.
#[cfg(target_os = "windows")]
fn exe_requests_elevation(exe_path: &Path) -> bool {
    let value_name = exe_path.to_string_lossy().to_string();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey_with_flags(AUTOSTART_LAYERS_PATH, KEY_READ) {
        if let Ok(flags) = key.get_value::<String, _>(&value_name) {
            if layer_flags_request_admin(&flags) {
                return true;
            }
        }
    }
    // Machine-wide layers (written by installers) win over the per-user key.
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok(key) = hklm.open_subkey_with_flags(AUTOSTART_LAYERS_PATH, KEY_READ) {
        if let Ok(flags) = key.get_value::<String, _>(&value_name) {
            if layer_flags_request_admin(&flags) {
                return true;
            }
        }
    }
    false
}

/// Layer values are space-separated tokens such as `~ RUNASADMIN HIGHDPIAWARE`.
#[cfg(target_os = "windows")]
fn layer_flags_request_admin(flags: &str) -> bool {
    flags
        .split_whitespace()
        .any(|token| token.eq_ignore_ascii_case("RUNASADMIN"))
}

#[cfg(target_os = "windows")]
fn write_run_entry(exe_path: &Path) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(AUTOSTART_KEY_PATH, KEY_WRITE)
        .map_err(|e| format!("{}: {e}", I18nKey::ErrRegistryOpen.text()))?;
    // Quoted so a path containing spaces is not split into a different target.
    let exe_value = format!("\"{}\"", exe_path.display());
    key.set_value(APP_NAME, &exe_value)
        .map_err(|e| format!("{}: {e}", I18nKey::ErrRegistryWrite.text()))
}

#[cfg(target_os = "windows")]
fn run_entry_matches(exe_path: &Path) -> bool {
    let expected = format!("\"{}\"", exe_path.display());
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = hkcu.open_subkey_with_flags(AUTOSTART_KEY_PATH, KEY_READ) else {
        return false;
    };
    key.get_value::<String, _>(APP_NAME)
        .is_ok_and(|value| value == expected)
}

#[cfg(target_os = "windows")]
fn remove_run_entry() {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey_with_flags(AUTOSTART_KEY_PATH, KEY_READ | KEY_WRITE) {
        let _ = key.delete_value(APP_NAME);
    }
}

/// Run `schtasks.exe` without flashing a console window.
#[cfg(target_os = "windows")]
fn run_schtasks(args: &[&str]) -> Result<std::process::Output, String> {
    use std::os::windows::process::CommandExt;

    /// `CREATE_NO_WINDOW` — a GUI process must not open a console for the child.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    Command::new("schtasks.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("schtasks {args:?}: {e}"))
}

/// Register (or replace) the logon task that starts this executable elevated.
///
/// Verified against the live Task Scheduler: an `ONLOGON` (and `ONSTART`) task
/// can only be created by an elevated process — a filtered process gets
/// "Access is denied" even without `/RL HIGHEST` — while a plain `DAILY` task is
/// allowed. The call therefore only succeeds for the elevated builds that need
/// it, and the caller falls back to the `Run` value otherwise.
#[cfg(target_os = "windows")]
fn create_auto_start_task(exe_path: &Path) -> Result<(), String> {
    let target = format!("\"{}\"", exe_path.display());
    let output = run_schtasks(&[
        "/Create",
        "/F",
        "/TN",
        AUTOSTART_TASK_NAME,
        "/TR",
        &target,
        "/SC",
        "ONLOGON",
        "/RL",
        "HIGHEST",
        "/IT",
    ])?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!("schtasks /Create: {}", schtasks_message(&output)))
}

/// Read the registered task's executable action, if any.
#[cfg(target_os = "windows")]
fn logon_task_command() -> Option<String> {
    let output = run_schtasks(&["/Query", "/TN", AUTOSTART_TASK_NAME, "/XML"]).ok()?;
    if !output.status.success() {
        return None;
    }
    let xml = decode_console_output(&output.stdout);
    xml_element_text(&xml, "Command")
}

#[cfg(target_os = "windows")]
fn logon_task_targets_exe(exe_path: &Path) -> bool {
    logon_task_command().is_some_and(|command| task_command_targets_exe(&command, exe_path))
}

/// Text of the first `<tag>…</tag>` element.
#[cfg(target_os = "windows")]
fn xml_element_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].trim().to_string())
}

/// Match the task's executable action, never an argument containing our path.
#[cfg(target_os = "windows")]
fn task_command_targets_exe(command: &str, exe_path: &Path) -> bool {
    let command = command.trim().trim_matches('"').to_lowercase();
    let needle = exe_path.to_string_lossy().to_lowercase();
    !needle.is_empty() && (command == needle || command == xml_escape(&needle))
}

/// Escape the characters Task Scheduler escapes when it writes the XML.
#[cfg(target_os = "windows")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Delete Clippi's dedicated logon task, including a stale or renamed exe.
#[cfg(target_os = "windows")]
fn remove_logon_task() -> Result<(), String> {
    if logon_task_command().is_none() {
        return Ok(());
    }
    let output = run_schtasks(&["/Delete", "/F", "/TN", AUTOSTART_TASK_NAME])?;
    if output.status.success() {
        log::info!("Removed the Clippi logon task");
        return Ok(());
    }
    Err(format!(
        "the Clippi logon task could not be removed ({}) — deleting it needs \
         administrator rights, so it keeps starting Clippi at logon",
        schtasks_message(&output)
    ))
}

/// First non-empty line of the captured `schtasks` output.
#[cfg(target_os = "windows")]
fn schtasks_message(output: &std::process::Output) -> String {
    let mut text = decode_console_output(&output.stdout);
    text.push('\n');
    text.push_str(&decode_console_output(&output.stderr));
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("unknown error")
        .to_string()
}

/// `schtasks.exe` writes UTF-16LE once its output is redirected, and text in the
/// system ANSI code page otherwise; decode both so error messages stay readable.
#[cfg(target_os = "windows")]
fn decode_console_output(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    ansi_to_string(bytes)
}

/// Decode console output in the system ANSI code page (GBK on a Chinese system),
/// which `from_utf8_lossy` would turn into replacement characters.
#[cfg(target_os = "windows")]
fn ansi_to_string(bytes: &[u8]) -> String {
    use windows_sys::Win32::Globalization::{MultiByteToWideChar, CP_ACP};

    if bytes.is_empty() {
        return String::new();
    }
    // SAFETY: `MultiByteToWideChar` reads exactly `bytes.len()` bytes from the
    // input pointer and writes at most `wide.len()` units into the output buffer.
    unsafe {
        let needed = MultiByteToWideChar(
            CP_ACP,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        );
        if needed <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut wide = vec![0u16; needed as usize];
        let written = MultiByteToWideChar(
            CP_ACP,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            wide.as_mut_ptr(),
            needed,
        );
        wide.truncate(written.max(0) as usize);
        String::from_utf16_lossy(&wide)
    }
}

#[cfg(target_os = "windows")]
pub fn set_auto_start(enable: bool) -> Result<AutoStartChange, String> {
    let exe_path =
        std::env::current_exe().map_err(|e| format!("{}: {e}", I18nKey::ErrGetExePath.text()))?;

    if !enable {
        remove_run_entry();
        return match remove_logon_task() {
            Ok(()) => Ok(AutoStartChange::Applied),
            Err(e) => {
                log::warn!("Failed to clear auto-start: {e}");
                Ok(AutoStartChange::LeftoverElevatedTask)
            }
        };
    }

    if exe_requests_elevation(&exe_path) {
        match create_auto_start_task(&exe_path) {
            Ok(()) => {
                // A `Run` value would additionally start a filtered instance at logon.
                remove_run_entry();
                log::info!(
                    "Auto-start registered as an elevated logon task for {}",
                    exe_path.display()
                );
                return Ok(AutoStartChange::Applied);
            }
            Err(e) => {
                // Creating an ONLOGON task needs administrator rights, so this
                // only happens when the exe demands elevation but the running
                // process is filtered; the user has to be told.
                log::warn!("{e}; falling back to the Run key, which cannot elevate at logon");
                write_run_entry(&exe_path)?;
                return Ok(AutoStartChange::AppliedWithoutElevation);
            }
        }
    }

    // The admin flag was removed. Drop the old highest-privilege task before
    // writing a Run value, so logging on cannot launch two copies.
    if let Err(e) = remove_logon_task() {
        log::warn!("Failed to switch auto-start back to the Run key: {e}");
        remove_run_entry();
        return Ok(AutoStartChange::KeptElevatedTask);
    }

    write_run_entry(&exe_path)?;
    Ok(AutoStartChange::Applied)
}

/// Re-register auto-start when the stored registration no longer matches the
/// running executable — the app was moved or renamed, a cleaner dropped the
/// entry while `auto_start` stayed true in the settings file, or the executable
/// only now carries "run as administrator" (which the Run key cannot honour).
///
/// The mechanism follows the persistent "run as administrator" flag on the exe,
/// never the privilege level of the current session: the installer and the
/// updater launch Clippi elevated once (`MUI_FINISHPAGE_RUN`,
/// `.onInstSuccess`), which would otherwise flip an installer user between a
/// `Run` value and a logon task on every update.
#[cfg(target_os = "windows")]
pub fn sync_auto_start_path() {
    let Ok(exe_path) = std::env::current_exe() else {
        return;
    };

    if exe_requests_elevation(&exe_path) {
        // Re-create only when the registration is missing or stale.
        if logon_task_targets_exe(&exe_path) {
            // The task is current, but a `Run` value written by an older version
            // would start a second instance at logon.
            remove_run_entry();
            return;
        }
        match create_auto_start_task(&exe_path) {
            Ok(()) => {
                remove_run_entry();
                log::info!(
                    "Auto-start uses an elevated logon task for {}",
                    exe_path.display()
                );
            }
            Err(e) => log::warn!("Failed to register the elevated logon task: {e}"),
        }
        return;
    }

    // If the admin flag was removed, migrate from the elevated task back to
    // the Run key. A filtered process may lack permission; in that case leave
    // the existing task alone and do not add a second registration.
    if let Err(e) = remove_logon_task() {
        remove_run_entry();
        log::warn!("Failed to switch auto-start back to the Run key: {e}");
        return;
    }

    if run_entry_matches(&exe_path) {
        return;
    }

    match write_run_entry(&exe_path) {
        Ok(()) => log::info!("Re-registered auto-start for {}", exe_path.display()),
        Err(e) => log::warn!("Failed to re-register auto-start: {e}"),
    }
}

#[cfg(target_os = "macos")]
fn launch_agent_plist_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| {
        h.join("Library/LaunchAgents")
            .join(format!("{LAUNCH_AGENT_ID}.plist"))
    })
}

#[cfg(target_os = "macos")]
pub fn set_auto_start(enable: bool) -> Result<AutoStartChange, String> {
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

    Ok(AutoStartChange::Applied)
}

/// macOS counterpart of the Windows registry check above.
#[cfg(target_os = "macos")]
pub fn sync_auto_start_path() {
    let Some(plist_path) = launch_agent_plist_path() else {
        return;
    };
    let Ok(exe_path) = std::env::current_exe() else {
        return;
    };
    let exe_str = exe_path.to_string_lossy();
    let expected = format!("<string>{exe_str}</string>");
    if std::fs::read_to_string(&plist_path).is_ok_and(|plist| plist.contains(expected.as_str())) {
        return;
    }
    match set_auto_start(true) {
        Ok(_) => log::info!("Re-registered auto-start for {exe_str}"),
        Err(e) => log::warn!("Failed to re-register auto-start: {e}"),
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn set_auto_start(_enable: bool) -> Result<AutoStartChange, String> {
    Ok(AutoStartChange::Applied)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn sync_auto_start_path() {}

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

/// Spawn a new process with the `--restart` flag and return the result.
///
/// The caller decides what to do next: exit the current process on success,
/// or keep running and notify the user when spawning fails.
pub fn spawn_new_process() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("get current executable path: {e}"))?;
    match Command::new(&exe).arg("--restart").spawn() {
        Ok(child) => {
            log::info!("Spawned new process (pid: {}) for restart", child.id());
            Ok(())
        }
        Err(e) => {
            log::error!("Failed to spawn new process for restart: {e}");
            Err(format!("spawn new process: {e}"))
        }
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

    // ── clipboard_app_blacklist: set union with case-insensitive dedup ──
    let mut cb_blacklist = source.clipboard_app_blacklist.clone();
    for app in &target.clipboard_app_blacklist {
        if !is_app_in_list(&cb_blacklist, app) {
            cb_blacklist.push(app.clone());
        }
    }
    merged.clipboard_app_blacklist = cb_blacklist;

    // ── pinned_tag_ids: set union (ids may differ across DBs, but config merge
    //    is best-effort — stale IDs are silently ignored on load) ──
    let mut pinned = source.pinned_tag_ids.clone();
    for id in &target.pinned_tag_ids {
        if !pinned.contains(id) {
            pinned.push(*id);
        }
    }
    merged.pinned_tag_ids = pinned;

    // Preserve source ordering and append tags known only to the target.
    for uid in &target.tag_order {
        if !merged.tag_order.contains(uid) {
            merged.tag_order.push(uid.clone());
        }
    }

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

// ── App name normalization & matching helpers ──

/// Normalize an app name for case-insensitive comparison.
/// Does NOT affect storage — only used when comparing.
pub fn normalize_app_name(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Check whether `app_name` is present in `list`, ignoring ASCII case.
/// Empty / whitespace-only names never match.
pub fn is_app_in_list(list: &[String], app_name: &str) -> bool {
    let n = normalize_app_name(app_name);
    if n.is_empty() {
        return false;
    }
    list.iter().any(|a| normalize_app_name(a) == n)
}

/// Should clipboard content from this source app be captured?
/// Unknown sources → `true` (fail open).
pub fn should_capture_source(source_app_name: &str, blacklist: &[String]) -> bool {
    if source_app_name.trim().is_empty() {
        return true;
    }
    !is_app_in_list(blacklist, source_app_name)
}

/// Full capture gate: combines startup grace-period and app-blacklist checks.
/// Returns `true` when content detection should proceed.
pub fn capture_gate(source_app_name: &str, blacklist: &[String], startup_done: bool) -> bool {
    if !startup_done {
        return false;
    }
    should_capture_source(source_app_name, blacklist)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_order_defaults_for_old_settings_and_merges_by_uid() {
        let source = AppSettings {
            tag_order: vec!["b".into(), "a".into()],
            ..Default::default()
        };
        let target = AppSettings {
            tag_order: vec!["a".into(), "c".into()],
            ..Default::default()
        };
        let merged = merge_configs(&source, &target, ":memory:");
        assert_eq!(merged.tag_order, vec!["b", "a", "c"]);
        let mut value = toml::Value::try_from(&source).unwrap();
        value.as_table_mut().unwrap().remove("tag_order");
        let old: AppSettings = value.try_into().unwrap();
        assert!(old.tag_order.is_empty());
    }

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

    #[test]
    fn missing_auto_fetch_url_title_defaults_to_enabled() {
        let settings = AppSettings::default();
        let toml = toml::to_string(&settings).unwrap();
        let legacy_toml = toml
            .lines()
            .filter(|line| !line.starts_with("auto_fetch_url_title"))
            .collect::<Vec<_>>()
            .join("\n");

        let loaded: AppSettings = toml::from_str(&legacy_toml).unwrap();

        assert!(loaded.auto_fetch_url_title);
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
            webdav_root_url: String::new(),
            webdav_path: String::new(),
            webdav_username: String::new(),
            webdav_password: String::new(),
        }
    }

    #[test]
    fn migrate_webdav_fields_preserves_legacy_url_as_root() {
        let mut settings = AppSettings::default();
        settings.sync_backends.push(BackendConfig {
            id: "webdav".into(),
            enabled: true,
            backend_type: "webdav".into(),
            name: "Nextcloud".into(),
            folder_path: String::new(),
            device_name: String::new(),
            last_sync_at: String::new(),
            last_item_count: 0,
            last_tag_count: 0,
            sync_interval_secs: Some(600),
            webdav_url: "https://cloud.example.com/remote.php/dav/files/rains/Clippi".into(),
            webdav_root_url: String::new(),
            webdav_path: String::new(),
            webdav_username: String::new(),
            webdav_password: String::new(),
        });

        settings.migrate_webdav_fields();

        let backend = &settings.sync_backends[0];
        assert_eq!(
            backend.webdav_root_url,
            "https://cloud.example.com/remote.php/dav/files/rains/Clippi"
        );
        assert!(backend.webdav_path.is_empty());
        assert_eq!(backend.webdav_url, backend.webdav_root_url);
    }

    #[test]
    fn migrate_webdav_fields_rebuilds_url_from_split_fields() {
        let mut settings = AppSettings::default();
        settings.sync_backends.push(BackendConfig {
            id: "webdav".into(),
            enabled: true,
            backend_type: "webdav".into(),
            name: "WebDAV".into(),
            folder_path: String::new(),
            device_name: String::new(),
            last_sync_at: String::new(),
            last_item_count: 0,
            last_tag_count: 0,
            sync_interval_secs: Some(600),
            webdav_url: String::new(),
            webdav_root_url: "https://cloud.example.com/dav/".into(),
            webdav_path: "/Clippi/".into(),
            webdav_username: String::new(),
            webdav_password: String::new(),
        });

        settings.migrate_webdav_fields();

        let backend = &settings.sync_backends[0];
        assert_eq!(backend.webdav_root_url, "https://cloud.example.com/dav");
        assert_eq!(backend.webdav_path, "Clippi");
        assert_eq!(backend.webdav_url, "https://cloud.example.com/dav/Clippi");
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

    // ── normalize_app_name ──

    #[test]
    fn normalize_app_name_lowercase() {
        assert_eq!(normalize_app_name("KeePass"), "keepass");
    }

    #[test]
    fn normalize_app_name_trim() {
        assert_eq!(normalize_app_name(" KeePass "), "keepass");
    }

    // ── is_app_in_list ──

    #[test]
    fn is_app_in_list_hit() {
        let list = vec!["Notepad".into(), "KeePass".into()];
        assert!(is_app_in_list(&list, "KeePass"));
    }

    #[test]
    fn is_app_in_list_case_variant() {
        let list = vec!["KeePass".into()];
        assert!(is_app_in_list(&list, "keepass"));
        assert!(is_app_in_list(&list, "KEEPASS"));
    }

    #[test]
    fn is_app_in_list_empty_name() {
        let list = vec!["KeePass".into()];
        assert!(!is_app_in_list(&list, ""));
    }

    #[test]
    fn is_app_in_list_whitespace() {
        let list = vec!["KeePass".into()];
        assert!(!is_app_in_list(&list, "  "));
    }

    #[test]
    fn is_app_in_list_empty_list() {
        let list: Vec<String> = vec![];
        assert!(!is_app_in_list(&list, "KeePass"));
    }

    // ── should_capture_source ──

    #[test]
    fn should_capture_source_normal() {
        let blacklist: Vec<String> = vec![];
        assert!(should_capture_source("Notepad", &blacklist));
    }

    #[test]
    fn should_capture_source_blacklisted() {
        let blacklist = vec!["KeePass".into()];
        assert!(!should_capture_source("KeePass", &blacklist));
    }

    #[test]
    fn should_capture_source_case_variant() {
        let blacklist = vec!["KeePass".into()];
        assert!(!should_capture_source("keepass", &blacklist));
    }

    #[test]
    fn should_capture_source_empty() {
        let blacklist = vec!["KeePass".into()];
        assert!(should_capture_source("", &blacklist)); // fail open
    }

    #[test]
    fn should_capture_source_whitespace() {
        let blacklist = vec!["KeePass".into()];
        assert!(should_capture_source("  ", &blacklist)); // fail open
    }

    // ── capture_gate ──

    #[test]
    fn capture_gate_grace_period_blocks() {
        let blacklist: Vec<String> = vec![];
        assert!(!capture_gate("Notepad", &blacklist, false));
    }

    #[test]
    fn capture_gate_blacklisted_blocks() {
        let blacklist = vec!["KeePass".into()];
        assert!(!capture_gate("KeePass", &blacklist, true));
    }

    #[test]
    fn capture_gate_normal_allows() {
        let blacklist: Vec<String> = vec![];
        assert!(capture_gate("Notepad", &blacklist, true));
    }

    #[test]
    fn capture_gate_unknown_allows() {
        let blacklist = vec!["KeePass".into()];
        assert!(capture_gate("", &blacklist, true)); // fail open
    }

    // ── merge clipboard_app_blacklist ──

    #[test]
    fn merge_configs_union_clipboard_blacklist() {
        let source = AppSettings {
            clipboard_app_blacklist: vec!["KeePass".into(), "Terminal".into()],
            ..Default::default()
        };
        let target = AppSettings {
            clipboard_app_blacklist: vec!["Terminal".into(), "Notepad".into()],
            ..Default::default()
        };

        let merged = merge_configs(&source, &target, "");
        assert_eq!(merged.clipboard_app_blacklist.len(), 3);
        assert!(merged
            .clipboard_app_blacklist
            .iter()
            .any(|a| a == "KeePass"));
        assert!(merged
            .clipboard_app_blacklist
            .iter()
            .any(|a| a == "Terminal"));
        assert!(merged
            .clipboard_app_blacklist
            .iter()
            .any(|a| a == "Notepad"));
    }

    #[test]
    fn merge_clipboard_blacklist_case_dedup() {
        let source = AppSettings {
            clipboard_app_blacklist: vec!["KeePass".into()],
            ..Default::default()
        };
        let target = AppSettings {
            clipboard_app_blacklist: vec!["keepass".into(), "Notepad".into()],
            ..Default::default()
        };

        let merged = merge_configs(&source, &target, "");
        // "KeePass" and "keepass" → one entry (source's original casing wins)
        assert_eq!(merged.clipboard_app_blacklist.len(), 2);
    }

    #[test]
    fn missing_cleanup_stale_items_defaults_to_disabled() {
        let serialized = toml::to_string(&AppSettings::default()).unwrap();
        let legacy_config = serialized
            .lines()
            .filter(|line| !line.starts_with("cleanup_stale_items ="))
            .collect::<Vec<_>>()
            .join("\n");

        let parsed: AppSettings = toml::from_str(&legacy_config).unwrap();

        assert!(!parsed.cleanup_stale_items);
    }

    #[test]
    fn missing_search_favorites_first_defaults_to_disabled() {
        let serialized = toml::to_string(&AppSettings::default()).unwrap();
        let legacy_config = serialized
            .lines()
            .filter(|line| !line.starts_with("search_favorites_first ="))
            .collect::<Vec<_>>()
            .join("\n");

        let parsed: AppSettings = toml::from_str(&legacy_config).unwrap();

        assert!(!parsed.search_favorites_first);
    }

    #[test]
    fn search_favorites_first_roundtrips_through_toml() {
        let settings = AppSettings {
            search_favorites_first: true,
            ..Default::default()
        };
        let serialized = toml::to_string(&settings).unwrap();
        let loaded: AppSettings = toml::from_str(&serialized).unwrap();

        assert!(loaded.search_favorites_first);
    }

    #[test]
    fn missing_paste_plain_hotkey_defaults_to_empty() {
        let serialized = toml::to_string(&AppSettings::default()).unwrap();
        let legacy_config = serialized
            .lines()
            .filter(|line| !line.starts_with("paste_plain_hotkey ="))
            .collect::<Vec<_>>()
            .join("\n");

        let parsed: AppSettings = toml::from_str(&legacy_config).unwrap();

        assert!(parsed.paste_plain_hotkey.is_empty());
    }

    #[test]
    fn paste_plain_hotkey_roundtrips_through_toml() {
        let settings = AppSettings {
            paste_plain_hotkey: "Alt+Shift+P".into(),
            ..Default::default()
        };
        let serialized = toml::to_string(&settings).unwrap();
        let loaded: AppSettings = toml::from_str(&serialized).unwrap();

        assert_eq!(loaded.paste_plain_hotkey, "Alt+Shift+P");
    }

    #[test]
    fn paste_plain_hotkey_follows_source_in_merge() {
        // Scalar fields keep the `source` (current user) values in merge.
        let source = AppSettings {
            paste_plain_hotkey: "Alt+Shift+P".into(),
            ..Default::default()
        };
        let target = AppSettings {
            paste_plain_hotkey: "Ctrl+Shift+P".into(),
            ..Default::default()
        };

        let merged = merge_configs(&source, &target, "/new/path/clippi.db");
        assert_eq!(merged.paste_plain_hotkey, "Alt+Shift+P");
    }

    #[test]
    fn missing_paste_click_mode_defaults_to_double_click() {
        let serialized = toml::to_string(&AppSettings::default()).unwrap();
        let legacy_config = serialized
            .lines()
            .filter(|line| !line.starts_with("paste_click_mode ="))
            .collect::<Vec<_>>()
            .join("\n");

        let parsed: AppSettings = toml::from_str(&legacy_config).unwrap();

        assert_eq!(parsed.paste_click_mode, "double_click");
        assert_eq!(parsed.paste_click_mode_normalized(), "double_click");
    }

    #[test]
    fn paste_click_mode_roundtrips_through_toml() {
        let settings = AppSettings {
            paste_click_mode: "single_click".into(),
            ..Default::default()
        };
        let serialized = toml::to_string(&settings).unwrap();
        let loaded: AppSettings = toml::from_str(&serialized).unwrap();

        assert_eq!(loaded.paste_click_mode, "single_click");
        assert_eq!(loaded.paste_click_mode_normalized(), "single_click");
    }

    #[test]
    fn default_paste_click_mode_is_double_click() {
        assert_eq!(AppSettings::default().paste_click_mode, "double_click");
        assert_eq!(default_paste_click_mode(), "double_click");
    }

    #[test]
    fn unknown_paste_click_mode_normalizes_to_default() {
        let settings = AppSettings {
            paste_click_mode: "bogus".into(),
            ..Default::default()
        };

        // Raw persisted value is preserved verbatim by serde (no enum validation);
        // the normalized accessor is the single source of truth for consumers.
        assert_eq!(settings.paste_click_mode, "bogus");
        assert_eq!(settings.paste_click_mode_normalized(), "double_click");
    }

    #[test]
    fn merge_configs_keeps_search_favorites_first_from_source() {
        let source = AppSettings {
            search_favorites_first: true,
            ..Default::default()
        };
        let target = AppSettings {
            search_favorites_first: false,
            ..Default::default()
        };

        let merged = merge_configs(&source, &target, "/new/path/clippi.db");

        assert!(merged.search_favorites_first); // scalar fields follow source
    }

    #[test]
    fn save_atomic_to_overwrites_existing_file() {
        // Cloud-apply writes the merged config over the live clippi.toml.
        // This must work when the destination already exists (Windows
        // `std::fs::rename` would fail here; `replace_file` must not).
        let dir = std::env::temp_dir().join(format!(
            "clippi-settings-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clippi.toml");

        let old = AppSettings {
            theme: "light".into(),
            ..Default::default()
        };
        old.save_atomic_to(&path).unwrap();

        let merged = AppSettings {
            theme: "dark".into(),
            auto_hide: false,
            ..Default::default()
        };
        merged.save_atomic_to(&path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let loaded: AppSettings = toml::from_str(&content).unwrap();
        assert_eq!(loaded.theme, "dark");
        assert!(!loaded.auto_hide);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeated_save_atomic_to_is_consistent() {
        // Re-saving the same merged settings must not change the file.
        // This covers the "apply + later save" disk behavior only;
        // the actual regression guard for committing `merged` into
        // `AppState.settings` lives in `WindowManager::apply_config_snapshot`
        // (it needs a GPUI context and cannot be exercised here).
        let dir = std::env::temp_dir().join(format!(
            "clippi-settings-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clippi.toml");

        let merged = AppSettings {
            theme: "dark".into(),
            max_items: 250,
            ..Default::default()
        };
        merged.save_atomic_to(&path).unwrap();
        let after_apply = std::fs::read_to_string(&path).unwrap();

        // Later runtime saves happen from the committed in-memory settings:
        // writing the same merged content again is idempotent on disk.
        merged.save_atomic_to(&path).unwrap();
        let after_resave = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after_apply, after_resave);

        let loaded: AppSettings = toml::from_str(&after_resave).unwrap();
        assert_eq!(loaded.theme, "dark");
        assert_eq!(loaded.max_items, 250);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The "Run as administrator" tick lands in the Layers value as one token
    /// among several, so detection must not depend on the whole string.
    #[cfg(target_os = "windows")]
    #[test]
    fn run_as_admin_layer_flag_is_detected_among_other_tokens() {
        assert!(layer_flags_request_admin("~ RUNASADMIN"));
        assert!(layer_flags_request_admin("~ RUNASADMIN HIGHDPIAWARE"));
        assert!(layer_flags_request_admin("~  highdpiaware runasadmin"));
        assert!(!layer_flags_request_admin("~ HIGHDPIAWARE"));
        assert!(!layer_flags_request_admin(""));
    }

    /// Guards the Win32 plumbing behind the elevated auto-start decision: a wrong
    /// information class or structure size would silently degrade to `false`.
    #[cfg(target_os = "windows")]
    #[test]
    fn elevation_query_reads_the_process_token() {
        assert!(
            process_elevation().is_some(),
            "GetTokenInformation(TokenElevation) failed"
        );
    }

    /// Only the task action may establish ownership; a path in arguments must
    /// not make a different program's task look like Clippi's.
    #[cfg(target_os = "windows")]
    #[test]
    fn task_command_matching_checks_only_the_executable() {
        let path = std::path::Path::new(r"C:\Program Files\Some App\clippi.exe");

        // What `create_auto_start_task` produces (verified against the live
        // Task Scheduler).
        let xml = format!(
            "<Task><Actions><Exec><Command>\"{}\"</Command></Exec></Actions></Task>",
            path.display()
        );
        let command = xml_element_text(&xml, "Command").unwrap();
        assert!(task_command_targets_exe(&command, path));
        assert!(task_command_targets_exe(
            r"C:\Program Files\Some App\clippi.exe",
            path
        ));

        // `&` is XML-escaped inside the task definition.
        let ampersand = std::path::Path::new(r"C:\Tools\A&B\clippi.exe");
        assert!(task_command_targets_exe(
            "C:\\Tools\\A&amp;B\\clippi.exe",
            ampersand
        ));

        // Another executable, a path in arguments, and a stale path do not match.
        assert!(!task_command_targets_exe(
            r"C:\Program Files\Some App\clippi-helper.exe",
            path
        ));
        assert!(!task_command_targets_exe(
            r"C:\Tools\runner.exe C:\Program Files\Some App\clippi.exe",
            path
        ));
        assert!(!task_command_targets_exe(r"C:\Other\clippi.exe", path));
        assert!(!task_command_targets_exe(
            r"C:\Program Files\Some App\clippi.exe",
            std::path::Path::new("")
        ));

        assert_eq!(xml_element_text(&xml, "Arguments"), None);
    }

    /// Redirected `schtasks` output is UTF-16LE; decoding it as UTF-8 would turn
    /// every error message into replacement characters.
    #[cfg(target_os = "windows")]
    #[test]
    fn schtasks_output_decodes_utf16_and_ansi() {
        let mut utf16 = vec![0xFF, 0xFE];
        for unit in "错误: 拒绝访问。".encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_console_output(&utf16), "错误: 拒绝访问。");
        assert_eq!(
            decode_console_output("ERROR: Access is denied.".as_bytes()),
            "ERROR: Access is denied."
        );
        assert_eq!(decode_console_output(&[]), "");
    }

    /// A stale task must never be the only registration on a filtered process,
    /// and the elevated build must not fall back to the Run key silently.
    #[test]
    fn auto_start_method_is_a_user_session_registration_when_not_elevated() {
        // The test harness runs filtered, so the Windows branch has to answer
        // `UserSession` unless the exe itself carries the RUNASADMIN layer.
        let method = auto_start_method();
        assert!(matches!(
            method,
            AutoStartMethod::UserSession | AutoStartMethod::ElevatedTask
        ));
        assert!(!is_process_elevated() || method == AutoStartMethod::ElevatedTask);
    }
}
