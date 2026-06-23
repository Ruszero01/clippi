//! Update orchestrator — coordinates check → download → verify → install → restart.
//!
//! High-level API consumed by `WindowManager`. All blocking work runs on
//! background threads; progress is communicated via `Arc<Mutex<UpdatePhase>>`.

use std::path::Path;
#[cfg(target_os = "macos")]
use std::path::PathBuf;

use super::update::{UpdateInfo, UpdatePhase};

/// Download, verify, and install an update. Blocking — call from background thread.
/// `phase_callback` is invoked on each phase transition for UI updates.
pub fn download_and_install(
    info: &UpdateInfo,
    phase_callback: impl Fn(UpdatePhase) + Send + 'static,
) -> Result<(), String> {
    let temp_dir = super::install::update_temp_dir();
    let _ = std::fs::create_dir_all(&temp_dir);

    let dest_path = temp_dir.join(&info.asset_name);

    // 1. Download
    phase_callback(UpdatePhase::Downloading { progress: 0 });

    let progress = super::downloader::DownloadProgress::new(info.asset_size);

    // Sub-spawn to poll progress and call back (this is still inside a
    // background thread — the callback writes to the shared phase mutex)
    super::downloader::download_file(&info.download_url, &dest_path, &progress)?;

    let pct = progress.percentage();
    phase_callback(UpdatePhase::Downloading { progress: pct });

    // 2. Verify
    phase_callback(UpdatePhase::Verifying);

    let expected_hash = super::downloader::fetch_checksum(&info.checksum_url)?;
    super::downloader::verify_sha256(&dest_path, &expected_hash)?;

    // 3. Install
    phase_callback(UpdatePhase::Installing);

    install_asset(&dest_path)?;

    phase_callback(UpdatePhase::ReadyToRestart);
    Ok(())
}

/// Install the downloaded asset based on current platform and mode.
fn install_asset(asset_path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        super::install::run_nsis_installer(asset_path)
    }

    #[cfg(target_os = "macos")]
    {
        let app_path = super::install::mount_dmg(asset_path)?;
        // Determine the mount volume for later unmounting
        let mount = app_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("/Volumes/Clippi"));
        super::install::replace_app_bundle(&app_path)?;
        super::install::unmount_dmg(&mount);
        Ok(())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = asset_path;
        Err("Platform not supported".into())
    }
}

/// Clean up old temp files from previous updates. Call at startup.
pub fn cleanup_temp() {
    let temp_dir = super::install::update_temp_dir();
    if temp_dir.exists() {
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
