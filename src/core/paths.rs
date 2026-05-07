//! Platform-aware path resolution for config and data files

use std::fs;
use std::path::PathBuf;

const APP_DIR_NAME: &str = "Clippi";
const CONFIG_FILE: &str = "clippi.toml";
const DB_FILE: &str = "clippi.db";

fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
}

pub fn config_path() -> PathBuf {
    app_data_dir().join(CONFIG_FILE)
}

pub fn resolve_db_path(db_setting: &str) -> PathBuf {
    if db_setting.is_empty() {
        app_data_dir().join(DB_FILE)
    } else {
        PathBuf::from(db_setting)
    }
}

fn ensure_app_data_dir() -> std::io::Result<()> {
    let dir = app_data_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
    }
    Ok(())
}

/// One-time migration from legacy CWD/exe-relative paths to platform data dir.
/// Non-fatal: logs warnings on failure.
pub fn migrate_legacy_files() {
    // Always ensure data directory exists (for fresh installs and after migration)
    if let Err(e) = ensure_app_data_dir() {
        eprintln!("Warning: failed to create data directory: {e}");
        return;
    }

    let data_dir = app_data_dir();
    let new_config = data_dir.join(CONFIG_FILE);

    // Skip migration if new location already has config
    if new_config.exists() {
        return;
    }

    // Find legacy files in exe's parent directory
    let Some(legacy_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
    else {
        return;
    };

    let legacy_config = legacy_dir.join(CONFIG_FILE);
    let legacy_db = legacy_dir.join(DB_FILE);

    if legacy_config.exists() {
        if let Err(e) = fs::copy(&legacy_config, &new_config) {
            eprintln!("Warning: failed to migrate config: {e}");
        }
    }

    if legacy_db.exists() {
        let new_db = data_dir.join(DB_FILE);
        if let Err(e) = fs::copy(&legacy_db, &new_db) {
            eprintln!("Warning: failed to migrate database: {e}");
        }
    }
}
