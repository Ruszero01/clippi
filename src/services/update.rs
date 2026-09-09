//! Update checker — resolves the latest release from GitHub Releases or the
//! official Aliyun OSS update manifest, compares versions via semver, and
//! opens the releases page in the browser.
use chrono::{DateTime, Duration as ChronoDuration, Utc};

use crate::core::i18n_keys::I18nKey;

const SCHEDULED_UPDATE_CHECK_INTERVAL_HOURS: i64 = 24;

/// Public base URL of the official OSS release directory. Mirrors the URL used
/// by the website download buttons (`download.js`).
const OSS_RELEASES_BASE: &str =
    "https://rains-ailurus-cn.oss-cn-shanghai.aliyuncs.com/clippi/releases";

/// Update manifest published by the release workflow after every stable tag.
const OSS_MANIFEST_URL: &str =
    "https://rains-ailurus-cn.oss-cn-shanghai.aliyuncs.com/clippi/releases/latest.json";

/// Manifest schema understood by this client. Bump together with the publisher.
const OSS_MANIFEST_SCHEMA: u64 = 1;

/// Info about the latest available release, if any.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub latest_version: String,
    /// Release notes (markdown source).
    pub release_notes: String,
    /// Direct download URL for the platform-appropriate asset.
    pub download_url: String,
    /// SHA256 checksum file URL. Empty when `sha256` is embedded below.
    pub checksum_url: String,
    /// Asset filename (for display + local temp path).
    pub asset_name: String,
    /// Asset size in bytes (0 if unknown).
    pub asset_size: u64,
    /// Expected SHA256 embedded in the update manifest (OSS channel). `None`
    /// means the hash must be fetched from `checksum_url` (GitHub channel).
    pub sha256: Option<String>,
    /// Channel that supplied this update.
    pub source: UpdateSource,
}

/// User-selected update channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateChannel {
    /// Prefer the official OSS mirror, fall back to GitHub when unavailable.
    Auto,
    /// Official OSS mirror only.
    Oss,
    /// GitHub Releases only.
    GitHub,
}

impl UpdateChannel {
    /// Parse the persisted `settings.update_channel` value. Unknown values
    /// fall back to `Auto` so older or hand-edited configs keep working.
    pub fn from_setting(value: &str) -> Self {
        match value {
            "oss" => Self::Oss,
            "github" => Self::GitHub,
            _ => Self::Auto,
        }
    }
}

/// Channel that actually supplied an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateSource {
    Oss,
    GitHub,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateErrorKind {
    Network,
    Server,
    /// The selected channel (e.g. the OSS manifest) is not published yet or
    /// not publicly readable. `Auto` treats this as "try the other channel".
    ChannelUnavailable,
    InvalidResponse,
    Version,
    Package,
    UnsupportedPlatform,
    Download,
    Verify,
    Install,
    Launch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateCheckError {
    kind: UpdateErrorKind,
    detail: String,
}

impl UpdateCheckError {
    fn new(kind: UpdateErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub fn kind(&self) -> UpdateErrorKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub fn user_message(&self) -> String {
        user_message_for_kind(self.kind)
    }

    pub fn is_network_failure(&self) -> bool {
        self.kind == UpdateErrorKind::Network
    }
}

pub fn user_message_for_kind(kind: UpdateErrorKind) -> String {
    match kind {
        UpdateErrorKind::Network => I18nKey::UpdateErrNetwork.text().to_string(),
        UpdateErrorKind::Server => I18nKey::UpdateErrServer.text().to_string(),
        UpdateErrorKind::ChannelUnavailable => {
            I18nKey::UpdateErrChannelUnavailable.text().to_string()
        }
        UpdateErrorKind::InvalidResponse => I18nKey::UpdateErrResponse.text().to_string(),
        UpdateErrorKind::Version => I18nKey::UpdateErrVersion.text().to_string(),
        UpdateErrorKind::Package => I18nKey::UpdateErrPackage.text().to_string(),
        UpdateErrorKind::UnsupportedPlatform => I18nKey::UpdateErrUnsupported.text().to_string(),
        UpdateErrorKind::Download => I18nKey::UpdateErrDownload.text().to_string(),
        UpdateErrorKind::Verify => I18nKey::UpdateErrVerify.text().to_string(),
        UpdateErrorKind::Install => I18nKey::UpdateErrInstall.text().to_string(),
        UpdateErrorKind::Launch => I18nKey::UpdateErrLaunch.text().to_string(),
    }
}

pub fn summarize_update_error(error: &str) -> String {
    user_message_for_kind(classify_update_error(error))
}

fn classify_update_error(error: &str) -> UpdateErrorKind {
    let lower = error.to_ascii_lowercase();
    if lower.contains("checksum") || lower.contains("sha256") || lower.contains("verify") {
        return UpdateErrorKind::Verify;
    }
    if lower.contains("download failed")
        || lower.contains("cannot fetch checksum")
        || lower.contains("network")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("dns")
        || lower.contains("connection")
        || lower.contains("transport")
    {
        return UpdateErrorKind::Network;
    }
    if lower.contains("incomplete download") {
        return UpdateErrorKind::Download;
    }
    if lower.contains("not supported") || lower.contains("unsupported") {
        return UpdateErrorKind::UnsupportedPlatform;
    }
    if lower.contains("installer") || lower.contains("launch") || lower.contains("restart") {
        return UpdateErrorKind::Launch;
    }
    if lower.contains("create")
        || lower.contains("write")
        || lower.contains("read")
        || lower.contains("flush")
        || lower.contains("open")
        || lower.contains("permission")
        || lower.contains("access")
    {
        return UpdateErrorKind::Download;
    }
    UpdateErrorKind::Install
}

pub fn scheduled_update_check_due(last_check_at: &str, now: DateTime<Utc>) -> bool {
    scheduled_update_check_due_after(
        last_check_at,
        now,
        ChronoDuration::hours(SCHEDULED_UPDATE_CHECK_INTERVAL_HOURS),
    )
}

fn scheduled_update_check_due_after(
    last_check_at: &str,
    now: DateTime<Utc>,
    interval: ChronoDuration,
) -> bool {
    let trimmed = last_check_at.trim();
    if trimmed.is_empty() {
        return true;
    }
    let Ok(last) = DateTime::parse_from_rfc3339(trimmed) else {
        return true;
    };
    now.signed_duration_since(last.with_timezone(&Utc)) >= interval
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

    /// Resolve the latest release for the selected channel.
    ///
    /// `Auto` prefers the official OSS manifest and only falls back to GitHub
    /// when OSS is unavailable, so users in mainland China get a fast path
    /// without losing GitHub as a safety net.
    pub fn check_full(
        &self,
        channel: UpdateChannel,
    ) -> Result<Option<UpdateInfo>, UpdateCheckError> {
        match channel {
            UpdateChannel::Oss => self.check_oss(),
            UpdateChannel::GitHub => self.check_github(),
            UpdateChannel::Auto => with_auto_fallback(self.check_oss(), || self.check_github()),
        }
    }

    /// Official OSS manifest check.
    fn check_oss(&self) -> Result<Option<UpdateInfo>, UpdateCheckError> {
        let user_agent = format!("Clippi/{}", self.current_version);
        let http = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(10))
            .timeout_read(std::time::Duration::from_secs(20))
            .build();
        let response = http
            .get(OSS_MANIFEST_URL)
            .set("User-Agent", &user_agent)
            .set("Accept", "application/json")
            // A cached manifest would hide a fresh release from the client.
            .set("Cache-Control", "no-cache")
            .call()
            .map_err(classify_oss_query_error)?;

        let body = response
            .into_string()
            .map_err(|e| UpdateCheckError::new(UpdateErrorKind::InvalidResponse, e.to_string()))?;
        parse_oss_manifest(&body, &self.current_version)
    }

    /// GitHub Releases check (original behaviour).
    fn check_github(&self) -> Result<Option<UpdateInfo>, UpdateCheckError> {
        self.fetch_latest_release_full()
    }

    /// Full fetch — version, release notes, and platform-appropriate asset with checksum.
    fn fetch_latest_release_full(&self) -> Result<Option<UpdateInfo>, UpdateCheckError> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/releases/latest",
            self.repo_owner, self.repo_name
        );

        let user_agent = format!("Clippi/{}", self.current_version);

        let http = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(10))
            .timeout_read(std::time::Duration::from_secs(30))
            .build();
        let response = http
            .get(&url)
            .set("User-Agent", &user_agent)
            .set("Accept", "application/vnd.github.v3+json")
            .call()
            .map_err(classify_github_query_error)?;

        let body = response
            .into_string()
            .map_err(|e| UpdateCheckError::new(UpdateErrorKind::InvalidResponse, e.to_string()))?;
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| UpdateCheckError::new(UpdateErrorKind::InvalidResponse, e.to_string()))?;

        let tag_name = parsed["tag_name"].as_str().ok_or_else(|| {
            UpdateCheckError::new(UpdateErrorKind::InvalidResponse, "missing tag_name")
        })?;

        // Strip leading 'v' if present
        let latest_ver = tag_name.strip_prefix('v').unwrap_or(tag_name);

        let current = semver::Version::parse(&self.current_version)
            .map_err(|e| UpdateCheckError::new(UpdateErrorKind::Version, e.to_string()))?;
        let latest = semver::Version::parse(latest_ver)
            .map_err(|e| UpdateCheckError::new(UpdateErrorKind::Version, e.to_string()))?;

        if !latest.pre.is_empty() {
            // `/releases/latest` excludes prereleases, but never offer one if
            // the API ever returns it.
            return Ok(None);
        }

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
            sha256: None,
            source: UpdateSource::GitHub,
        }))
    }
}

/// Auto-channel policy: OSS first, GitHub only when OSS is unavailable.
fn with_auto_fallback(
    oss: Result<Option<UpdateInfo>, UpdateCheckError>,
    github: impl FnOnce() -> Result<Option<UpdateInfo>, UpdateCheckError>,
) -> Result<Option<UpdateInfo>, UpdateCheckError> {
    match oss {
        Ok(result) => Ok(result),
        Err(oss_error) => {
            log::warn!(
                "[update] OSS channel unavailable ({}), falling back to GitHub",
                oss_error.detail()
            );
            github().map_err(|github_error| combine_channel_errors(oss_error, github_error))
        }
    }
}

fn combine_channel_errors(oss: UpdateCheckError, github: UpdateCheckError) -> UpdateCheckError {
    // Prefer the primary channel's error unless it is only "channel
    // unavailable", in which case GitHub's error is the more informative one.
    let kind = if oss.kind() == UpdateErrorKind::ChannelUnavailable {
        github.kind()
    } else {
        oss.kind()
    };
    UpdateCheckError::new(
        kind,
        format!("OSS: {}; GitHub: {}", oss.detail(), github.detail()),
    )
}

fn classify_github_query_error(error: ureq::Error) -> UpdateCheckError {
    match error {
        ureq::Error::Status(status, response) => {
            let detail = response
                .into_string()
                .unwrap_or_else(|_| format!("GitHub returned HTTP {status}"));
            UpdateCheckError::new(UpdateErrorKind::Server, format!("HTTP {status}: {detail}"))
        }
        ureq::Error::Transport(transport) => {
            UpdateCheckError::new(UpdateErrorKind::Network, transport.to_string())
        }
    }
}

fn classify_oss_query_error(error: ureq::Error) -> UpdateCheckError {
    match error {
        ureq::Error::Status(status, response) => {
            let detail = response
                .into_string()
                .unwrap_or_else(|_| format!("OSS returned HTTP {status}"));
            let kind = if status == 403 || status == 404 {
                // Missing object or missing public-read ACL — either way the
                // channel is unusable, so `Auto` can fall back to GitHub.
                UpdateErrorKind::ChannelUnavailable
            } else {
                UpdateErrorKind::Server
            };
            UpdateCheckError::new(kind, format!("HTTP {status}: {detail}"))
        }
        ureq::Error::Transport(transport) => {
            UpdateCheckError::new(UpdateErrorKind::Network, transport.to_string())
        }
    }
}

/// Parse the OSS `latest.json` manifest for the current platform.
fn parse_oss_manifest(
    body: &str,
    current_version: &str,
) -> Result<Option<UpdateInfo>, UpdateCheckError> {
    parse_oss_manifest_for_platform(body, current_version, platform_key()?)
}

/// Platform-parameterised manifest parser (pure, so unit tests can exercise
/// every platform regardless of the host).
fn parse_oss_manifest_for_platform(
    body: &str,
    current_version: &str,
    platform: &str,
) -> Result<Option<UpdateInfo>, UpdateCheckError> {
    let parsed: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| UpdateCheckError::new(UpdateErrorKind::InvalidResponse, e.to_string()))?;

    let schema = parsed["schema"].as_u64().ok_or_else(|| {
        UpdateCheckError::new(UpdateErrorKind::InvalidResponse, "missing manifest schema")
    })?;
    if schema != OSS_MANIFEST_SCHEMA {
        return Err(UpdateCheckError::new(
            UpdateErrorKind::InvalidResponse,
            format!("unsupported manifest schema: {schema}"),
        ));
    }

    let version = parsed["version"].as_str().ok_or_else(|| {
        UpdateCheckError::new(UpdateErrorKind::InvalidResponse, "missing manifest version")
    })?;
    let version = version.strip_prefix('v').unwrap_or(version);

    let current = semver::Version::parse(current_version)
        .map_err(|e| UpdateCheckError::new(UpdateErrorKind::Version, e.to_string()))?;
    let latest = semver::Version::parse(version)
        .map_err(|e| UpdateCheckError::new(UpdateErrorKind::Version, e.to_string()))?;

    if !latest.pre.is_empty() {
        // The OSS channel is the stable update index. A prerelease manifest
        // must never reach stable users; fail the channel so `Auto` falls back
        // to GitHub instead of offering it.
        return Err(UpdateCheckError::new(
            UpdateErrorKind::InvalidResponse,
            format!("manifest version is a prerelease: {version}"),
        ));
    }

    if latest <= current {
        return Ok(None);
    }

    let asset = parsed["assets"].get(platform).ok_or_else(|| {
        UpdateCheckError::new(
            UpdateErrorKind::Package,
            format!("manifest asset missing for {platform}"),
        )
    })?;

    let name = asset["name"].as_str().unwrap_or("").trim();
    let path = asset["path"].as_str().unwrap_or("").trim();
    let sha256 = asset["sha256"].as_str().unwrap_or("").trim();
    let size = asset["size"].as_u64().unwrap_or(0);

    let expected_extension = if platform.starts_with("windows") {
        ".exe"
    } else {
        ".dmg"
    };
    if !is_safe_asset_name(name) || !name.to_ascii_lowercase().ends_with(expected_extension) {
        return Err(UpdateCheckError::new(
            UpdateErrorKind::InvalidResponse,
            format!("invalid manifest asset name for {platform}: {name}"),
        ));
    }
    if !is_safe_relative_path(path) || path != format!("v{version}/{name}") {
        return Err(UpdateCheckError::new(
            UpdateErrorKind::InvalidResponse,
            format!("unexpected manifest asset path for {platform}: {path}"),
        ));
    }
    if !is_sha256_hex(sha256) {
        return Err(UpdateCheckError::new(
            UpdateErrorKind::InvalidResponse,
            format!("invalid manifest sha256 for {platform}"),
        ));
    }

    Ok(Some(UpdateInfo {
        latest_version: latest.to_string(),
        release_notes: parsed["notes"].as_str().unwrap_or("").to_string(),
        download_url: format!("{OSS_RELEASES_BASE}/{path}"),
        checksum_url: String::new(),
        asset_name: name.to_string(),
        asset_size: size,
        sha256: Some(sha256.to_ascii_lowercase()),
        source: UpdateSource::Oss,
    }))
}

/// Manifest platform key for the current build.
fn platform_key() -> Result<&'static str, UpdateCheckError> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok("windows-x86_64"),
        ("macos", "aarch64") => Ok("macos-aarch64"),
        ("macos", "x86_64") => Ok("macos-x86_64"),
        (os, arch) => Err(UpdateCheckError::new(
            UpdateErrorKind::UnsupportedPlatform,
            format!("automatic updates are not supported on {os}/{arch}"),
        )),
    }
}

/// A manifest path must stay inside the OSS release directory.
fn is_safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains("://")
        && !path.contains('\\')
        && !path.contains('?')
        && !path.contains('#')
        && !path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
}

/// A manifest asset name must be a plain file name; it is joined onto the
/// update temp directory and must never escape it (or select another drive on
/// Windows, which is why `:` and separators are rejected outright).
pub(crate) fn is_safe_asset_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+'))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Pick the right asset for the current platform from the release JSON.
fn select_platform_asset(
    release: &serde_json::Value,
    version: &str,
) -> Result<(String, String, String, u64), UpdateCheckError> {
    let assets = release["assets"].as_array().ok_or_else(|| {
        UpdateCheckError::new(UpdateErrorKind::InvalidResponse, "missing release assets")
    })?;

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
        .ok_or_else(|| {
            UpdateCheckError::new(
                UpdateErrorKind::Package,
                format!("release asset not found: {asset_pattern}"),
            )
        })?;

    // Find the corresponding checksum file.
    let checksum_url = asset_map
        .iter()
        .find(|(name, _)| name.ends_with(&checksum_pattern))
        .map(|(_, (url, _))| url.to_string())
        .ok_or_else(|| {
            UpdateCheckError::new(
                UpdateErrorKind::Package,
                format!("release checksum not found: {checksum_pattern}"),
            )
        })?;

    Ok((
        download_url.to_string(),
        checksum_url,
        main_name.to_string(),
        *size,
    ))
}

/// Returns (asset_name_fragment, checksum_name_fragment) for the current platform.
#[cfg(target_os = "windows")]
fn platform_asset_patterns(version: &str) -> Result<(String, String), UpdateCheckError> {
    asset_patterns_for("windows", std::env::consts::ARCH, version)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn platform_asset_patterns(version: &str) -> Result<(String, String), UpdateCheckError> {
    asset_patterns_for("macos", "aarch64", version)
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
fn platform_asset_patterns(version: &str) -> Result<(String, String), UpdateCheckError> {
    asset_patterns_for("macos", "x86_64", version)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_asset_patterns(version: &str) -> Result<(String, String), UpdateCheckError> {
    asset_patterns_for(std::env::consts::OS, std::env::consts::ARCH, version)
}

fn asset_patterns_for(
    os: &str,
    arch: &str,
    version: &str,
) -> Result<(String, String), UpdateCheckError> {
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
        _ => Err(UpdateCheckError::new(
            UpdateErrorKind::UnsupportedPlatform,
            format!("automatic updates are not supported on {os}/{arch}"),
        )),
    }
}

/// Open the releases page in the system browser.
pub fn open_releases_page(url: &str) {
    #[cfg(target_os = "windows")]
    {
        let url_utf16: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: All string arguments are NUL-terminated UTF-16. `ShellExecuteW`
        // is called with `SW_SHOW` to open the URL in the default browser —
        // a read-only operation with respect to our process.
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
            .detail()
            .contains("checksum"));
    }

    #[test]
    fn scheduled_update_check_uses_daily_interval() {
        let now = DateTime::parse_from_rfc3339("2026-07-20T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(scheduled_update_check_due("", now));
        assert!(scheduled_update_check_due("not a date", now));
        assert!(!scheduled_update_check_due("2026-07-19T12:30:00Z", now));
        assert!(scheduled_update_check_due("2026-07-19T11:59:59Z", now));
    }

    #[test]
    fn user_facing_update_errors_are_summarized() {
        let raw = "Download failed: very long transport error containing https://api.github.com/repos/Ruszero01/clippi/releases/latest and many details";
        let message = summarize_update_error(raw);
        assert!(!message.contains("api.github.com"));
        assert!(!message.contains("transport error"));
        assert!(message.chars().count() <= 32);
    }

    #[test]
    fn check_error_keeps_detail_for_logs_only() {
        let err = UpdateCheckError::new(
            UpdateErrorKind::Network,
            "dns failure while resolving api.github.com",
        );
        assert!(err.is_network_failure());
        assert!(err.detail().contains("api.github.com"));
        assert!(!err.user_message().contains("api.github.com"));
    }

    fn sample_update_info(source: UpdateSource) -> UpdateInfo {
        UpdateInfo {
            latest_version: "9.9.9".to_string(),
            release_notes: String::new(),
            download_url: "https://example.invalid/installer.exe".to_string(),
            checksum_url: String::new(),
            asset_name: "installer.exe".to_string(),
            asset_size: 42,
            sha256: Some("a".repeat(64)),
            source,
        }
    }

    fn valid_oss_manifest() -> serde_json::Value {
        json!({
            "schema": 1,
            "version": "1.2.4",
            "tag": "v1.2.4",
            "published_at": "2026-09-09T06:00:00Z",
            "notes": "### 新增\n- test",
            "assets": {
                "windows-x86_64": {
                    "name": "Clippi_Setup.exe",
                    "path": "v1.2.4/Clippi_Setup.exe",
                    "size": 8024658,
                    "sha256": "a".repeat(64)
                },
                "macos-aarch64": {
                    "name": "Clippi_aarch64.dmg",
                    "path": "v1.2.4/Clippi_aarch64.dmg",
                    "size": 123,
                    "sha256": "b".repeat(64)
                },
                "macos-x86_64": {
                    "name": "Clippi_x64.dmg",
                    "path": "v1.2.4/Clippi_x64.dmg",
                    "size": 456,
                    "sha256": "c".repeat(64)
                }
            }
        })
    }

    #[test]
    fn oss_manifest_parses_windows_asset() {
        let manifest = valid_oss_manifest().to_string();
        let info = parse_oss_manifest_for_platform(&manifest, "1.2.3", "windows-x86_64")
            .unwrap()
            .expect("update available");

        assert_eq!(info.latest_version, "1.2.4");
        assert_eq!(info.asset_name, "Clippi_Setup.exe");
        assert_eq!(
            info.download_url,
            format!("{OSS_RELEASES_BASE}/v1.2.4/Clippi_Setup.exe")
        );
        assert_eq!(info.asset_size, 8024658);
        let expected_sha = "a".repeat(64);
        assert_eq!(info.sha256.as_deref(), Some(expected_sha.as_str()));
        assert_eq!(info.source, UpdateSource::Oss);
        assert!(info.release_notes.contains("新增"));
    }

    #[test]
    fn oss_manifest_parses_macos_assets() {
        let manifest = valid_oss_manifest().to_string();

        let arm = parse_oss_manifest_for_platform(&manifest, "1.2.3", "macos-aarch64")
            .unwrap()
            .expect("update available");
        assert_eq!(arm.asset_name, "Clippi_aarch64.dmg");
        assert_eq!(
            arm.download_url,
            format!("{OSS_RELEASES_BASE}/v1.2.4/Clippi_aarch64.dmg")
        );

        let intel = parse_oss_manifest_for_platform(&manifest, "1.2.3", "macos-x86_64")
            .unwrap()
            .expect("update available");
        assert_eq!(intel.asset_name, "Clippi_x64.dmg");
    }

    #[test]
    fn oss_manifest_ignores_same_or_older_version() {
        let manifest = valid_oss_manifest().to_string();
        assert!(
            parse_oss_manifest_for_platform(&manifest, "1.2.4", "windows-x86_64")
                .unwrap()
                .is_none()
        );
        assert!(
            parse_oss_manifest_for_platform(&manifest, "1.2.5", "windows-x86_64")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn oss_manifest_rejects_prerelease_versions() {
        let mut manifest = valid_oss_manifest();
        manifest["version"] = json!("1.2.4-beta.1");
        manifest["assets"]["windows-x86_64"]["path"] = json!("v1.2.4-beta.1/Clippi_Setup.exe");
        let error =
            parse_oss_manifest_for_platform(&manifest.to_string(), "1.2.3", "windows-x86_64")
                .unwrap_err();
        assert_eq!(error.kind(), UpdateErrorKind::InvalidResponse);
        assert!(error.detail().contains("prerelease"));
    }

    #[test]
    fn oss_manifest_rejects_unknown_schema() {
        let mut manifest = valid_oss_manifest();
        manifest["schema"] = json!(2);
        let error =
            parse_oss_manifest_for_platform(&manifest.to_string(), "1.2.3", "windows-x86_64")
                .unwrap_err();
        assert_eq!(error.kind(), UpdateErrorKind::InvalidResponse);
    }

    #[test]
    fn oss_manifest_rejects_invalid_sha256() {
        let mut manifest = valid_oss_manifest();
        manifest["assets"]["windows-x86_64"]["sha256"] = json!("deadbeef");
        let error =
            parse_oss_manifest_for_platform(&manifest.to_string(), "1.2.3", "windows-x86_64")
                .unwrap_err();
        assert_eq!(error.kind(), UpdateErrorKind::InvalidResponse);
        assert!(error.detail().contains("sha256"));
    }

    #[test]
    fn oss_manifest_rejects_unsafe_asset_path() {
        let mut manifest = valid_oss_manifest();
        manifest["assets"]["windows-x86_64"]["path"] = json!("../evil/Clippi_Setup.exe");
        let error =
            parse_oss_manifest_for_platform(&manifest.to_string(), "1.2.3", "windows-x86_64")
                .unwrap_err();
        assert_eq!(error.kind(), UpdateErrorKind::InvalidResponse);

        manifest = valid_oss_manifest();
        manifest["assets"]["windows-x86_64"]["path"] =
            json!("https://evil.example/Clippi_Setup.exe");
        let error =
            parse_oss_manifest_for_platform(&manifest.to_string(), "1.2.3", "windows-x86_64")
                .unwrap_err();
        assert_eq!(error.kind(), UpdateErrorKind::InvalidResponse);
    }

    #[test]
    fn oss_manifest_rejects_unsafe_asset_name() {
        let mut manifest = valid_oss_manifest();
        manifest["assets"]["windows-x86_64"]["name"] = json!("..\\evil.exe");
        let error =
            parse_oss_manifest_for_platform(&manifest.to_string(), "1.2.3", "windows-x86_64")
                .unwrap_err();
        assert_eq!(error.kind(), UpdateErrorKind::InvalidResponse);

        manifest = valid_oss_manifest();
        manifest["assets"]["windows-x86_64"]["name"] = json!("Clippi.dmg");
        let error =
            parse_oss_manifest_for_platform(&manifest.to_string(), "1.2.3", "windows-x86_64")
                .unwrap_err();
        assert_eq!(error.kind(), UpdateErrorKind::InvalidResponse);
    }

    #[test]
    fn oss_manifest_requires_current_platform_asset() {
        let mut manifest = valid_oss_manifest();
        manifest["assets"]
            .as_object_mut()
            .unwrap()
            .remove("windows-x86_64");
        let error =
            parse_oss_manifest_for_platform(&manifest.to_string(), "1.2.3", "windows-x86_64")
                .unwrap_err();
        assert_eq!(error.kind(), UpdateErrorKind::Package);
    }

    #[test]
    fn publisher_manifest_example_parses_for_every_platform() {
        // Contract test: `scripts/latest.example.json` is produced by
        // `scripts/publish_oss.py --dry-run`, so this keeps the publisher and
        // this parser in lockstep.
        let manifest = include_str!("../../scripts/latest.example.json");
        for platform in ["windows-x86_64", "macos-aarch64", "macos-x86_64"] {
            let info = parse_oss_manifest_for_platform(manifest, "0.0.1", platform)
                .unwrap_or_else(|error| panic!("{platform}: {error:?}"))
                .expect("update available");
            assert_eq!(info.latest_version, "0.4.7");
            assert_eq!(info.source, UpdateSource::Oss);
            assert!(info.sha256.is_some());
            assert!(info.download_url.starts_with(OSS_RELEASES_BASE));
        }
    }

    #[test]
    fn unsafe_asset_names_are_rejected() {
        assert!(is_safe_asset_name("Clippi_Setup.exe"));
        assert!(is_safe_asset_name("Clippi_x64.dmg"));
        assert!(!is_safe_asset_name(""));
        assert!(!is_safe_asset_name("."));
        assert!(!is_safe_asset_name(".."));
        // Windows drive-relative names would replace the temp directory.
        assert!(!is_safe_asset_name("C:evil.exe"));
        assert!(!is_safe_asset_name("a/b.exe"));
        assert!(!is_safe_asset_name("a\\b.exe"));
        assert!(!is_safe_asset_name("a b.exe"));
        assert!(!is_safe_asset_name(&"a".repeat(129)));
    }

    #[test]
    fn update_channel_from_setting_handles_known_and_unknown_values() {
        assert_eq!(UpdateChannel::from_setting("auto"), UpdateChannel::Auto);
        assert_eq!(UpdateChannel::from_setting("oss"), UpdateChannel::Oss);
        assert_eq!(UpdateChannel::from_setting("github"), UpdateChannel::GitHub);
        assert_eq!(UpdateChannel::from_setting(""), UpdateChannel::Auto);
        assert_eq!(
            UpdateChannel::from_setting("something-else"),
            UpdateChannel::Auto
        );
    }

    #[test]
    fn auto_channel_does_not_query_github_when_oss_succeeds() {
        let called = std::cell::Cell::new(false);
        let result = with_auto_fallback(Ok(Some(sample_update_info(UpdateSource::Oss))), || {
            called.set(true);
            Ok(None)
        });
        assert_eq!(result.unwrap().unwrap().source, UpdateSource::Oss);
        assert!(!called.get());
    }

    #[test]
    fn auto_channel_falls_back_to_github_when_oss_unavailable() {
        let result = with_auto_fallback(
            Err(UpdateCheckError::new(
                UpdateErrorKind::ChannelUnavailable,
                "HTTP 403",
            )),
            || Ok(Some(sample_update_info(UpdateSource::GitHub))),
        );
        assert_eq!(result.unwrap().unwrap().source, UpdateSource::GitHub);
    }

    #[test]
    fn auto_channel_prefers_primary_oss_error_over_github_error() {
        let error = with_auto_fallback(
            Err(UpdateCheckError::new(
                UpdateErrorKind::InvalidResponse,
                "bad manifest",
            )),
            || {
                Err(UpdateCheckError::new(
                    UpdateErrorKind::Network,
                    "dns failure",
                ))
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), UpdateErrorKind::InvalidResponse);
    }

    #[test]
    fn auto_channel_reports_network_failure_when_both_channels_fail() {
        let error = with_auto_fallback(
            Err(UpdateCheckError::new(
                UpdateErrorKind::ChannelUnavailable,
                "HTTP 403",
            )),
            || {
                Err(UpdateCheckError::new(
                    UpdateErrorKind::Network,
                    "dns failure",
                ))
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), UpdateErrorKind::Network);
        assert!(error.detail().contains("OSS:"));
        assert!(error.detail().contains("GitHub:"));
    }
}
