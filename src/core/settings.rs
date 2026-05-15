//! Settings persistence - loads and saves app settings

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendConfig {
    pub id: String,
    pub enabled: bool,
    pub backend_type: String, // "local_folder"
    pub name: String,
    pub folder_path: String,
    pub device_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub theme: String,
    pub hotkey: String,
    pub auto_start: bool,
    pub auto_hide: bool,
    pub db_path: String,
    pub sort_by_created: bool,  // true=按创建时间排序, false=按更新时间排序
    pub window_position_mode: String, // "center" | "follow" | "remember"
    pub saved_window_x: i32,
    pub saved_window_y: i32,
    pub card_height_mode: String, // "low" | "medium" | "high" | "auto"
    #[serde(default)]
    pub silent_start: bool,
    #[serde(default)]
    pub show_source_app: bool,
    #[serde(default)]
    pub auto_scroll_to_top: bool, // 每次开启窗口时自动回到列表顶部
    #[serde(default)]
    pub copy_as_plain_text: bool, // 复制为纯文本: 开启后丢弃格式标签
    #[serde(default)]
    pub show_original_on_hover: bool, // 悬停时显示原内容: 有备注时鼠标悬停显示原始内容
    #[serde(default)]
    pub saved_window_width: f32,  // 用户调整后的窗口宽度 (0=使用默认值)
    #[serde(default)]
    pub saved_window_height: f32, // 用户调整后的窗口高度 (0=使用默认值)
    // ── Cloud sync ──
    #[serde(default)]
    pub sync_enabled: bool,
    #[serde(default)]
    pub sync_data_dir: String,       // 云同步目录路径 (OneDrive/iCloud) — deprecated, use sync_backends
    #[serde(default)]
    pub sync_device_name: String,    // 设备名 — deprecated, use sync_backends
    #[serde(default)]
    pub sync_last_at: String,        // 上次同步时间 RFC3339 — deprecated
    #[serde(default)]
    pub sync_interval_secs: u64,     // 同步间隔秒数 (默认60)
    #[serde(default)]
    pub sync_backends: Vec<BackendConfig>, // 多后端配置列表 (new)
    #[serde(default)]
    pub sync_auto_enabled: bool,           // 自动同步开关 (dirty + interval)
    #[serde(default)]
    pub max_items: u32,                    // 最大保存条目数 (0=不限制, 默认0)
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
            card_height_mode: "medium".to_string(),
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
            sync_auto_enabled: true,
            max_items: 0,
        }
    }
}

impl AppSettings {
    pub fn load() -> Self {
        let path = Self::config_path();
        let mut settings: Self = if path.exists() {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            toml::from_str(&content).unwrap_or_default()
        } else {
            Self::default()
        };
        // Migrate old flat sync fields → BackendConfig list
        settings.migrate_sync_fields();
        settings
    }

    /// One-time migration: old `sync_enabled` + `sync_data_dir` → `sync_backends` entry.
    fn migrate_sync_fields(&mut self) {
        if self.sync_enabled
            && !self.sync_data_dir.is_empty()
            && self.sync_backends.is_empty()
        {
            let device_name = if self.sync_device_name.is_empty() {
                crate::services::backends::local_folder::hostname()
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
            });
            // Clear old fields
            self.sync_enabled = false;
            self.sync_data_dir.clear();
            self.sync_device_name.clear();
            self.sync_last_at.clear();
            // Save migrated state
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
    unsafe {
        let mtm = objc2::MainThreadMarker::new_unchecked();
        let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
        let appearance = app.effectiveAppearance();
        let name = appearance.name();
        name.to_string().contains("Dark")
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn is_system_dark_mode() -> bool {
    false
}

#[cfg(target_os = "windows")]
pub fn set_auto_start(enable: bool) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(AUTOSTART_KEY_PATH, KEY_WRITE)
        .map_err(|e| format!("打开注册表失败: {e}"))?;

    if enable {
        let exe_path = std::env::current_exe().map_err(|e| format!("获取程序路径失败: {e}"))?;
        key.set_value(APP_NAME, &exe_path.to_string_lossy().as_ref())
            .map_err(|e| format!("写入注册表失败: {e}"))?;
    } else {
        let _ = key.delete_value(APP_NAME);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn launch_agent_plist_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join("Library/LaunchAgents").join(format!("{LAUNCH_AGENT_ID}.plist")))
}

#[cfg(target_os = "macos")]
pub fn set_auto_start(enable: bool) -> Result<(), String> {
    let plist_path = launch_agent_plist_path()
        .ok_or("无法获取 LaunchAgents 路径")?;

    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 LaunchAgents 目录失败: {e}"))?;
    }

    if enable {
        let exe_path = std::env::current_exe()
            .map_err(|e| format!("获取程序路径失败: {e}"))?;
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
            .map_err(|e| format!("写入 plist 失败: {e}"))?;
    } else {
        if plist_path.exists() {
            std::fs::remove_file(&plist_path)
                .map_err(|e| format!("删除 plist 失败: {e}"))?;
        }
    }

    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn set_auto_start(_enable: bool) -> Result<(), String> {
    Ok(())
}

/// Generate a simple unique ID for backend configs.
pub(crate) fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let rand: u32 = (ts as u32).wrapping_mul(1103515245).wrapping_add(12345);
    format!("{:08x}{:08x}", ts as u32, rand)
}

pub fn migrate_database(old_path: &PathBuf, new_path: &PathBuf) -> Result<(), String> {
    if *new_path == *old_path {
        return Err("新路径与当前路径相同".into());
    }

    if let Some(parent) = new_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录失败: {e}"))?;
    }

    std::fs::copy(old_path, new_path)
        .map_err(|e| format!("复制数据库失败: {e}"))?;

    Ok(())
}

pub fn spawn_new_process() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = Command::new(exe).spawn();
    }
}
