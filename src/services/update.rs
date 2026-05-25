//! Update checker — queries GitHub Releases API, compares versions via semver,
//! caches results, and opens the releases page in the browser.

use std::sync::Mutex;
use std::time::Instant;

/// Info about the latest available release, if any.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub latest_version: String,
    pub html_url: String,
}

/// Thread-safe update cache with cooldown.
pub struct UpdateChecker {
    current_version: String,
    repo_owner: String,
    repo_name: String,
    cache: Mutex<Option<CachedResult>>,
    cache_ttl_secs: u64,
}

struct CachedResult {
    info: Option<UpdateInfo>, // None = no update available
    checked_at: Instant,
}

impl UpdateChecker {
    pub fn new(current_version: &str, repo_owner: &str, repo_name: &str) -> Self {
        Self {
            current_version: current_version.to_string(),
            repo_owner: repo_owner.to_string(),
            repo_name: repo_name.to_string(),
            cache: Mutex::new(None),
            cache_ttl_secs: 3600, // 1 hour
        }
    }

    /// Returns the current app version string.
    pub fn current_version(&self) -> &str {
        &self.current_version
    }

    /// Returns cached update info if still fresh, otherwise performs a live check.
    /// `None` means no update available. `Some(UpdateInfo)` means a newer version exists.
    pub fn check(&self) -> Option<UpdateInfo> {
        // Return cached result if still fresh
        if let Ok(cache) = self.cache.lock() {
            if let Some(ref cached) = *cache {
                if cached.checked_at.elapsed().as_secs() < self.cache_ttl_secs {
                    return cached.info.clone();
                }
            }
        }

        // Perform live check
        let result = self.fetch_latest_release();
        if let Ok(mut cache) = self.cache.lock() {
            *cache = Some(CachedResult {
                info: result.clone(),
                checked_at: Instant::now(),
            });
        }
        result
    }

    /// Force a fresh check, ignoring cache.
    #[allow(dead_code)]
    pub fn check_now(&self) -> Option<UpdateInfo> {
        let result = self.fetch_latest_release();
        if let Ok(mut cache) = self.cache.lock() {
            *cache = Some(CachedResult {
                info: result.clone(),
                checked_at: Instant::now(),
            });
        }
        result
    }

    /// Build the releases page URL.
    #[allow(dead_code)]
    pub fn releases_url(&self) -> String {
        format!(
            "https://github.com/{}/{}/releases",
            self.repo_owner, self.repo_name
        )
    }

    fn fetch_latest_release(&self) -> Option<UpdateInfo> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/releases/latest",
            self.repo_owner, self.repo_name
        );

        let agent = format!("Clippi/{}", self.current_version);

        let response = ureq::get(&url)
            .set("User-Agent", &agent)
            .set("Accept", "application/vnd.github.v3+json")
            .call()
            .ok()?;

        let body = response.into_string().ok()?;
        let parsed: serde_json::Value = serde_json::from_str(&body).ok()?;

        let tag_name = parsed["tag_name"].as_str()?;
        let html_url = parsed["html_url"].as_str().unwrap_or("");

        // Strip leading 'v' if present
        let latest_ver = tag_name.strip_prefix('v').unwrap_or(tag_name);

        let current = semver::Version::parse(&self.current_version).ok()?;
        let latest = semver::Version::parse(latest_ver).ok()?;

        if latest > current {
            Some(UpdateInfo {
                latest_version: latest.to_string(),
                html_url: html_url.to_string(),
            })
        } else {
            None // No update available
        }
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
