use std::path::PathBuf;
use std::process::Command;

use winreg::enums::{HKEY_CURRENT_USER, KEY_WRITE, KEY_READ};
use winreg::RegKey;

const AUTOSTART_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APP_NAME: &str = "Clippi";

// ========== 开机自启 ==========

pub fn is_auto_start_enabled() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = hkcu.open_subkey_with_flags(AUTOSTART_KEY_PATH, KEY_READ) else {
        return false;
    };
    let Ok(exe_path) = std::env::current_exe() else {
        return false;
    };
    match key.get_value::<String, _>(APP_NAME) {
        Ok(val) => val == exe_path.to_string_lossy(),
        Err(_) => false,
    }
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

// ========== 数据库迁移 ==========

pub fn migrate_database(old_path: &PathBuf, new_path: &PathBuf) -> Result<(), String> {
    if *new_path == *old_path {
        return Err("新路径与当前路径相同".into());
    }

    // 确保目标目录存在
    if let Some(parent) = new_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录失败: {e}"))?;
    }

    std::fs::copy(old_path, new_path)
        .map_err(|e| format!("复制数据库失败: {e}"))?;

    Ok(())
}

pub fn get_default_db_path() -> PathBuf {
    PathBuf::from("clippi.db")
}

/// 启动新进程（由调用者负责退出事件循环）
pub fn spawn_new_process() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = Command::new(exe).spawn();
    }
}
