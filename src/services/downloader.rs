//! Download artifacts from GitHub Releases with progress and SHA256 verification.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Progress of an ongoing download — shared between background thread and UI poll.
pub struct DownloadProgress {
    pub downloaded: AtomicU64,
    pub total: u64,
}

impl DownloadProgress {
    pub fn new(total: u64) -> Self {
        Self {
            downloaded: AtomicU64::new(0),
            total,
        }
    }

    pub fn percentage(&self) -> u8 {
        let d = self.downloaded.load(Ordering::Relaxed);
        if self.total == 0 {
            return 0;
        }
        ((d * 100) / self.total).min(100) as u8
    }
}

/// Download a file from `url` to `dest_path`, writing progress to `progress`.
/// Blocking — call from a background thread.
pub fn download_file(
    url: &str,
    dest_path: &Path,
    progress: &DownloadProgress,
) -> Result<(), String> {
    let response = ureq::get(url)
        .set(
            "User-Agent",
            &format!("Clippi/{}", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;

    // Reset downloaded counter
    progress.downloaded.store(0, Ordering::Relaxed);

    let mut reader = response.into_reader();
    let mut file =
        std::fs::File::create(dest_path).map_err(|e| format!("Cannot create file: {e}"))?;

    let mut buf = [0u8; 8192];
    let mut downloaded = 0u64;
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
        progress.downloaded.store(downloaded, Ordering::Relaxed);
    }
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
    let response = ureq::get(sha256_url)
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
    Ok(body.split_whitespace().next().unwrap_or("").to_string())
}
