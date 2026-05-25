//! Platform-aware path resolution for config and data files

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

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

/// Log file path — follows the database directory so custom db_path
/// users get the log file next to their database.
pub fn log_path(db_setting: &str) -> PathBuf {
    let db = resolve_db_path(db_setting);
    let dir = db.parent().unwrap_or_else(|| std::path::Path::new("."));
    dir.join("clippi.log")
}

pub fn app_icon_dir() -> PathBuf {
    let dir = images_dir().join("icons");
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
}

/// File path for a cached app icon (sanitized app name → PNG filename).
pub fn app_icon_path(app_name: &str) -> PathBuf {
    let sanitized: String = app_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    app_icon_dir().join(format!("{sanitized}.png"))
}

static RESOLVED_IMAGES_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Initialize the resolved images directory based on db_path.
/// Must be called once at startup before any clipboard capture.
pub fn init_images_dir(db_path: &str) {
    let dir = if db_path.is_empty() {
        app_data_dir().join("images")
    } else {
        PathBuf::from(db_path)
            .parent()
            .map(|p| p.join("images"))
            .unwrap_or_else(|| app_data_dir().join("images"))
    };
    let _ = RESOLVED_IMAGES_DIR.set(dir);
}

pub fn images_dir() -> PathBuf {
    let dir = RESOLVED_IMAGES_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| app_data_dir().join("images"));
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
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
        log::error!("failed to create data directory: {e}");
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
            log::error!("failed to migrate config: {e}");
        }
    }

    if legacy_db.exists() {
        let new_db = data_dir.join(DB_FILE);
        if let Err(e) = fs::copy(&legacy_db, &new_db) {
            log::error!("failed to migrate database: {e}");
        }
    }
}
