//! Download artifacts from GitHub Releases with progress and SHA256 verification.

use std::io::{Read, Write};
use std::path::Path;
/// Download a file from `url` to `dest_path`, reporting percentage changes.
/// Blocking — call from a background thread.
pub fn download_file(
    url: &str,
    dest_path: &Path,
    expected_size: u64,
    mut on_progress: impl FnMut(u8),
) -> Result<(), String> {
    let http = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(300))
        .build();
    let response = http
        .get(url)
        .set(
            "User-Agent",
            &format!("Clippi/{}", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;

    let total = response
        .header("Content-Length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(expected_size);

    let mut reader = response.into_reader();
    let mut file =
        std::fs::File::create(dest_path).map_err(|e| format!("Cannot create file: {e}"))?;

    let mut buf = [0u8; 8192];
    let mut downloaded = 0u64;
    let mut last_percentage = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("Read error: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("Write error: {e}"))?;
        downloaded += n as u64;
        if let Some(percentage) = downloaded.saturating_mul(100).checked_div(total) {
            let percentage = percentage.min(100) as u8;
            if percentage != last_percentage {
                last_percentage = percentage;
                on_progress(percentage);
            }
        }
    }
    file.sync_all()
        .map_err(|e| format!("Cannot flush downloaded file: {e}"))?;
    if total > 0 && downloaded != total {
        return Err(format!(
            "Incomplete download: expected {total} bytes, received {downloaded}"
        ));
    }
    on_progress(100);
    Ok(())
}

/// Verify SHA256 of `file_path` against expected hash hex string.
pub fn verify_sha256(file_path: &Path, expected_hex: &str) -> Result<(), String> {
    use sha2::Digest;

    let mut file =
        std::fs::File::open(file_path).map_err(|e| format!("Cannot open for verify: {e}"))?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("Read error during verify: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let hash = format!("{:x}", hasher.finalize());
    let expected = expected_hex.trim();
    if hash.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "Checksum mismatch.\nExpected: {}\nGot: {}",
            expected, hash
        ))
    }
}

/// Fetch the checksum file from `sha256_url` and return the hash string.
/// The file format is: `<hash>  <filename>` or just `<hash>`.
pub fn fetch_checksum(sha256_url: &str) -> Result<String, String> {
    if sha256_url.is_empty() {
        return Err("Release checksum URL is missing".into());
    }
    let http = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(30))
        .build();
    let response = http
        .get(sha256_url)
        .set(
            "User-Agent",
            &format!("Clippi/{}", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|e| format!("Cannot fetch checksum: {e}"))?;
    let body = response
        .into_string()
        .map_err(|e| format!("Cannot read checksum: {e}"))?;
    // Take the first whitespace-delimited token (the hash)
    let hash = body.split_whitespace().next().unwrap_or("");
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Release checksum is not a valid SHA256 hash".into());
    }
    Ok(hash.to_string())
}
