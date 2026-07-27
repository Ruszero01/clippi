//! Favicon fetching and disk caching for URL domains via Google's favicon service.
//! Non-critical: all failures are silent and callers fall back to chain icon.

use std::path::PathBuf;
use std::time::Duration;

use crate::core::paths::images_dir;

/// Sanitize a domain string for use as a filename (remove port and invalid chars).
fn sanitize_domain(domain: &str) -> String {
    let without_port = domain.split(':').next().unwrap_or(domain);
    without_port
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
}

fn cache_path(domain_or_url: &str) -> Option<PathBuf> {
    let host = crate::core::secret::url_clean_host(domain_or_url)?;
    Some(
        images_dir()
            .join("icons")
            .join(format!("favicon_{}.png", sanitize_domain(&host))),
    )
}

/// Check if a favicon is cached locally for the given domain.
/// Returns the file path if it exists — no network request.
pub fn favicon_cache_path(domain: &str) -> Option<String> {
    let path = cache_path(domain)?;
    if path.exists() {
        Some(path.to_string_lossy().to_string())
    } else {
        None
    }
}

/// Try to fetch a favicon from Google's service and cache it to disk.
/// Returns the disk path if already cached or successfully fetched.
/// Returns `None` on any failure (network, non-200, disk write).
pub fn ensure_favicon_cached(domain: &str) -> Option<String> {
    let host = crate::core::secret::url_clean_host(domain)?;
    let path = cache_path(&host)?;

    // --- Already cached ---
    if path.exists() {
        return Some(path.to_string_lossy().to_string());
    }

    // --- Ensure icons directory exists ---
    let _ = std::fs::create_dir_all(path.parent()?);

    let mut service_url = url::Url::parse("https://www.google.com/s2/favicons").ok()?;
    service_url
        .query_pairs_mut()
        .append_pair("domain", &host)
        .append_pair("sz", "32");

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(3))
        .timeout_read(Duration::from_secs(5))
        .build();

    match agent.get(service_url.as_str()).call() {
        Ok(response) => {
            let mut body = Vec::new();
            if response.into_reader().read_to_end(&mut body).is_err() {
                return None;
            }
            // --- Quick PNG validation: check magic bytes ---
            if body.len() < 8 || &body[..4] != b"\x89PNG" {
                return None;
            }
            let _ = std::fs::write(&path, &body);
            Some(path.to_string_lossy().to_string())
        }
        Err(_) => None,
    }
}
