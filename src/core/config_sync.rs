//! Config sync — manual upload/download of portable settings snapshots.
//!
//! This module is independent of the clipboard sync protocol. It produces and
//! consumes `clippi_settings.json` files stored alongside `clippi_sync.json`
//! on each sync backend. Only a whitelist of cross-device-safe settings is
//! included; platform-specific keys, paths, credentials, and runtime state are
//! never serialised into the snapshot.
//!
//! # Versioning
//!
//! `schema_version` is independent of the clipboard sync protocol version.
//! - Unknown JSON fields are silently ignored so older Clippi versions can
//!   read newer snapshots with non-breaking additions.
//! - Original v1 fields are required: a snapshot missing any of them is rejected
//!   wholesale instead of silently resetting local settings to defaults.
//! - Additive v1 fields are optional; absence preserves the receiving device's
//!   value so snapshots from older builds remain safe to apply.
//! - A schema version other than the exactly supported one
//!   (`CURRENT_SCHEMA_VERSION`) is rejected with
//!   `ConfigSyncError::UnsupportedVersion`; no downgrade/guess is attempted.
//! - Breaking structure changes must bump the major version number and provide
//!   an explicit `vN → v(N+1)` migration function.

use serde::{Deserialize, Serialize};

use crate::core::filters::BUILTIN_TYPE_KEYS;
use crate::core::settings::{AppSettings, TypeFilterEntry};

/// File name stored in each backend's root directory.
pub const CONFIG_SYNC_FILENAME: &str = "clippi_settings.json";

/// Maximum snapshot size in bytes (1 MiB).
pub const MAX_CONFIG_SNAPSHOT_BYTES: u64 = 1_048_576;

/// Current schema version produced by this build.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Known built-in copy sound file names — must match [`crate::services::copy_sound::SOUND_LIST`].
/// Embedded in the binary, so every Clippi build carries the same set.
const VALID_COPY_SOUND_FILES: &[&str] = &[
    "copy_penclick.wav",
    "copy_kacha.wav",
    "copy_clack.wav",
    "copy_mechkb.wav",
    "copy_blip.wav",
    "copy_bubble.wav",
];

/// Upper bound for `max_items` accepted from a cloud snapshot (0 = unlimited).
/// Loose enough for any real history, tight enough to reject absurd values
/// that would pin an unbounded amount of data on the receiving device.
const MAX_ITEMS_LIMIT: u32 = 100_000;

/// Upper bound for `retention_days` accepted from a cloud snapshot (0 = keep
/// forever). 10 years exceeds any plausible retention setting; larger values
/// would silently delete every non-favorite item on the receiving device.
const MAX_RETENTION_DAYS: u32 = 3_650;

/// Accepted bounds for the global `sync_interval_secs` (1 second … 1 day).
const MIN_SYNC_INTERVAL_SECS: u64 = 1;
const MAX_SYNC_INTERVAL_SECS: u64 = 86_400;

/// `transfer_retention_days` must be one of the options offered by the
/// settings UI — anything else is rejected before it can reach the transfer
/// station, where oversized values would overflow Chrono date math.
const TRANSFER_RETENTION_OPTIONS: &[u32] = &[0, 1, 3, 7, 30];

// ── Error type ────────────────────────────────────────────────────────────

/// All errors that can occur during config sync operations.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ConfigSyncError {
    /// Remote file does not exist (not an error for download — caller decides).
    RemoteNotFound,
    /// Network / I/O transport failure.
    Transport(String),
    /// Payload exceeds `MAX_CONFIG_SNAPSHOT_BYTES`.
    TooLarge,
    /// JSON parse failure or missing required fields.
    InvalidSnapshot(String),
    /// `schema_version` is not exactly `CURRENT_SCHEMA_VERSION`.
    UnsupportedVersion(u32),
    /// `uploaded_at` is not a valid RFC3339 timestamp.
    InvalidTimestamp(String),
    /// A field value failed whitelist validation.
    InvalidFieldValue(String),
    /// Saving the merged config to disk failed.
    LocalSave(String),
    /// Restart (spawn new process) failed.
    Restart(String),
}

impl std::fmt::Display for ConfigSyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RemoteNotFound => write!(f, "remote config snapshot not found"),
            Self::Transport(msg) => write!(f, "transport error: {msg}"),
            Self::TooLarge => write!(f, "config snapshot exceeds size limit"),
            Self::InvalidSnapshot(msg) => write!(f, "invalid config snapshot: {msg}"),
            Self::UnsupportedVersion(v) => {
                write!(f, "unsupported config snapshot version {v}")
            }
            Self::InvalidTimestamp(msg) => write!(f, "invalid timestamp: {msg}"),
            Self::InvalidFieldValue(msg) => write!(f, "invalid field value: {msg}"),
            Self::LocalSave(msg) => write!(f, "failed to save config: {msg}"),
            Self::Restart(msg) => write!(f, "restart failed: {msg}"),
        }
    }
}

// ── Data types ────────────────────────────────────────────────────────────

/// A complete portable-settings snapshot stored in `clippi_settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    pub schema_version: u32,
    pub uploaded_at: String,
    pub source: ConfigSnapshotSource,
    pub settings: PortableSettingsV1,
}

/// Metadata about the device that uploaded the snapshot. Display-only;
/// never written back to `AppSettings`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSnapshotSource {
    pub app_version: String,
    pub platform: String,
    pub device_name: String,
}

/// Whitelist of settings that are safe to migrate across devices.
///
/// # Field policy
///
/// Original v1 fields are **required** on the wire. A snapshot missing any
/// original v1 field fails deserialisation and is rejected wholesale — a truncated
/// snapshot must never silently reset local settings to `false`/`0` defaults.
/// Additive fields use `Option` to distinguish absence from an explicit value:
/// older snapshots preserve local preferences rather than resetting them.
/// Unknown *additional* fields from newer builds are still ignored (serde
/// default behaviour; `deny_unknown_fields` is intentionally not enabled).
///
/// # Safety
///
/// Every field declared here has been reviewed for cross-device portability.
/// New fields added to `AppSettings` default to **not** synced unless they
/// are explicitly added to this struct and `from_local` / `apply_to`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PortableSettingsV1 {
    // ── Appearance & Interaction ──
    pub theme: String,
    pub language: String,
    pub auto_hide: bool,
    pub silent_start: bool,
    pub window_position_mode: String,
    pub card_height_mode: String,
    pub auto_focus_search: bool,
    pub clear_search_on_show: bool,
    pub always_reset_to_clipboard: bool,
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub quick_window_sync_filters: Option<bool>,
    // Outer None = absent in an older snapshot; Some(None) = system/default size.
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub font_size_scale: Option<Option<f32>>,
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub font_size_level: Option<String>,

    // ── List & Content ──
    pub sort_by_created: bool,
    pub show_source_app: bool,
    pub auto_scroll_to_top: bool,
    pub copy_as_plain_text: bool,
    pub show_original_on_hover: bool,
    pub type_filter_config: Vec<TypeFilterEntry>,
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub search_favorites_first: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub paste_click_mode: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub filter_foreign_paths: Option<bool>,

    // ── Clipboard Features ──
    pub ocr_enabled: bool,
    pub qr_enabled: bool,
    pub auto_fetch_url_title: bool,
    pub copy_sound_enabled: bool,
    pub copy_sound_file: String,
    pub image_alt_mode: String,
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub record_images: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub record_files: Option<bool>,

    // ── Data Retention ──
    pub max_items: u32,
    pub retention_days: u32,
    pub cleanup_interval: String,
    pub cleanup_stale_items: bool,

    // ── Sync Policy ──
    pub sync_interval_secs: u64,
    pub sync_auto_enabled: bool,
    pub sync_favorites_only: bool,
    pub sync_include_images: bool,
    pub sync_compress_images: bool,

    // ── Transfer Station Policy ──
    pub transfer_station_enabled: bool,
    pub transfer_retention_days: u32,

    // ── Updates ──
    pub auto_check_updates: bool,
}

/// Preserve field presence, including an explicit null for a nullable setting.
/// Unlike plain Option<T>, null is rejected when T itself is not nullable.
fn deserialize_present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

// ── Construction ──────────────────────────────────────────────────────────

impl ConfigSnapshot {
    /// Create a snapshot from the current local settings.
    ///
    /// `uploaded_at` is set to the current UTC time in RFC3339 format.
    /// The caller should pass `app_version` from `env!("CARGO_PKG_VERSION")`.
    pub fn from_local(settings: &AppSettings, app_version: &str) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            uploaded_at: chrono::Utc::now().to_rfc3339(),
            source: ConfigSnapshotSource {
                app_version: app_version.to_string(),
                platform: std::env::consts::OS.to_string(),
                device_name: hostname::get()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "unknown".to_string()),
            },
            settings: PortableSettingsV1::from_local(settings),
        }
    }

    /// Parse a snapshot from raw JSON bytes, validating everything.
    ///
    /// Returns `ConfigSyncError` on the first validation failure; partial
    /// application is never allowed.
    pub fn from_slice(data: &[u8]) -> Result<Self, ConfigSyncError> {
        // Check size before parsing to avoid memory pressure.
        if data.len() as u64 > MAX_CONFIG_SNAPSHOT_BYTES {
            return Err(ConfigSyncError::TooLarge);
        }

        // Read the version first so a snapshot with an unsupported version is
        // rejected without attempting a v1 parse of its body.
        let header: serde_json::Value = serde_json::from_slice(data)
            .map_err(|e| ConfigSyncError::InvalidSnapshot(e.to_string()))?;
        let version = header
            .get("schema_version")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ConfigSyncError::InvalidSnapshot("missing schema_version".to_string()))?
            as u32;
        if version != CURRENT_SCHEMA_VERSION {
            return Err(ConfigSyncError::UnsupportedVersion(version));
        }

        let snapshot: Self = serde_json::from_value(header)
            .map_err(|e| ConfigSyncError::InvalidSnapshot(e.to_string()))?;

        // Validate timestamp.
        chrono::DateTime::parse_from_rfc3339(&snapshot.uploaded_at)
            .map_err(|e| ConfigSyncError::InvalidTimestamp(e.to_string()))?;

        // Validate field values. Any invalid value rejects the whole
        // snapshot — no silent repair of individual fields.
        snapshot.settings.validate()?;

        Ok(snapshot)
    }

    /// Serialise to a pretty-printed JSON byte vector.
    pub fn to_vec(&self) -> Result<Vec<u8>, ConfigSyncError> {
        // Apply the same policy to locally generated snapshots so invalid
        // local/TOML values can never poison the remote snapshot slot.
        self.settings.validate()?;

        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| ConfigSyncError::InvalidSnapshot(e.to_string()))?;

        if json.len() as u64 > MAX_CONFIG_SNAPSHOT_BYTES {
            return Err(ConfigSyncError::TooLarge);
        }

        Ok(json)
    }
}

// ── PortableSettingsV1 ────────────────────────────────────────────────────

impl PortableSettingsV1 {
    /// Extract only whitelist fields from the current `AppSettings`.
    pub fn from_local(settings: &AppSettings) -> Self {
        Self {
            theme: settings.theme.clone(),
            language: settings.language.clone(),
            auto_hide: settings.auto_hide,
            silent_start: settings.silent_start,
            window_position_mode: settings.window_position_mode.clone(),
            card_height_mode: settings.card_height_mode.clone(),
            auto_focus_search: settings.auto_focus_search,
            clear_search_on_show: settings.clear_search_on_show,
            always_reset_to_clipboard: settings.always_reset_to_clipboard,
            quick_window_sync_filters: Some(settings.quick_window_sync_filters),
            font_size_scale: Some(settings.font_size_scale),
            font_size_level: Some(settings.font_size_level.clone()),

            sort_by_created: settings.sort_by_created,
            show_source_app: settings.show_source_app,
            auto_scroll_to_top: settings.auto_scroll_to_top,
            copy_as_plain_text: settings.copy_as_plain_text,
            show_original_on_hover: settings.show_original_on_hover,
            type_filter_config: settings.type_filter_config.clone(),
            search_favorites_first: Some(settings.search_favorites_first),
            paste_click_mode: Some(settings.paste_click_mode.clone()),
            filter_foreign_paths: Some(settings.filter_foreign_paths),

            ocr_enabled: settings.ocr_enabled,
            qr_enabled: settings.qr_enabled,
            auto_fetch_url_title: settings.auto_fetch_url_title,
            copy_sound_enabled: settings.copy_sound_enabled,
            copy_sound_file: settings.copy_sound_file.clone(),
            image_alt_mode: settings.image_alt_mode.clone(),
            record_images: Some(settings.record_images),
            record_files: Some(settings.record_files),

            max_items: settings.max_items,
            retention_days: settings.retention_days,
            cleanup_interval: settings.cleanup_interval.clone(),
            cleanup_stale_items: settings.cleanup_stale_items,

            sync_interval_secs: settings.sync_interval_secs,
            sync_auto_enabled: settings.sync_auto_enabled,
            sync_favorites_only: settings.sync_favorites_only,
            sync_include_images: settings.sync_include_images,
            sync_compress_images: settings.sync_compress_images,

            transfer_station_enabled: settings.transfer_station_enabled,
            transfer_retention_days: settings.transfer_retention_days,

            auto_check_updates: settings.auto_check_updates,
        }
    }

    /// Apply whitelist fields onto `current`, returning a new `AppSettings`.
    ///
    /// Fields not declared in `PortableSettingsV1` retain their values from
    /// `current`. This includes hotkeys, blacklists, window geometry, database
    /// paths, backend credentials, and runtime timestamps.
    pub fn apply_to(&self, current: &AppSettings) -> AppSettings {
        let mut result = current.clone();

        // ── Appearance & Interaction ──
        result.theme = self.theme.clone();
        result.language = self.language.clone();
        result.auto_hide = self.auto_hide;
        result.silent_start = self.silent_start;
        result.window_position_mode = self.window_position_mode.clone();
        result.card_height_mode = self.card_height_mode.clone();
        result.auto_focus_search = self.auto_focus_search;
        result.clear_search_on_show = self.clear_search_on_show;
        result.always_reset_to_clipboard = self.always_reset_to_clipboard;
        if let Some(value) = self.quick_window_sync_filters {
            result.quick_window_sync_filters = value;
        }
        if let Some(value) = self.font_size_scale {
            result.font_size_scale = value;
        }
        if let Some(value) = &self.font_size_level {
            result.font_size_level = value.clone();
        }

        // ── List & Content ──
        result.sort_by_created = self.sort_by_created;
        result.show_source_app = self.show_source_app;
        result.auto_scroll_to_top = self.auto_scroll_to_top;
        result.copy_as_plain_text = self.copy_as_plain_text;
        result.show_original_on_hover = self.show_original_on_hover;
        result.type_filter_config = self.type_filter_config.clone();
        if let Some(value) = self.search_favorites_first {
            result.search_favorites_first = value;
        }
        if let Some(value) = &self.paste_click_mode {
            result.paste_click_mode = value.clone();
        }
        if let Some(value) = self.filter_foreign_paths {
            result.filter_foreign_paths = value;
        }

        // ── Clipboard Features ──
        result.ocr_enabled = self.ocr_enabled;
        result.qr_enabled = self.qr_enabled;
        result.auto_fetch_url_title = self.auto_fetch_url_title;
        result.copy_sound_enabled = self.copy_sound_enabled;
        result.copy_sound_file = self.copy_sound_file.clone();
        result.image_alt_mode = self.image_alt_mode.clone();
        if let Some(value) = self.record_images {
            result.record_images = value;
        }
        if let Some(value) = self.record_files {
            result.record_files = value;
        }

        // ── Data Retention ──
        result.max_items = self.max_items;
        result.retention_days = self.retention_days;
        result.cleanup_interval = self.cleanup_interval.clone();
        result.cleanup_stale_items = self.cleanup_stale_items;

        // ── Sync Policy ──
        result.sync_interval_secs = self.sync_interval_secs;
        result.sync_auto_enabled = self.sync_auto_enabled;
        result.sync_favorites_only = self.sync_favorites_only;
        result.sync_include_images = self.sync_include_images;
        result.sync_compress_images = self.sync_compress_images;

        // ── Transfer Station Policy ──
        result.transfer_station_enabled = self.transfer_station_enabled;
        result.transfer_retention_days = self.transfer_retention_days;

        // ── Updates ──
        result.auto_check_updates = self.auto_check_updates;

        result
    }

    /// Validate all field values. Returns `Ok(())` or the first
    /// `ConfigSyncError::InvalidFieldValue`. Any invalid value rejects the
    /// whole snapshot — fields are never silently repaired.
    fn validate(&self) -> Result<(), ConfigSyncError> {
        if let Some(Some(scale)) = self.font_size_scale {
            // Match the range supported by ui::font::apply_scale.
            if !scale.is_finite() || !(0.8..=1.6).contains(&scale) {
                return Err(ConfigSyncError::InvalidFieldValue(format!(
                    "font_size_scale out of range: {scale}"
                )));
            }
        }
        if let Some(level) = &self.font_size_level {
            if !["compact", "standard", "large", "xlarge"].contains(&level.as_str()) {
                return Err(ConfigSyncError::InvalidFieldValue(format!(
                    "unknown font_size_level: {level}"
                )));
            }
        }
        if let Some(mode) = &self.paste_click_mode {
            if !["single_click", "double_click"].contains(&mode.as_str()) {
                return Err(ConfigSyncError::InvalidFieldValue(format!(
                    "unknown paste_click_mode: {mode}"
                )));
            }
        }
        // theme
        if !["system", "dark", "light"].contains(&self.theme.as_str()) {
            return Err(ConfigSyncError::InvalidFieldValue(format!(
                "unknown theme: {}",
                self.theme
            )));
        }

        // language
        if !["", "zh_CN", "en"].contains(&self.language.as_str()) {
            return Err(ConfigSyncError::InvalidFieldValue(format!(
                "unknown language: {}",
                self.language
            )));
        }

        // window_position_mode
        if !["center", "follow", "remember"].contains(&self.window_position_mode.as_str()) {
            return Err(ConfigSyncError::InvalidFieldValue(format!(
                "unknown position mode: {}",
                self.window_position_mode
            )));
        }

        // card_height_mode
        if !["low", "medium", "high", "auto"].contains(&self.card_height_mode.as_str()) {
            return Err(ConfigSyncError::InvalidFieldValue(format!(
                "unknown card height mode: {}",
                self.card_height_mode
            )));
        }

        // copy_sound_file — every sound is embedded in the binary, so the
        // value must be one of the known files. Unknown values reject the
        // whole snapshot.
        if !VALID_COPY_SOUND_FILES.contains(&self.copy_sound_file.as_str()) {
            return Err(ConfigSyncError::InvalidFieldValue(format!(
                "unknown copy sound file: {}",
                self.copy_sound_file
            )));
        }

        // image_alt_mode
        if !["bitmap", "path", "ocr"].contains(&self.image_alt_mode.as_str()) {
            return Err(ConfigSyncError::InvalidFieldValue(format!(
                "unknown image alt mode: {}",
                self.image_alt_mode
            )));
        }

        // cleanup_interval
        if !["daily", "weekly", "never"].contains(&self.cleanup_interval.as_str()) {
            return Err(ConfigSyncError::InvalidFieldValue(format!(
                "unknown cleanup interval: {}",
                self.cleanup_interval
            )));
        }

        // max_items — 0 means unlimited; cap to reject runaway values.
        if self.max_items > MAX_ITEMS_LIMIT {
            return Err(ConfigSyncError::InvalidFieldValue(format!(
                "max_items out of range: {}",
                self.max_items
            )));
        }

        // retention_days — 0 means keep forever; cap to reject values that
        // would delete every non-favorite item on the receiving device.
        if self.retention_days > MAX_RETENTION_DAYS {
            return Err(ConfigSyncError::InvalidFieldValue(format!(
                "retention_days out of range: {}",
                self.retention_days
            )));
        }

        // sync_interval_secs — the global sync cadence must stay within sane
        // bounds so a bad snapshot cannot disable or overwhelm syncing.
        if !(MIN_SYNC_INTERVAL_SECS..=MAX_SYNC_INTERVAL_SECS).contains(&self.sync_interval_secs) {
            return Err(ConfigSyncError::InvalidFieldValue(format!(
                "sync_interval_secs out of range: {}",
                self.sync_interval_secs
            )));
        }

        // transfer_retention_days — must be one of the UI options. Oversized
        // values would overflow Chrono date arithmetic in the transfer
        // station (`DateTime + Duration::days(u32::MAX)` panics).
        if !TRANSFER_RETENTION_OPTIONS.contains(&self.transfer_retention_days) {
            return Err(ConfigSyncError::InvalidFieldValue(format!(
                "transfer_retention_days out of range: {}",
                self.transfer_retention_days
            )));
        }

        // type_filter_config — only built-in keys, no duplicates, and at most
        // one entry per built-in key. Unknown keys would still consume toolbar
        // width in the filter bar (they are skipped only at render time), and
        // duplicate or oversized lists would let a snapshot bloat the layout
        // and per-frame iteration cost.
        if self.type_filter_config.len() > BUILTIN_TYPE_KEYS.len() {
            return Err(ConfigSyncError::InvalidFieldValue(format!(
                "type_filter_config too long: {}",
                self.type_filter_config.len()
            )));
        }
        let mut seen = Vec::with_capacity(self.type_filter_config.len());
        for entry in &self.type_filter_config {
            if !BUILTIN_TYPE_KEYS.contains(&entry.key.as_str()) {
                return Err(ConfigSyncError::InvalidFieldValue(format!(
                    "unknown type filter key: {}",
                    entry.key
                )));
            }
            if seen.contains(&entry.key) {
                return Err(ConfigSyncError::InvalidFieldValue(format!(
                    "duplicate type filter key: {}",
                    entry.key
                )));
            }
            seen.push(entry.key.clone());
        }

        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_settings() -> AppSettings {
        AppSettings::default()
    }

    fn full_settings_json(theme: &str) -> serde_json::Value {
        serde_json::json!({
            "theme": theme,
            "language": "",
            "auto_hide": true,
            "silent_start": true,
            "window_position_mode": "center",
            "card_height_mode": "auto",
            "auto_focus_search": false,
            "clear_search_on_show": false,
            "always_reset_to_clipboard": false,
            "sort_by_created": false,
            "show_source_app": false,
            "auto_scroll_to_top": false,
            "copy_as_plain_text": false,
            "show_original_on_hover": false,
            "type_filter_config": [],
            "ocr_enabled": false,
            "qr_enabled": true,
            "auto_fetch_url_title": true,
            "copy_sound_enabled": true,
            "copy_sound_file": "copy_penclick.wav",
            "image_alt_mode": "bitmap",
            "max_items": 0,
            "retention_days": 0,
            "cleanup_interval": "never",
            "cleanup_stale_items": false,
            "sync_interval_secs": 60,
            "sync_auto_enabled": false,
            "sync_favorites_only": true,
            "sync_include_images": false,
            "sync_compress_images": false,
            "transfer_station_enabled": false,
            "transfer_retention_days": 3,
            "auto_check_updates": true
        })
    }

    fn snapshot_json(theme: &str) -> serde_json::Value {
        serde_json::json!({
            "schema_version": 1,
            "uploaded_at": "2026-08-03T12:34:56Z",
            "source": {
                "app_version": "1.0.0",
                "platform": "windows",
                "device_name": "test"
            },
            "settings": full_settings_json(theme)
        })
    }

    #[test]
    fn added_preferences_roundtrip_and_apply_explicit_false() {
        let source = AppSettings {
            quick_window_sync_filters: false,
            font_size_scale: Some(1.225),
            font_size_level: "large".into(),
            search_favorites_first: true,
            paste_click_mode: "single_click".into(),
            filter_foreign_paths: true,
            record_images: false,
            record_files: false,
            ..AppSettings::default()
        };
        let snapshot = ConfigSnapshot::from_local(&source, "test");
        let parsed = ConfigSnapshot::from_slice(&snapshot.to_vec().unwrap()).unwrap();
        let target = AppSettings {
            hotkey: "Alt+Z".into(),
            font_family: "Local Font".into(),
            ..AppSettings::default()
        };
        let result = parsed.settings.apply_to(&target);
        assert!(!result.quick_window_sync_filters);
        assert_eq!(result.font_size_scale, source.font_size_scale);
        assert_eq!(result.font_size_level, "large");
        assert!(result.search_favorites_first);
        assert_eq!(result.paste_click_mode, "single_click");
        assert!(result.filter_foreign_paths);
        assert!(!result.record_images);
        assert!(!result.record_files);
        assert_eq!(result.hotkey, "Alt+Z");
        assert_eq!(result.font_family, "Local Font");
    }

    #[test]
    fn older_v1_snapshot_preserves_added_local_preferences() {
        // This fixture intentionally contains only the original 33 fields.
        let data = serde_json::to_vec(&snapshot_json("dark")).unwrap();
        let parsed = ConfigSnapshot::from_slice(&data).unwrap();
        let current = AppSettings {
            quick_window_sync_filters: false,
            font_size_scale: Some(1.3),
            font_size_level: "xlarge".into(),
            search_favorites_first: true,
            paste_click_mode: "single_click".into(),
            filter_foreign_paths: true,
            record_images: false,
            record_files: false,
            ..AppSettings::default()
        };
        let result = parsed.settings.apply_to(&current);
        assert_eq!(result.theme, "dark");
        assert_eq!(
            result.quick_window_sync_filters,
            current.quick_window_sync_filters
        );
        assert_eq!(result.font_size_scale, current.font_size_scale);
        assert_eq!(result.font_size_level, current.font_size_level);
        assert_eq!(
            result.search_favorites_first,
            current.search_favorites_first
        );
        assert_eq!(result.paste_click_mode, current.paste_click_mode);
        assert_eq!(result.filter_foreign_paths, current.filter_foreign_paths);
        assert_eq!(result.record_images, current.record_images);
        assert_eq!(result.record_files, current.record_files);
        let reserialized: serde_json::Value =
            serde_json::from_slice(&parsed.to_vec().unwrap()).unwrap();
        assert!(reserialized["settings"].get("font_size_scale").is_none());
    }

    #[test]
    fn explicit_null_font_scale_restores_legacy_size() {
        let source = AppSettings {
            font_size_scale: None,
            font_size_level: "compact".into(),
            ..AppSettings::default()
        };
        let bytes = ConfigSnapshot::from_local(&source, "test")
            .to_vec()
            .unwrap();
        let parsed = ConfigSnapshot::from_slice(&bytes).unwrap();
        assert_eq!(parsed.settings.font_size_scale, Some(None));
        let current = AppSettings {
            font_size_scale: Some(1.3),
            ..AppSettings::default()
        };
        let result = parsed.settings.apply_to(&current);
        assert_eq!(result.font_size_scale, None);
        assert_eq!(result.font_size_level, "compact");
    }

    #[test]
    fn rejects_invalid_added_preferences() {
        for (key, value) in [
            ("font_size_scale", serde_json::json!(0.0)),
            ("font_size_scale", serde_json::json!(1.7)),
            ("font_size_scale", serde_json::json!("large")),
            ("font_size_level", serde_json::json!("huge")),
            ("paste_click_mode", serde_json::json!("triple_click")),
            ("record_images", serde_json::Value::Null),
            ("record_files", serde_json::json!("false")),
            ("quick_window_sync_filters", serde_json::Value::Null),
            ("search_favorites_first", serde_json::json!(1)),
            ("filter_foreign_paths", serde_json::json!([])),
        ] {
            let mut json = snapshot_json("system");
            json["settings"][key] = value;
            let bytes = serde_json::to_vec(&json).unwrap();
            assert!(ConfigSnapshot::from_slice(&bytes).is_err(), "{key}");
        }
        for scale in [f32::NAN, f32::INFINITY, -1.0, 2.0] {
            let source = AppSettings {
                font_size_scale: Some(scale),
                ..AppSettings::default()
            };
            assert!(ConfigSnapshot::from_local(&source, "test")
                .to_vec()
                .is_err());
        }
    }

    #[test]
    fn from_local_only_produces_whitelist_fields() {
        let settings = test_settings();
        let portable = PortableSettingsV1::from_local(&settings);

        assert_eq!(portable.theme, settings.theme);
        assert_eq!(portable.auto_hide, settings.auto_hide);
        assert_eq!(portable.card_height_mode, settings.card_height_mode);
    }

    #[test]
    fn apply_to_overwrites_whitelist_fields() {
        let mut current = test_settings();
        current.theme = "dark".to_string();
        current.auto_hide = false;

        let mut portable = PortableSettingsV1::from_local(&test_settings());
        portable.theme = "light".to_string();
        portable.auto_hide = true;

        let result = portable.apply_to(&current);
        assert_eq!(result.theme, "light");
        assert!(result.auto_hide);
    }

    #[test]
    fn apply_to_preserves_excluded_fields() {
        let mut current = test_settings();
        current.hotkey = "Alt+X".to_string();
        current.db_path = "/custom/path".to_string();
        let backend = crate::core::settings::BackendConfig {
            id: "test-id".to_string(),
            enabled: true,
            backend_type: "local_folder".to_string(),
            name: "test".to_string(),
            folder_path: String::new(),
            device_name: String::new(),
            last_sync_at: String::new(),
            last_item_count: 0,
            last_tag_count: 0,
            sync_interval_secs: None,
            webdav_url: String::new(),
            webdav_root_url: String::new(),
            webdav_path: String::new(),
            webdav_username: String::new(),
            webdav_password: String::new(),
        };
        current.sync_backends = vec![backend];

        let portable = PortableSettingsV1::from_local(&test_settings());
        let result = portable.apply_to(&current);

        // Excluded fields must stay.
        assert_eq!(result.hotkey, "Alt+X");
        assert_eq!(result.db_path, "/custom/path");
        assert_eq!(result.sync_backends.len(), 1);
        assert_eq!(result.sync_backends[0].id, "test-id");
    }

    #[test]
    fn apply_to_idempotent() {
        let current = test_settings();
        let portable = PortableSettingsV1::from_local(&current);
        let first = portable.apply_to(&current);
        let second = portable.apply_to(&first);
        assert_eq!(first.theme, second.theme);
        assert_eq!(first.auto_hide, second.auto_hide);
    }

    #[test]
    fn json_roundtrip_preserves_portable_settings() {
        let settings = test_settings();
        let portable = PortableSettingsV1::from_local(&settings);

        let snapshot = ConfigSnapshot {
            schema_version: CURRENT_SCHEMA_VERSION,
            uploaded_at: "2026-08-03T12:34:56Z".to_string(),
            source: ConfigSnapshotSource {
                app_version: "1.0.0".to_string(),
                platform: "windows".to_string(),
                device_name: "test-pc".to_string(),
            },
            settings: portable.clone(),
        };

        let json = serde_json::to_vec(&snapshot).unwrap();
        let parsed: ConfigSnapshot = serde_json::from_slice(&json).unwrap();

        assert_eq!(parsed.settings, portable);
    }

    #[test]
    fn full_roundtrip_whitelist_fields_match_source() {
        let mut source = test_settings();
        source.theme = "dark".to_string();
        source.auto_hide = false;
        source.card_height_mode = "medium".to_string();
        source.max_items = 500;
        source.sync_auto_enabled = true;

        let portable = PortableSettingsV1::from_local(&source);
        let json = serde_json::to_vec(&portable).unwrap();
        let parsed: PortableSettingsV1 = serde_json::from_slice(&json).unwrap();

        let mut target = test_settings();
        target.hotkey = "Alt+Z".to_string();
        target.db_path = "/keep/me".to_string();

        let result = parsed.apply_to(&target);

        // Whitelist from source.
        assert_eq!(result.theme, "dark");
        assert!(!result.auto_hide);
        assert_eq!(result.card_height_mode, "medium");
        assert_eq!(result.max_items, 500);
        assert!(result.sync_auto_enabled);

        // Excluded from target preserved.
        assert_eq!(result.hotkey, "Alt+Z");
        assert_eq!(result.db_path, "/keep/me");
    }

    #[test]
    fn rejects_newer_unsupported_version() {
        let mut json = snapshot_json("system");
        json["schema_version"] = serde_json::json!(2);
        let data = serde_json::to_vec(&json).unwrap();
        let result = ConfigSnapshot::from_slice(&data);
        assert!(matches!(
            result,
            Err(ConfigSyncError::UnsupportedVersion(2))
        ));
    }

    #[test]
    fn rejects_zero_schema_version() {
        let mut json = snapshot_json("system");
        json["schema_version"] = serde_json::json!(0);
        let data = serde_json::to_vec(&json).unwrap();
        let result = ConfigSnapshot::from_slice(&data);
        assert!(matches!(
            result,
            Err(ConfigSyncError::UnsupportedVersion(0))
        ));
    }

    #[test]
    fn rejects_missing_schema_version() {
        let mut json = snapshot_json("system");
        json.as_object_mut().unwrap().remove("schema_version");
        let data = serde_json::to_vec(&json).unwrap();
        let result = ConfigSnapshot::from_slice(&data);
        assert!(matches!(result, Err(ConfigSyncError::InvalidSnapshot(_))));
    }

    #[test]
    fn rejects_invalid_timestamp() {
        let mut json = snapshot_json("system");
        json["uploaded_at"] = serde_json::json!("not-a-timestamp");
        let data = serde_json::to_vec(&json).unwrap();
        let result = ConfigSnapshot::from_slice(&data);
        assert!(matches!(result, Err(ConfigSyncError::InvalidTimestamp(_))));
    }

    #[test]
    fn rejects_invalid_theme() {
        let json = snapshot_json("neon");
        let data = serde_json::to_vec(&json).unwrap();
        let result = ConfigSnapshot::from_slice(&data);
        assert!(matches!(result, Err(ConfigSyncError::InvalidFieldValue(_))));
    }

    #[test]
    fn rejects_unknown_copy_sound_file() {
        let mut json = snapshot_json("system");
        json["settings"]["copy_sound_file"] = serde_json::json!("custom_sound.wav");
        let data = serde_json::to_vec(&json).unwrap();
        let result = ConfigSnapshot::from_slice(&data);
        assert!(matches!(result, Err(ConfigSyncError::InvalidFieldValue(_))));
    }

    #[test]
    fn rejects_transfer_retention_days_outside_ui_options() {
        for invalid in [4_u32, 31, 365, u32::MAX] {
            let mut json = snapshot_json("system");
            json["settings"]["transfer_retention_days"] = serde_json::json!(invalid);
            let data = serde_json::to_vec(&json).unwrap();
            let result = ConfigSnapshot::from_slice(&data);
            assert!(
                matches!(result, Err(ConfigSyncError::InvalidFieldValue(_))),
                "transfer_retention_days {invalid} must be rejected"
            );
        }
    }

    #[test]
    fn accepts_transfer_retention_days_in_ui_options() {
        for valid in [0_u32, 1, 3, 7, 30] {
            let mut json = snapshot_json("system");
            json["settings"]["transfer_retention_days"] = serde_json::json!(valid);
            let data = serde_json::to_vec(&json).unwrap();
            let parsed = ConfigSnapshot::from_slice(&data)
                .unwrap_or_else(|e| panic!("transfer_retention_days {valid} rejected: {e}"));
            assert_eq!(parsed.settings.transfer_retention_days, valid);
        }
    }

    #[test]
    fn rejects_max_items_above_limit() {
        let mut json = snapshot_json("system");
        json["settings"]["max_items"] = serde_json::json!(MAX_ITEMS_LIMIT + 1);
        let data = serde_json::to_vec(&json).unwrap();
        let result = ConfigSnapshot::from_slice(&data);
        assert!(matches!(result, Err(ConfigSyncError::InvalidFieldValue(_))));
    }

    #[test]
    fn rejects_retention_days_above_limit() {
        let mut json = snapshot_json("system");
        json["settings"]["retention_days"] = serde_json::json!(MAX_RETENTION_DAYS + 1);
        let data = serde_json::to_vec(&json).unwrap();
        let result = ConfigSnapshot::from_slice(&data);
        assert!(matches!(result, Err(ConfigSyncError::InvalidFieldValue(_))));
    }

    #[test]
    fn rejects_sync_interval_out_of_bounds() {
        for invalid in [0_u64, MAX_SYNC_INTERVAL_SECS + 1, u64::MAX] {
            let mut json = snapshot_json("system");
            json["settings"]["sync_interval_secs"] = serde_json::json!(invalid);
            let data = serde_json::to_vec(&json).unwrap();
            let result = ConfigSnapshot::from_slice(&data);
            assert!(
                matches!(result, Err(ConfigSyncError::InvalidFieldValue(_))),
                "sync_interval_secs {invalid} must be rejected"
            );
        }
    }

    #[test]
    fn accepts_boundary_numeric_values() {
        let mut json = snapshot_json("system");
        json["settings"]["max_items"] = serde_json::json!(MAX_ITEMS_LIMIT);
        json["settings"]["retention_days"] = serde_json::json!(MAX_RETENTION_DAYS);
        json["settings"]["sync_interval_secs"] = serde_json::json!(MAX_SYNC_INTERVAL_SECS);
        let data = serde_json::to_vec(&json).unwrap();
        assert!(ConfigSnapshot::from_slice(&data).is_ok());
    }

    #[test]
    fn rejects_unknown_type_filter_key() {
        let mut json = snapshot_json("system");
        json["settings"]["type_filter_config"] = serde_json::json!([
            { "key": "plain_text", "visible": true },
            { "key": "custom_type", "visible": true }
        ]);
        let data = serde_json::to_vec(&json).unwrap();
        let result = ConfigSnapshot::from_slice(&data);
        assert!(matches!(result, Err(ConfigSyncError::InvalidFieldValue(_))));
    }

    #[test]
    fn rejects_duplicate_type_filter_keys() {
        let mut json = snapshot_json("system");
        json["settings"]["type_filter_config"] = serde_json::json!([
            { "key": "plain_text", "visible": true },
            { "key": "plain_text", "visible": false }
        ]);
        let data = serde_json::to_vec(&json).unwrap();
        let result = ConfigSnapshot::from_slice(&data);
        assert!(matches!(result, Err(ConfigSyncError::InvalidFieldValue(_))));
    }

    #[test]
    fn rejects_oversized_type_filter_config() {
        let mut json = snapshot_json("system");
        let entries: Vec<serde_json::Value> = BUILTIN_TYPE_KEYS
            .iter()
            .map(|key| serde_json::json!({ "key": key, "visible": true }))
            .collect();
        let mut duplicate = entries.clone();
        duplicate.push(serde_json::json!({ "key": "plain_text", "visible": true }));
        json["settings"]["type_filter_config"] = serde_json::Value::Array(duplicate);
        let data = serde_json::to_vec(&json).unwrap();
        let result = ConfigSnapshot::from_slice(&data);
        assert!(matches!(result, Err(ConfigSyncError::InvalidFieldValue(_))));
    }

    #[test]
    fn accepts_full_builtin_type_filter_config() {
        let mut json = snapshot_json("system");
        let entries: Vec<serde_json::Value> = BUILTIN_TYPE_KEYS
            .iter()
            .map(|key| serde_json::json!({ "key": key, "visible": true }))
            .collect();
        json["settings"]["type_filter_config"] = serde_json::Value::Array(entries);
        let data = serde_json::to_vec(&json).unwrap();
        assert!(ConfigSnapshot::from_slice(&data).is_ok());
    }

    #[test]
    fn rejects_invalid_values_before_upload_serialization() {
        let settings = AppSettings {
            transfer_retention_days: u32::MAX,
            ..AppSettings::default()
        };
        let snapshot = ConfigSnapshot::from_local(&settings, "0.4.1");
        assert!(matches!(
            snapshot.to_vec(),
            Err(ConfigSyncError::InvalidFieldValue(_))
        ));
    }

    #[test]
    fn rejects_snapshot_missing_known_fields() {
        // A truncated snapshot missing most fields must be rejected, not
        // silently defaulted to false/0 and applied.
        let json = serde_json::json!({
            "schema_version": 1,
            "uploaded_at": "2026-08-03T12:34:56Z",
            "source": {
                "app_version": "1.0.0",
                "platform": "windows",
                "device_name": "test"
            },
            "settings": {
                "theme": "dark",
                "language": ""
            }
        });
        let data = serde_json::to_vec(&json).unwrap();
        let result = ConfigSnapshot::from_slice(&data);
        assert!(matches!(result, Err(ConfigSyncError::InvalidSnapshot(_))));
    }

    #[test]
    fn ignores_unknown_json_fields() {
        // Simulate a newer Clippi adding a field that this version doesn't know.
        let mut json = snapshot_json("dark");
        json["settings"]["future_feature_flag"] = serde_json::json!(true);
        let data = serde_json::to_vec(&json).unwrap();
        let parsed = ConfigSnapshot::from_slice(&data).unwrap();
        assert_eq!(parsed.settings.theme, "dark");
    }

    #[test]
    fn passwords_never_appear_in_serialized_json() {
        let mut settings = test_settings();
        settings
            .sync_backends
            .push(crate::core::settings::BackendConfig {
                id: "id".to_string(),
                enabled: true,
                backend_type: "webdav".to_string(),
                name: "test".to_string(),
                folder_path: String::new(),
                device_name: String::new(),
                last_sync_at: String::new(),
                last_item_count: 0,
                last_tag_count: 0,
                sync_interval_secs: None,
                webdav_url: String::new(),
                webdav_root_url: String::new(),
                webdav_path: String::new(),
                webdav_username: String::new(),
                webdav_password: "secret123".to_string(),
            });

        let portable = PortableSettingsV1::from_local(&settings);
        let json = serde_json::to_string(&portable).unwrap();
        assert!(!json.contains("secret123"));
    }

    #[test]
    fn local_paths_never_appear_in_serialized_json() {
        let mut settings = test_settings();
        settings.db_path = "/home/user/secret/data".to_string();

        let portable = PortableSettingsV1::from_local(&settings);
        let json = serde_json::to_string(&portable).unwrap();
        assert!(!json.contains("/home/user/secret/data"));
    }

    #[test]
    fn rejects_corrupt_json() {
        let result = ConfigSnapshot::from_slice(b"not json");
        assert!(matches!(result, Err(ConfigSyncError::InvalidSnapshot(_))));
    }

    #[test]
    fn rejects_oversized_payload() {
        let big = vec![b'x'; (MAX_CONFIG_SNAPSHOT_BYTES as usize) + 1];
        let result = ConfigSnapshot::from_slice(&big);
        assert!(matches!(result, Err(ConfigSyncError::TooLarge)));
    }
}
