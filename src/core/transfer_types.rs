//! Transfer station types — independent of sync protocol.
//!
//! The file manifest (`clippi_files.json`) is a shared state file on the
//! backend, decoupled from the sync payload (`clippi_sync.json`). It carries
//! file metadata and blob references for cross-device file transfer.

use serde::{Deserialize, Serialize};

/// A manifest together with the backend revision used for optimistic locking.
#[derive(Debug, Clone)]
pub struct ManifestSnapshot {
    pub manifest: FileManifest,
    /// None means the manifest does not exist yet.
    pub revision: Option<String>,
}

/// A conditional manifest write can fail because another device won the race.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestWriteError {
    Conflict,
    Other(String),
}

/// Top-level file manifest stored as `clippi_files.json` on the backend.
///
/// This is the shared state that all devices read and write. Unlike the sync
/// payload, there is no merge logic — the entire manifest is read-modify-written
/// with atomic file replacement to prevent corruption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileManifest {
    pub version: u32,
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub updated_at: String, // RFC3339
    #[serde(default)]
    pub files: Vec<ManifestEntry>,
}

/// A single file entry in the transfer station manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Content hash of the file (hex string), used as the blob filename stem.
    pub hash: String,
    /// Original filename (e.g. "report.pdf").
    pub name: String,
    /// File extension without the dot (e.g. "pdf").
    pub ext: String,
    /// File size in bytes.
    pub size: u64,
    /// Upload timestamp (RFC3339).
    pub uploaded_at: String,
    /// Expiration timestamp (RFC3339). After this time the entry and its blob
    /// are eligible for automatic cleanup.
    pub expires_at: String,
    /// Name of the device that uploaded this file.
    #[serde(default)]
    pub uploaded_by: String,
}

/// A manifest entry resolved against the local database.
///
/// Used for rendering in the transfer station view. `is_local` is determined
/// by checking whether a DB record exists with a matching `remote_hash`.
#[derive(Debug, Clone)]
pub struct ResolvedEntry {
    pub entry: ManifestEntry,
    /// `true` if this file has a corresponding DB record (already downloaded
    /// or uploaded by this device), `false` if it only exists in the cloud.
    pub is_local: bool,
    /// Local-only absolute path. Never serialized into the shared manifest.
    pub local_path: Option<String>,
}

impl ManifestEntry {
    /// Validate all remotely supplied fields before they are used in a URL or path.
    pub fn validate(&self) -> Result<(), String> {
        if self.hash.len() != 64 || !self.hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid SHA-256 hash".into());
        }
        if self.name.is_empty()
            || self.name == "."
            || self.name == ".."
            || self.name.chars().any(|character| {
                character.is_control()
                    || matches!(
                        character,
                        '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                    )
            })
            || self.name.ends_with([' ', '.'])
        {
            return Err("invalid file name".into());
        }
        if self.ext.contains(['/', '\\', '\0']) {
            return Err("invalid file extension".into());
        }
        let stem = self
            .name
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if matches!(
            stem.as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        ) {
            return Err("reserved file name".into());
        }
        Ok(())
    }
}
