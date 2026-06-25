//! Platform-specific install/replace logic for updates.
//!
//! All functions are blocking — call from a background thread.

use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};

/// Temp directory for downloads. Cleaned on startup.
pub fn update_temp_dir() -> PathBuf {
    std::env::temp_dir().join("clippi-update")
}

// ─── Windows ──────────────────────────────────────────────────────────

/// Run the NSIS installer with admin privileges, non-silent.
/// Uses ShellExecuteW with the `runas` verb to trigger a UAC prompt.
/// The user goes through the installer wizard manually.
/// The installer handles restarting Clippi after installation completes.
#[cfg(target_os = "windows")]
pub fn launch_nsis_installer(installer_path: &Path) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;

    let exe = installer_path.to_str().ok_or("Invalid installer path")?;
    let exe_wide: Vec<u16> = exe.encode_utf16().chain(std::iter::once(0)).collect();
    let verb = "runas\0".encode_utf16().collect::<Vec<u16>>();

    // SAFETY: ShellExecuteW with known string pointers — safe call.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(), // parent window
            verb.as_ptr(),        // "runas" — triggers UAC elevation
            exe_wide.as_ptr(),    // installer path
            std::ptr::null(),     // no silent flag — user runs installer manually
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

/// Extract and validate the application from a DMG into the update directory.
#[cfg(target_os = "macos")]
pub fn prepare_macos_app(dmg_path: &Path, update_dir: &Path) -> Result<PathBuf, String> {
    let mount_point = update_dir.join("mount");
    if mount_point.exists() {
        let _ = Command::new("hdiutil")
            .args(["detach", "-force"])
            .arg(&mount_point)
            .status();
        std::fs::remove_dir_all(&mount_point)
            .map_err(|e| format!("Cannot reset DMG mount directory: {e}"))?;
    }
    std::fs::create_dir_all(&mount_point)
        .map_err(|e| format!("Cannot create DMG mount directory: {e}"))?;

    let output = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(&mount_point)
        .arg(dmg_path)
        .output()
        .map_err(|e| format!("hdiutil attach failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Cannot mount DMG: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let result = (|| {
        let source_app = std::fs::read_dir(&mount_point)
            .map_err(|e| format!("Cannot read DMG: {e}"))?
            .filter_map(Result::ok)
            .find(|entry| entry.path().extension().is_some_and(|ext| ext == "app"))
            .map(|entry| entry.path())
            .ok_or("No .app bundle found in DMG")?;
        let staged_app = update_dir.join("Clippi.app");
        if staged_app.exists() {
            std::fs::remove_dir_all(&staged_app)
                .map_err(|e| format!("Cannot remove previous staged app: {e}"))?;
        }
        let status = Command::new("ditto")
            .arg(&source_app)
            .arg(&staged_app)
            .status()
            .map_err(|e| format!("Cannot extract application: {e}"))?;
        if !status.success() {
            return Err("ditto failed while extracting the application".into());
        }
        let executable = staged_app.join("Contents/MacOS/clippi");
        if !executable.is_file() {
            return Err("Downloaded application is missing its executable".into());
        }
        let verify = Command::new("codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(&staged_app)
            .output()
            .map_err(|e| format!("Cannot verify application signature: {e}"))?;
        if !verify.status.success() {
            return Err(format!(
                "Application signature verification failed: {}",
                String::from_utf8_lossy(&verify.stderr).trim()
            ));
        }
        Ok(staged_app)
    })();

    let detach = Command::new("hdiutil")
        .args(["detach", "-force"])
        .arg(&mount_point)
        .output();
    if let Ok(output) = detach {
        if !output.status.success() {
            log::warn!(
                "Failed to detach update DMG: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }
    result
}

/// Launch a helper that waits for this process to exit, atomically replaces the
/// current application bundle, and opens the new version.
#[cfg(target_os = "macos")]
pub fn launch_macos_installer(staged_app: &Path) -> Result<(), String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot get current exe: {e}"))?;
    let app_bundle = current_exe
        .ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
        .ok_or("Clippi must be launched from an .app bundle to update")?;
    let bundle_path = app_bundle.to_string_lossy();
    if bundle_path.contains("/AppTranslocation/") || bundle_path.starts_with("/Volumes/") {
        return Err("Move Clippi to Applications before installing updates".into());
    }
    if !staged_app.join("Contents/MacOS/clippi").is_file() {
        return Err("The prepared update is missing; download it again".into());
    }

    let script_path = update_temp_dir().join("apply-update.sh");
    std::fs::write(&script_path, MACOS_INSTALL_SCRIPT)
        .map_err(|e| format!("Cannot create update helper: {e}"))?;

    let pid = std::process::id().to_string();
    let parent = app_bundle
        .parent()
        .ok_or("Application bundle has no parent directory")?;
    let writable = path_is_writable(parent);
    if writable {
        Command::new("/bin/sh")
            .arg(&script_path)
            .arg(&pid)
            .arg(staged_app)
            .arg(app_bundle)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Cannot launch update helper: {e}"))?;
    } else {
        let command = format!(
            "nohup /bin/sh {} {} {} {} >/tmp/clippi-update.log 2>&1 &",
            shell_quote(&script_path.to_string_lossy()),
            shell_quote(&pid),
            shell_quote(&staged_app.to_string_lossy()),
            shell_quote(&app_bundle.to_string_lossy())
        );
        let apple_script = format!(
            "do shell script \"{}\" with administrator privileges",
            command.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let output = Command::new("osascript")
            .args(["-e", &apple_script])
            .output()
            .map_err(|e| format!("Cannot request permission to update: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "Update permission was not granted: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn path_is_writable(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    CString::new(path.as_os_str().as_bytes())
        .ok()
        .is_some_and(|path| unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 })
}

#[cfg(target_os = "macos")]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(target_os = "macos")]
const MACOS_INSTALL_SCRIPT: &str = r#"#!/bin/sh
set -u
pid="$1"
staged="$2"
target="$3"
attempt=0
while kill -0 "$pid" 2>/dev/null && [ "$attempt" -lt 300 ]; do
  sleep 0.1
  attempt=$((attempt + 1))
done
if kill -0 "$pid" 2>/dev/null; then
  exit 1
fi
backup="${target}.clippi-backup-$$"
cleanup() { rm -rf "$backup"; }
trap cleanup EXIT
if [ -e "$target" ]; then
  mv "$target" "$backup" || exit 1
fi
if /usr/bin/ditto "$staged" "$target"; then
  /usr/bin/xattr -dr com.apple.quarantine "$target" 2>/dev/null || true
  rm -rf "$backup" "$staged"
  /usr/bin/open "$target"
  exit 0
fi
rm -rf "$target"
if [ -e "$backup" ]; then
  mv "$backup" "$target"
fi
exit 1
"#;
