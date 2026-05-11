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
    pub silent_start: bool,
    pub show_source_app: bool,
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
        }
    }
}

impl AppSettings {
    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            toml::from_str(&content).unwrap_or_default()
        } else {
            Self::default()
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
