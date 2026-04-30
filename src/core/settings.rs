//! Settings persistence - loads and saves app settings to clippi.toml

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
use winreg::RegKey;

const AUTOSTART_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APP_NAME: &str = "Clippi";
const CONFIG_FILE: &str = "clippi.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub theme: String,
    pub hotkey: String,
    pub auto_start: bool,
    pub auto_hide: bool,
    pub db_path: String,
    pub sort_by_created: bool,  // true=按创建时间排序, false=按更新时间排序
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
            let _ = std::fs::write(&path, content);
        }
    }

    fn config_path() -> PathBuf {
        PathBuf::from(CONFIG_FILE)
    }

    pub fn resolve_db_path(&self) -> PathBuf {
        if self.db_path.is_empty() {
            if let Ok(exe) = std::env::current_exe() {
                exe.parent().unwrap_or(&std::path::Path::new(".")).join("clippi.db")
            } else {
                PathBuf::from("clippi.db")
            }
        } else {
            PathBuf::from(&self.db_path)
        }
    }
}

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
