//! Transfer station types — independent of sync protocol.
//!
//! The file manifest (`clippi_files.json`) is a shared state file on the
//! backend, decoupled from the sync payload (`clippi_sync.json`). It carries
//! file metadata and blob references for cross-device file transfer.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// Hard safety limit for a single transfer. The current transports buffer the
/// payload in memory, so accepting an unbounded remote size can exhaust the
/// process before integrity verification runs.
pub const MAX_TRANSFER_FILE_SIZE_BYTES: u64 = 512 * 1024 * 1024;
pub const TRANSFER_STATUS_LOCAL_UID: &str = "clippi:transfer:local";
pub const TRANSFER_STATUS_CLOUD_UID: &str = "clippi:transfer:cloud";
pub const TRANSFER_STATUS_DOWNLOADING_UID: &str = "clippi:transfer:downloading";

/// Virtual tag UID marking a transfer entry as pinned by the user.
pub const TRANSFER_STATUS_PINNED_UID: &str = "clippi:transfer:pinned";
/// Virtual tag UID carrying the effective expiration timestamp of a transfer
/// entry (`TagInfo.updated_at`), used by the card's remaining-time pill.
pub const TRANSFER_STATUS_RETENTION_UID: &str = "clippi:transfer:retention";

/// Shared transfer-station blue used for pinned state, cloud/downloading
/// status tags and the pin toolbar accent.
pub const TRANSFER_BLUE: &str = "#3B82F6";

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

/// Materialized transfer-station state. WebDAV stores it as
/// `clippi_files.json`; local-folder backends rebuild it from unique operation
/// files so cloud-drive replicas can merge concurrent mutations safely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Content hash of the file (hex string), used as the blob filename stem.
    pub hash: String,
    /// Immutable storage key for this upload generation. Version-1 entries
    /// omit it and continue to address their blob by content hash.
    #[serde(default)]
    pub blob_id: String,
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
    /// Whether the user pinned this entry. Pinned entries are never removed
    /// by automatic expiration cleanup. Protocol v2 entries omit it and
    /// deserialize as `false`.
    #[serde(default)]
    pub pinned: bool,
}

/// A manifest entry resolved against the local database.
///
/// Used for rendering in the transfer station view. `is_local` is determined
/// by checking whether a DB record exists with a matching `remote_hash`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEntry {
    pub entry: ManifestEntry,
    /// `true` if this device has a usable local copy (a DB record whose path
    /// still exists), `false` if the entry is cloud-only.
    pub is_local: bool,
    /// Local-only absolute path. Never serialized into the shared manifest.
    pub local_path: Option<String>,
}

pub fn validate_portable_file_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 255
        || name.encode_utf16().count() > 255
        || name == "."
        || name == ".."
        || name.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        })
        || name.ends_with([' ', '.'])
    {
        return Err("invalid file name".into());
    }
    let stem = name
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

impl ManifestEntry {
    /// Validate all remotely supplied fields before they are used in a URL or path.
    pub fn validate(&self) -> Result<(), String> {
        if self.hash.len() != 64
            || !self
                .hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("invalid SHA-256 hash".into());
        }
        if !self.blob_id.is_empty() {
            let expected_prefix = format!("{}-", self.hash);
            let Some(uuid) = self.blob_id.strip_prefix(&expected_prefix) else {
                return Err("invalid transfer blob id".into());
            };
            if uuid::Uuid::parse_str(uuid).is_err() {
                return Err("invalid transfer blob id".into());
            }
        }
        validate_portable_file_name(&self.name)?;
        if self.ext.len() > 32
            || self.ext.chars().any(|character| {
                character.is_control()
                    || matches!(
                        character,
                        '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                    )
            })
        {
            return Err("invalid file extension".into());
        }
        if self.size > MAX_TRANSFER_FILE_SIZE_BYTES {
            return Err(format!(
                "file exceeds the {} MiB transfer limit",
                MAX_TRANSFER_FILE_SIZE_BYTES / 1024 / 1024
            ));
        }
        validate_utc_timestamp(&self.uploaded_at, false, "upload timestamp")?;
        validate_utc_timestamp(&self.expires_at, true, "expiration timestamp")?;
        Ok(())
    }

    pub fn blob_key(&self) -> &str {
        if self.blob_id.is_empty() {
            &self.hash
        } else {
            &self.blob_id
        }
    }
}

/// Compute the effective expiration instant for a manifest entry.
///
/// Shared by automatic cleanup and the UI remaining-time projection so they
/// never diverge on the retention rules:
/// - global retention off (`retention_days == 0`) means keep forever: any
///   explicit `expires_at` from earlier settings is ignored so the UI
///   projection and the (disabled) cleanup scheduling can never disagree;
/// - an empty `expires_at` also means keep forever: entries uploaded under
///   "permanent" retention (or pinned) never gain an expiration merely
///   because the global retention setting later changed;
/// - otherwise the parseable `expires_at` wins.
///
/// `pinned` does not change the returned value; callers decide whether the
/// entry is exempt from cleanup, so unpinning can still recompute a fresh
/// retention window from this function's rules.
pub fn effective_expiration(entry: &ManifestEntry, retention_days: u32) -> Option<DateTime<Utc>> {
    if retention_days == 0 {
        return None;
    }
    // An empty `expires_at` means the entry was uploaded under "keep forever"
    // retention (or was pinned). It must stay permanent regardless of the
    // current global retention setting; falling back to
    // `uploaded_at + retention_days` would retroactively expire it.
    if entry.expires_at.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(&entry.expires_at)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

/// Return the end of a retention window without panicking on an invalid local
/// setting or a timestamp too close to Chrono's upper bound.
pub fn retention_expiration(start: DateTime<Utc>, retention_days: u32) -> Option<DateTime<Utc>> {
    if retention_days == 0 {
        return None;
    }
    start.checked_add_signed(Duration::days(i64::from(retention_days)))
}

fn validate_utc_timestamp(value: &str, allow_empty: bool, label: &str) -> Result<(), String> {
    if allow_empty && value.is_empty() {
        return Ok(());
    }
    let parsed =
        chrono::DateTime::parse_from_rfc3339(value).map_err(|_| format!("invalid {label}"))?;
    if parsed.offset().local_minus_utc() != 0 {
        return Err(format!("{label} must use UTC"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(now: chrono::DateTime<Utc>) -> ManifestEntry {
        ManifestEntry {
            hash: "a".repeat(64),
            blob_id: String::new(),
            name: "file.bin".into(),
            ext: "bin".into(),
            size: 1,
            uploaded_at: now.to_rfc3339(),
            expires_at: String::new(),
            uploaded_by: String::new(),
            pinned: false,
        }
    }

    #[test]
    fn effective_expiration_prefers_explicit_timestamp() {
        let now = Utc::now();
        let mut e = entry(now);
        e.expires_at = (now + Duration::days(7)).to_rfc3339();
        let expires = effective_expiration(&e, 1).unwrap();
        assert_eq!(expires, now + Duration::days(7));
    }

    #[test]
    fn effective_expiration_treats_empty_expires_as_permanent() {
        let now = Utc::now();
        // A "keep forever" upload leaves expires_at empty. A later global
        // retention change must not retroactively expire it.
        let e = entry(now - Duration::days(30));
        assert_eq!(effective_expiration(&e, 3), None);
        assert_eq!(effective_expiration(&e, 30), None);
    }

    #[test]
    fn effective_expiration_returns_none_when_retention_is_disabled() {
        let now = Utc::now();
        let e = entry(now);
        assert_eq!(effective_expiration(&e, 0), None);
        // Global "keep forever" ignores stale explicit expirations from
        // earlier retention settings so UI and cleanup stay consistent.
        let mut e = entry(now);
        e.expires_at = (now + Duration::days(2)).to_rfc3339();
        assert_eq!(effective_expiration(&e, 0), None);
    }

    #[test]
    fn retention_expiration_does_not_panic_on_oversized_days() {
        let start = Utc::now();
        assert_eq!(retention_expiration(start, u32::MAX), None);
        assert_eq!(retention_expiration(start, 0), None);
        assert_eq!(
            retention_expiration(start, 3),
            Some(start + Duration::days(3))
        );
    }

    #[test]
    fn pinned_does_not_change_effective_expiration() {
        let now = Utc::now();
        // Pinned entries with an empty expires_at are permanent.
        let mut e = entry(now);
        e.pinned = true;
        assert_eq!(effective_expiration(&e, 3), None);

        // An explicit expires_at still wins even when pinned.
        let mut e = entry(now);
        e.pinned = true;
        e.expires_at = (now + Duration::days(3)).to_rfc3339();
        assert_eq!(effective_expiration(&e, 3), Some(now + Duration::days(3)));
    }
}
