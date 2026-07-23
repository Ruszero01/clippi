//! Platform-aware path resolution for config and data files

use base64::Engine as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

const APP_DIR_NAME: &str = "Clippi";
const CONFIG_FILE: &str = "clippi.toml";
const DB_FILE: &str = "clippi.db";

/// Cached portable-mode flag — true when the exe directory is writable.
static IS_PORTABLE: AtomicBool = AtomicBool::new(false);

/// Initialise portable mode detection. Call once at startup.
///
/// Portable mode is active when a config file (`clippi.toml`) already exists
/// in the executable directory. This is more reliable than probing for
/// writability — on Windows, the writable test can give inconsistent results
/// across runs (UAC, antivirus, filesystem quirks), causing the config and
/// database location to oscillate between exe_dir and app_data_dir.
pub fn init_portable_mode() {
    let exe = exe_dir();
    let portable = exe.join(CONFIG_FILE).exists();
    IS_PORTABLE.store(portable, Ordering::Relaxed);
    log::info!("Portable mode: {} (exe_dir: {})", portable, exe.display());
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

/// Returns the database path when in portable mode (exe_dir / clippi.db).
/// This is the explicit portable DB path, independent of `is_portable_mode()`.
pub fn portable_db_path() -> PathBuf {
    exe_dir().join(DB_FILE)
}

/// Returns the system data directory (`%LOCALAPPDATA%/Clippi/` on Windows,
/// `~/Library/Application Support/Clippi/` on macOS).
pub fn system_data_dir() -> PathBuf {
    app_data_dir()
}

/// Merge images from the source data directory into the target data directory.
///
/// Copies only files that don't already exist in the target. Directories are
/// traversed recursively. Non-fatal: logs warnings on individual copy failures
/// and continues.
pub fn merge_images_dir(source_db_path: &Path, target_db_path: &Path) -> usize {
    let src_images = resolve_data_dir(source_db_path.to_string_lossy().as_ref()).join("images");
    let dst_images = resolve_data_dir(target_db_path.to_string_lossy().as_ref()).join("images");

    if !src_images.is_dir() {
        return 0;
    }
    if let Err(e) = fs::create_dir_all(&dst_images) {
        log::warn!(
            "merge_images: failed to create target dir {}: {e}",
            dst_images.display()
        );
        return 0;
    }

    let mut copied = 0usize;
    copy_missing_recursive(&src_images, &dst_images, &mut copied);
    log::info!(
        "merge_images: copied {copied} files from {} to {}",
        src_images.display(),
        dst_images.display()
    );
    copied
}

/// Recursively copy files from `src` to `dst` that don't exist in `dst`.
fn copy_missing_recursive(src: &Path, dst: &Path, count: &mut usize) {
    let entries = match fs::read_dir(src) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("merge_images: failed to read dir {}: {e}", src.display());
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let dest = dst.join(entry.file_name());
        if path.is_dir() {
            copy_missing_recursive(&path, &dest, count);
        } else if !dest.exists() && fs::copy(&path, &dest).is_ok() {
            *count += 1;
        }
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
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    app_icon_dir().join(format!("{sanitized}.png"))
}

/// Return the standard cached application-icon path, writing the supplied PNG
/// payload only when the cache file does not already exist.
pub fn cache_app_icon(app_name: &str, icon_base64: &str) -> Option<PathBuf> {
    if app_name.trim().is_empty() {
        return None;
    }
    let path = app_icon_path(app_name);
    if path.exists() {
        return Some(path);
    }
    if icon_base64.is_empty() {
        return None;
    }
    match base64::engine::general_purpose::STANDARD.decode(icon_base64) {
        Ok(png) => match fs::write(&path, png) {
            Ok(()) => Some(path),
            Err(error) => {
                log::warn!(
                    "cache_app_icon: failed to write {}: {error}",
                    path.display()
                );
                None
            }
        },
        Err(error) => {
            log::warn!("cache_app_icon: invalid icon for '{app_name}': {error}");
            None
        }
    }
}

static RESOLVED_IMAGES_DIR: OnceLock<PathBuf> = OnceLock::new();
static RESOLVED_TRANSFER_CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Initialize the resolved images directory based on db_path.
/// Must be called once at startup before any clipboard capture.
pub fn init_images_dir(db_path: &str) {
    let data_dir = resolve_data_dir(db_path);
    let dir = data_dir.join("images");
    // Pre-create icon and file-icon cache directories at startup
    // so the render path never needs filesystem writes.
    let _ = std::fs::create_dir_all(dir.join("icons"));
    let _ = std::fs::create_dir_all(dir.join("file_icons"));
    let _ = RESOLVED_IMAGES_DIR.set(dir);
    let _ = RESOLVED_TRANSFER_CACHE_DIR.set(data_dir.join("file_cache"));
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

/// Directory for transfer station cached files.
///
/// Files are stored as `{hash}/{portable_name}` and are only used for local access —
/// status determination ("cloud" vs "local") is done via DB comparison.
pub fn transfer_cache_dir() -> PathBuf {
    RESOLVED_TRANSFER_CACHE_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| resolve_data_dir("").join("file_cache"))
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
    // --- Portable mode — data lives in exe dir, no legacy migration needed. ---
    if is_portable_mode() {
        log::info!("Portable mode: skipping legacy migration");
        return;
    }

    // Always ensure data directory exists (for fresh installs and after migration)
    if let Err(e) = ensure_app_data_dir() {
        log::error!("failed to create data directory: {e}");
        return;
    }

    // --- Find legacy files in exe's parent directory ---
    let Some(legacy_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
    else {
        return;
    };

    migrate_legacy_files_from(&legacy_dir, &app_data_dir());
}

fn migrate_legacy_files_from(legacy_dir: &Path, data_dir: &Path) {
    if legacy_dir == data_dir {
        return;
    }
    if let Err(e) = fs::create_dir_all(data_dir) {
        log::error!("failed to create data directory: {e}");
        return;
    }

    let legacy_config = legacy_dir.join(CONFIG_FILE);
    let new_config = data_dir.join(CONFIG_FILE);
    if legacy_config.exists() && !new_config.exists() {
        if let Err(e) = fs::copy(&legacy_config, &new_config) {
            log::error!("failed to migrate config: {e}");
        }
    }

    let legacy_db = legacy_dir.join(DB_FILE);
    let new_db = data_dir.join(DB_FILE);
    if legacy_db.exists() && !new_db.exists() {
        if let Err(e) = fs::copy(&legacy_db, &new_db) {
            log::error!("failed to migrate database: {e}");
        } else {
            for suffix in ["-wal", "-shm"] {
                let legacy_companion = legacy_dir.join(format!("{DB_FILE}{suffix}"));
                let new_companion = data_dir.join(format!("{DB_FILE}{suffix}"));
                if legacy_companion.exists() {
                    let _ = fs::copy(legacy_companion, new_companion);
                }
            }
        }
    }

    let legacy_images = legacy_dir.join("images");
    if legacy_images.is_dir() {
        copy_dir_recursive_missing(&legacy_images, &data_dir.join("images"));
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

    // --- Copy database ---
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

    // --- Copy images directory (recursive) ---
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

fn copy_dir_recursive_missing(src: &Path, dst: &Path) {
    if let Err(e) = fs::create_dir_all(dst) {
        log::warn!(
            "failed to create migration directory {}: {e}",
            dst.display()
        );
        return;
    }
    let entries = match fs::read_dir(src) {
        Ok(entries) => entries,
        Err(e) => {
            log::warn!("failed to read migration directory {}: {e}", src.display());
            return;
        }
    };
    for entry in entries.flatten() {
        let source = entry.path();
        let destination = dst.join(entry.file_name());
        if source.is_dir() {
            copy_dir_recursive_missing(&source, &destination);
        } else if !destination.exists() {
            if let Err(e) = fs::copy(&source, &destination) {
                log::warn!("failed to migrate {}: {e}", source.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{migrate_legacy_files_from, CONFIG_FILE, DB_FILE};

    #[test]
    fn legacy_migration_copies_missing_data_when_config_already_exists() {
        let root = std::env::temp_dir().join(format!(
            "clippi-paths-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let legacy = root.join("legacy");
        let data = root.join("data");
        std::fs::create_dir_all(legacy.join("images")).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(legacy.join(CONFIG_FILE), "legacy").unwrap();
        std::fs::write(legacy.join(DB_FILE), "database").unwrap();
        std::fs::write(legacy.join("images").join("image.png"), "image").unwrap();
        std::fs::write(data.join(CONFIG_FILE), "existing").unwrap();

        migrate_legacy_files_from(&legacy, &data);

        assert_eq!(
            std::fs::read_to_string(data.join(CONFIG_FILE)).unwrap(),
            "existing"
        );
        assert_eq!(
            std::fs::read_to_string(data.join(DB_FILE)).unwrap(),
            "database"
        );
        assert_eq!(
            std::fs::read_to_string(data.join("images").join("image.png")).unwrap(),
            "image"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn merge_images_copies_missing_files() {
        let root = std::env::temp_dir().join(format!(
            "clippi-images-merge-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let src_db = root.join("src").join("clippi.db");
        let tgt_db = root.join("tgt").join("clippi.db");

        // Source: images/ dir with one file.
        let src_images = root.join("src").join("images");
        std::fs::create_dir_all(&src_images).unwrap();
        std::fs::write(src_images.join("a.png"), "a").unwrap();

        // Target: images/ dir with a different file.
        let tgt_images = root.join("tgt").join("images");
        std::fs::create_dir_all(&tgt_images).unwrap();
        std::fs::write(tgt_images.join("b.png"), "b").unwrap();

        let copied = super::merge_images_dir(&src_db, &tgt_db);
        assert_eq!(copied, 1); // a.png copied, b.png already exists

        // Verify a.png was copied.
        assert!(tgt_images.join("a.png").exists());
        assert_eq!(
            std::fs::read_to_string(tgt_images.join("a.png")).unwrap(),
            "a"
        );
        // b.png unchanged.
        assert_eq!(
            std::fs::read_to_string(tgt_images.join("b.png")).unwrap(),
            "b"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn merge_images_skips_when_source_missing() {
        let root = std::env::temp_dir().join(format!(
            "clippi-images-no-src-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // Source has NO images dir.
        let src_db = root.join("src").join("clippi.db");
        std::fs::create_dir_all(root.join("src")).unwrap();

        let tgt_db = root.join("tgt").join("clippi.db");
        let tgt_images = root.join("tgt").join("images");
        std::fs::create_dir_all(&tgt_images).unwrap();

        let copied = super::merge_images_dir(&src_db, &tgt_db);
        assert_eq!(copied, 0);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn merge_images_idempotent() {
        let root = std::env::temp_dir().join(format!(
            "clippi-images-idem-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let src_db = root.join("src").join("clippi.db");
        let tgt_db = root.join("tgt").join("clippi.db");

        let src_images = root.join("src").join("images");
        std::fs::create_dir_all(&src_images).unwrap();
        std::fs::write(src_images.join("x.png"), "x").unwrap();

        let tgt_images = root.join("tgt").join("images");
        std::fs::create_dir_all(&tgt_images).unwrap();

        let copied1 = super::merge_images_dir(&src_db, &tgt_db);
        assert_eq!(copied1, 1);

        // Second merge: nothing new.
        let copied2 = super::merge_images_dir(&src_db, &tgt_db);
        assert_eq!(copied2, 0);

        std::fs::remove_dir_all(root).unwrap();
    }
}
