//! Update checker — queries GitHub Releases API, compares versions via semver,
//! --- caches results, and opens the releases page in the browser. ---

/// Info about the latest available release, if any.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub latest_version: String,
    /// GitHub Release body (markdown source).
    pub release_notes: String,
    /// Direct download URL for the platform-appropriate asset.
    pub download_url: String,
    /// SHA256 checksum file URL.
    pub checksum_url: String,
    /// Asset filename (for display + local temp path).
    pub asset_name: String,
    /// Asset size in bytes (0 if unknown).
    pub asset_size: u64,
}

/// Phase of the update process (for UI display).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdatePhase {
    Idle,
    Checking,
    UpToDate,
    UpdateAvailable,
    Downloading { progress: u8 },
    Verifying,
    Installing,
    ReadyToRestart,
    Error(String),
}

/// GitHub Releases update checker.
pub struct UpdateChecker {
    current_version: String,
    repo_owner: String,
    repo_name: String,
}

impl UpdateChecker {
    pub fn new(current_version: &str, repo_owner: &str, repo_name: &str) -> Self {
        Self {
            current_version: current_version.to_string(),
            repo_owner: repo_owner.to_string(),
            repo_name: repo_name.to_string(),
        }
    }

    /// Full check — version, release notes, and platform-appropriate asset.
    pub fn check_full(&self) -> Result<Option<UpdateInfo>, String> {
        self.fetch_latest_release_full()
    }

    /// Full fetch — version, release notes, and platform-appropriate asset with checksum.
    fn fetch_latest_release_full(&self) -> Result<Option<UpdateInfo>, String> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/releases/latest",
            self.repo_owner, self.repo_name
        );

        let agent = format!("Clippi/{}", self.current_version);

        let response = ureq::get(&url)
            .set("User-Agent", &agent)
            .set("Accept", "application/vnd.github.v3+json")
            .call()
            .map_err(|e| format!("Failed to query GitHub Releases: {e}"))?;

        let body = response
            .into_string()
            .map_err(|e| format!("Failed to read GitHub response: {e}"))?;
        let parsed: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("Invalid GitHub response: {e}"))?;

        let tag_name = parsed["tag_name"]
            .as_str()
            .ok_or("GitHub release is missing tag_name")?;

        // Strip leading 'v' if present
        let latest_ver = tag_name.strip_prefix('v').unwrap_or(tag_name);

        let current = semver::Version::parse(&self.current_version)
            .map_err(|e| format!("Invalid current version: {e}"))?;
        let latest = semver::Version::parse(latest_ver)
            .map_err(|e| format!("Invalid release version {latest_ver:?}: {e}"))?;

        if latest <= current {
            return Ok(None); // No update available
        }

        let release_notes = parsed["body"].as_str().unwrap_or("").to_string();
        let version_str = latest.to_string();

        // Select the right asset for the current platform
        let (download_url, checksum_url, asset_name, asset_size) =
            select_platform_asset(&parsed, &version_str)?;

        Ok(Some(UpdateInfo {
            latest_version: version_str,
            release_notes,
            download_url,
            checksum_url,
            asset_name,
            asset_size,
        }))
    }
}

/// Pick the right asset for the current platform from the release JSON.
fn select_platform_asset(
    release: &serde_json::Value,
    version: &str,
) -> Result<(String, String, String, u64), String> {
    let assets = release["assets"]
        .as_array()
        .ok_or("GitHub release is missing assets")?;

    // Build a map: filename → (download_url, size)
    let mut asset_map: std::collections::HashMap<&str, (&str, u64)> =
        std::collections::HashMap::new();
    for asset in assets {
        let name = asset["name"].as_str().unwrap_or("");
        let url = asset["browser_download_url"].as_str().unwrap_or("");
        let size = asset["size"].as_u64().unwrap_or(0);
        if !name.is_empty() && !url.is_empty() {
            asset_map.insert(name, (url, size));
        }
    }

    // Determine which asset name pattern to look for
    let (asset_pattern, checksum_pattern) = platform_asset_patterns(version)?;

    // Find the main asset — use ends_with so ".sha256" isn't falsely matched.
    let (main_name, (download_url, size)) = asset_map
        .iter()
        .find(|(name, _)| name.ends_with(&asset_pattern))
        .ok_or_else(|| format!("Release asset not found: {asset_pattern}"))?;

    // Find the corresponding checksum file.
    let checksum_url = asset_map
        .iter()
        .find(|(name, _)| name.ends_with(&checksum_pattern))
        .map(|(_, (url, _))| url.to_string())
        .ok_or_else(|| format!("Release checksum not found: {checksum_pattern}"))?;

    Ok((
        download_url.to_string(),
        checksum_url,
        main_name.to_string(),
        *size,
    ))
}

/// Returns (asset_name_fragment, checksum_name_fragment) for the current platform.
#[cfg(target_os = "windows")]
fn platform_asset_patterns(version: &str) -> Result<(String, String), String> {
    asset_patterns_for("windows", std::env::consts::ARCH, version)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn platform_asset_patterns(version: &str) -> Result<(String, String), String> {
    asset_patterns_for("macos", "aarch64", version)
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
fn platform_asset_patterns(version: &str) -> Result<(String, String), String> {
    asset_patterns_for("macos", "x86_64", version)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_asset_patterns(version: &str) -> Result<(String, String), String> {
    asset_patterns_for(std::env::consts::OS, std::env::consts::ARCH, version)
}

fn asset_patterns_for(os: &str, arch: &str, version: &str) -> Result<(String, String), String> {
    match (os, arch) {
        // Always use the NSIS installer on Windows — portable mode only affects
        // the data directory, not how the software itself is updated.
        ("windows", "x86_64") => Ok((
            format!("Clippi_{version}_x64-setup.exe"),
            format!("Clippi_{version}_x64-setup.exe.sha256"),
        )),
        ("macos", "aarch64") => Ok((
            "Clippi_aarch64.dmg".to_string(),
            "Clippi_aarch64.dmg.sha256".to_string(),
        )),
        ("macos", "x86_64") => Ok((
            "Clippi_x86_64.dmg".to_string(),
            "Clippi_x86_64.dmg.sha256".to_string(),
        )),
        _ => Err(format!(
            "Automatic updates are not supported on {os}/{arch}"
        )),
    }
}

/// Open the releases page in the system browser.
pub fn open_releases_page(url: &str) {
    #[cfg(target_os = "windows")]
    {
        let url_utf16: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            use windows_sys::Win32::UI::Shell::ShellExecuteW;
            use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;
            ShellExecuteW(
                std::ptr::null_mut(),
                "open\0".encode_utf16().collect::<Vec<u16>>().as_ptr(),
                url_utf16.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOW,
            );
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn release_asset_patterns_match_packaging_names() {
        assert_eq!(
            asset_patterns_for("windows", "x86_64", "1.2.3").unwrap(),
            (
                "Clippi_1.2.3_x64-setup.exe".into(),
                "Clippi_1.2.3_x64-setup.exe.sha256".into()
            )
        );
        assert_eq!(
            asset_patterns_for("macos", "aarch64", "1.2.3").unwrap(),
            (
                "Clippi_aarch64.dmg".into(),
                "Clippi_aarch64.dmg.sha256".into()
            )
        );
        assert_eq!(
            asset_patterns_for("macos", "x86_64", "1.2.3").unwrap(),
            (
                "Clippi_x86_64.dmg".into(),
                "Clippi_x86_64.dmg.sha256".into()
            )
        );
    }

    #[test]
    fn platform_asset_requires_matching_checksum() {
        let (asset, _) = platform_asset_patterns("1.2.3").unwrap();
        let release = json!({
            "assets": [{
                "name": asset,
                "browser_download_url": "https://example.invalid/update",
                "size": 42
            }]
        });
        assert!(select_platform_asset(&release, "1.2.3")
            .unwrap_err()
            .contains("checksum"));
    }
}
