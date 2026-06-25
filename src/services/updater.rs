//! Update orchestrator — coordinates download → verify → prepare → restart.
//!
//! High-level API consumed by `WindowManager`. All blocking work runs on
//! background threads; progress is communicated via `Arc<Mutex<UpdatePhase>>`.

use std::path::Path;

use super::update::{UpdateInfo, UpdatePhase};

/// Download, verify, and prepare an update. Blocking — call from background thread.
/// `phase_callback` is invoked on each phase transition for UI updates.
pub fn download_and_prepare(
    info: &UpdateInfo,
    phase_callback: impl Fn(UpdatePhase) + Send + 'static,
) -> Result<(), String> {
    // Clean up previous downloads so only the latest installer is kept.
    cleanup_temp();
    let temp_dir = super::install::update_temp_dir();
    let _ = std::fs::create_dir_all(&temp_dir);

    let dest_path = temp_dir.join(&info.asset_name);

    // 1. Download
    phase_callback(UpdatePhase::Downloading { progress: 0 });

    super::downloader::download_file(
        &info.download_url,
        &dest_path,
        info.asset_size,
        |progress| {
            phase_callback(UpdatePhase::Downloading { progress });
        },
    )?;

    // 2. Verify
    phase_callback(UpdatePhase::Verifying);

    let expected_hash = super::downloader::fetch_checksum(&info.checksum_url)?;
    super::downloader::verify_sha256(&dest_path, &expected_hash)?;

    // 3. Prepare the platform artifact. Installation happens only after the
    // user clicks restart, so the running executable is never replaced here.
    phase_callback(UpdatePhase::Installing);
    prepare_asset(&dest_path, &temp_dir)?;

    phase_callback(UpdatePhase::ReadyToRestart);
    Ok(())
}

/// Prepare the downloaded asset for installation after the app exits.
fn prepare_asset(asset_path: &Path, temp_dir: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let _ = (asset_path, temp_dir);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        super::install::prepare_macos_app(asset_path, temp_dir).map(|_| ())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = asset_path;
        Err("Platform not supported".into())
    }
}

/// Launch the prepared platform installer. The caller must quit immediately
/// after this returns successfully.
pub fn launch_prepared_update(info: &UpdateInfo) -> Result<(), String> {
    let temp_dir = super::install::update_temp_dir();
    #[cfg(target_os = "windows")]
    {
        super::install::launch_nsis_installer(&temp_dir.join(&info.asset_name))
    }
    #[cfg(target_os = "macos")]
    {
        let _ = info;
        super::install::launch_macos_installer(&temp_dir.join("Clippi.app"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (info, temp_dir);
        Err("Platform not supported".into())
    }
}

/// Clean up old temp files from previous updates. Call at startup.
pub fn cleanup_temp() {
    let temp_dir = super::install::update_temp_dir();
    #[cfg(target_os = "macos")]
    {
        let mount_point = temp_dir.join("mount");
        if mount_point.exists() {
            let _ = std::process::Command::new("hdiutil")
                .args(["detach", "-force"])
                .arg(&mount_point)
                .status();
        }
    }
    if temp_dir.exists() {
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
