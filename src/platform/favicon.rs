//! Favicon fetching and disk caching for URL domains via Google's favicon service.
//! Non-critical: all failures are silent and callers fall back to chain icon.

use std::path::PathBuf;

use crate::core::paths::images_dir;

/// Sanitize a domain string for use as a filename (remove port and invalid chars).
fn sanitize_domain(domain: &str) -> String {
    let without_port = domain.split(':').next().unwrap_or(domain);
    without_port
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect::<String>()
}

fn cache_path(domain: &str) -> PathBuf {
    images_dir().join("icons").join(format!("favicon_{}.png", sanitize_domain(domain)))
}

/// Get the expected cache path for a domain (does not guarantee file exists).
pub fn favicon_cache_path(domain: &str) -> String {
    cache_path(domain).to_string_lossy().to_string()
}

/// Try to fetch a favicon from Google's service and cache it to disk.
/// Returns the disk path if already cached or successfully fetched.
/// Returns `None` on any failure (network, non-200, disk write).
pub fn ensure_favicon_cached(domain: &str) -> Option<String> {
    let path = cache_path(domain);

    // Already cached
    if path.exists() {
        return Some(path.to_string_lossy().to_string());
    }

    // Ensure icons directory exists
    let _ = std::fs::create_dir_all(path.parent()?);

    let url = format!(
        "https://www.google.com/s2/favicons?domain={}&sz=32",
        domain
    );

    match ureq::get(&url).call() {
        Ok(response) => {
            let mut body = Vec::new();
            if response.into_reader().read_to_end(&mut body).is_err() {
                return None;
            }
            // Quick PNG validation: check magic bytes
            if body.len() < 8 || &body[..4] != b"\x89PNG" {
                return None;
            }
            let _ = std::fs::write(&path, &body);
            Some(path.to_string_lossy().to_string())
        }
        Err(_) => None,
    }
}
