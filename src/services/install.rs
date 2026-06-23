//! Platform-specific install/replace logic for updates.
//!
//! All functions are blocking — call from a background thread.

use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;

/// Temp directory for downloads. Cleaned on startup.
pub fn update_temp_dir() -> PathBuf {
    std::env::temp_dir().join("clippi-update")
}

// ─── Windows ──────────────────────────────────────────────────────────

/// Run the NSIS installer silently with admin privileges.
/// Uses ShellExecuteW with the `runas` verb to trigger a UAC prompt.
/// The installer handles restarting Clippi after installation completes.
#[cfg(target_os = "windows")]
pub fn run_nsis_installer(installer_path: &Path) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;

    let exe = installer_path.to_str().ok_or("Invalid installer path")?;
    let exe_wide: Vec<u16> = exe.encode_utf16().chain(std::iter::once(0)).collect();
    let args = "/S\0".encode_utf16().collect::<Vec<u16>>();
    let verb = "runas\0".encode_utf16().collect::<Vec<u16>>();

    // SAFETY: ShellExecuteW with known string pointers — safe call.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(), // parent window
            verb.as_ptr(),        // "runas" — triggers UAC elevation
            exe_wide.as_ptr(),    // installer path
            args.as_ptr(),        // "/S" silent
            std::ptr::null(),     // working directory
            SW_SHOW,
        )
    } as isize;

    // ShellExecuteW returns a value > 32 on success
    if result > 32 {
        Ok(())
    } else {
        Err(format!(
            "Failed to launch installer (ShellExecuteW returned {result})"
        ))
    }
}
// ─── macOS ────────────────────────────────────────────────────────────

/// Mount a DMG and return the path to the .app bundle inside.
#[cfg(target_os = "macos")]
pub fn mount_dmg(dmg_path: &Path) -> Result<PathBuf, String> {
    let output = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly"])
        .arg(dmg_path)
        .output()
        .map_err(|e| format!("hdiutil attach failed: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Parse mount point from hdiutil output (last line contains the mount path)
    let mount_point = stdout
        .lines()
        .last()
        .and_then(|line| line.split('\t').last())
        .map(|s| s.trim())
        .unwrap_or("");

    if mount_point.is_empty() {
        return Err("Cannot parse DMG mount point".into());
    }

    // Find the .app inside the mounted DMG
    let mount = PathBuf::from(mount_point);
    let app = std::fs::read_dir(&mount)
        .map_err(|e| format!("Cannot read DMG: {e}"))?
        .filter_map(|e| e.ok())
        .find(|e| e.path().extension().map_or(false, |ext| ext == "app"))
        .map(|e| e.path())
        .ok_or("No .app found in DMG")?;

    Ok(app)
}

/// Replace the current .app bundle with a new one.
/// macOS allows replacing a running .app bundle.
#[cfg(target_os = "macos")]
pub fn replace_app_bundle(new_app_path: &Path) -> Result<(), String> {
    // Find the current .app bundle path
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot get current exe: {e}"))?;

    // The exe is at Clippi.app/Contents/MacOS/clippi
    // Navigate up to find Clippi.app
    let app_bundle = current_exe
        .ancestors()
        .find(|p| p.extension().map_or(false, |ext| ext == "app"))
        .ok_or("Cannot find .app bundle")?;

    let target_dir = app_bundle
        .parent()
        .unwrap_or_else(|| Path::new("/Applications"));

    // Use `cp -R` to replace the bundle contents
    let status = Command::new("cp")
        .arg("-R")
        .arg(new_app_path)
        .arg(target_dir)
        .status()
        .map_err(|e| format!("Cannot copy .app: {e}"))?;

    if !status.success() {
        return Err("cp -R failed".into());
    }
    Ok(())
}

/// Unmount a DMG volume.
#[cfg(target_os = "macos")]
pub fn unmount_dmg(mount_point: &Path) {
    let _ = Command::new("hdiutil")
        .args(["detach", "-force"])
        .arg(mount_point)
        .spawn();
}
