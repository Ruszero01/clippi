//! Platform-aware path resolution for config and data files

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

const APP_DIR_NAME: &str = "Clippi";
const CONFIG_FILE: &str = "clippi.toml";
const DB_FILE: &str = "clippi.db";

/// Cached portable-mode flag — true when the exe directory is writable.
static IS_PORTABLE: AtomicBool = AtomicBool::new(false);

/// Initialise portable mode detection. Call once at startup.
pub fn init_portable_mode() {
    let exe = exe_dir();
    // Try to create + delete a temp file to test writability.
    let probe = exe.join(".clippi_writable_test");
    let writable = std::fs::write(&probe, b"1").is_ok();
    if writable {
        let _ = std::fs::remove_file(&probe);
    }
    IS_PORTABLE.store(writable, Ordering::Relaxed);
    log::info!(
        "Portable mode: {} (exe_dir: {})",
        writable,
        exe.display()
    );
}

/// The directory containing the running executable.
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Returns true when the exe directory is writable (portable mode active).
pub fn is_portable_mode() -> bool {
    IS_PORTABLE.load(Ordering::Relaxed)
}

fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
}

/// Config file path — always in the base directory (exe_dir if portable,
/// otherwise platform data dir). Never affected by user db_path changes.
pub fn config_path() -> PathBuf {
    if is_portable_mode() {
        exe_dir().join(CONFIG_FILE)
    } else {
        app_data_dir().join(CONFIG_FILE)
    }
}

/// Resolve the database path.
///
/// - If `db_setting` is non-empty, use it directly (user override).
/// - Otherwise, default to exe_dir (portable) or platform data dir.
pub fn resolve_db_path(db_setting: &str) -> PathBuf {
    if !db_setting.is_empty() {
        PathBuf::from(db_setting)
    } else if is_portable_mode() {
        exe_dir().join(DB_FILE)
    } else {
        app_data_dir().join(DB_FILE)
    }
}

/// Log file path — always in the base directory (same as config, not next to DB).
/// No longer takes a `db_path` argument; log location is independent of data dir.
pub fn log_path() -> PathBuf {
    let base = if is_portable_mode() {
        exe_dir()
    } else {
        app_data_dir()
    };
    base.join("clippi.log")
}

/// The directory that contains database + images (resolved from db_setting).
/// Used by init_images_dir to determine where to store clipboard images.
pub fn resolve_data_dir(db_setting: &str) -> PathBuf {
    let db = resolve_db_path(db_setting);
    db.parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
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
    let dir = resolve_data_dir(db_path).join("images");
    let _ = RESOLVED_IMAGES_DIR.set(dir);
}

pub fn images_dir() -> PathBuf {
    let dir = RESOLVED_IMAGES_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| resolve_data_dir("").join("images"));
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
}

/// Directory containing config and log files (exe_dir or app_data_dir).
pub fn config_dir() -> PathBuf {
    if is_portable_mode() {
        exe_dir()
    } else {
        app_data_dir()
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
    // Portable mode — data lives in exe dir, no legacy migration needed.
    if is_portable_mode() {
        log::info!("Portable mode: skipping legacy migration");
        return;
    }

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

/// In portable mode, migrate existing data from the system data directory
/// to the exe directory. This handles the case where a user previously ran
/// Clippi as a non-portable install (data in %LOCALAPPDATA%/Clippi/) and
/// then upgrades to a portable install (exe in a writable directory).
///
/// Only migrates if:
/// - Portable mode is active (exe dir writable)
/// - System data dir has an existing database
/// - Exe dir does NOT already have a database (avoid overwriting)
///
/// Copies: clippi.db, clippi.toml (if exists), images/ directory (if exists).
/// Non-fatal: logs warnings on failure, the app will start with a fresh DB.
pub fn migrate_portable_data() {
    if !is_portable_mode() {
        return;
    }

    let system_db = app_data_dir().join(DB_FILE);
    let portable_db = exe_dir().join(DB_FILE);

    // Only migrate if system has data and portable doesn't
    if !system_db.exists() {
        log::info!("Portable: no system data to migrate");
        return;
    }
    if portable_db.exists() {
        log::info!("Portable: exe dir already has data, skipping migration");
        return;
    }

    log::info!(
        "Portable: migrating data from {} to {}",
        app_data_dir().display(),
        exe_dir().display()
    );

    // Copy database
    if let Err(e) = fs::copy(&system_db, &portable_db) {
        log::error!(
            "Portable: failed to migrate database from {} to {}: {e}",
            system_db.display(),
            portable_db.display()
        );
        return;
    }

    // Copy config (if exists and not already present)
    let system_config = app_data_dir().join(CONFIG_FILE);
    let portable_config = exe_dir().join(CONFIG_FILE);
    if system_config.exists() && !portable_config.exists() {
        if let Err(e) = fs::copy(&system_config, &portable_config) {
            log::warn!("Portable: failed to migrate config: {e}");
        }
    }

    // Copy log file (if exists)
    let system_log = app_data_dir().join("clippi.log");
    let portable_log = exe_dir().join("clippi.log");
    if system_log.exists() && !portable_log.exists() {
        if let Err(e) = fs::copy(&system_log, &portable_log) {
            log::warn!("Portable: failed to migrate log: {e}");
        }
    }

    // Copy images directory (recursive)
    let system_images = app_data_dir().join("images");
    let portable_images = exe_dir().join("images");
    if system_images.exists() && system_images.is_dir() {
        copy_dir_recursive(&system_images, &portable_images);
    }

    log::info!("Portable: data migration complete");
}

/// Recursively copy a directory. Non-fatal — logs and skips on errors.
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) {
    if let Err(e) = fs::create_dir_all(dst) {
        log::warn!("Portable: failed to create dir {}: {e}", dst.display());
        return;
    }
    let entries = match fs::read_dir(src) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("Portable: failed to read dir {}: {e}", src.display());
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let dest = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &dest);
        } else if let Err(e) = fs::copy(&path, &dest) {
            log::warn!("Portable: failed to copy {}: {e}", path.display());
        }
    }
}
