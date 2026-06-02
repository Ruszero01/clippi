//! Sync state — cloud sync status and backend management.
//!
//! Tracks the current state of each sync backend and the last sync time.
//! The actual sync logic lives in `services::sync::SyncManager`.

use crate::core::settings::BackendConfig;

/// Per-backend sync status.
#[derive(Debug, Clone)]
pub struct BackendStatus {
    pub config: BackendConfig,
    /// Whether a sync cycle is currently running
    pub syncing: bool,
    /// Last sync result message
    pub last_message: String,
    /// Whether the last sync had an error
    pub last_error: bool,
}

/// Sync state owned by `AppState`.
#[derive(Debug, Default)]
pub struct SyncState {
    /// Configured backends with their current status
    pub backends: Vec<BackendStatus>,
    /// Global auto-sync enabled
    pub auto_enabled: bool,
    /// Sync interval in seconds
    pub interval_secs: u64,
    /// Favorites-only sync
    pub favorites_only: bool,
    /// Last sync timestamp (RFC3339)
    pub last_sync_at: String,
    /// Number of items from last sync
    pub last_item_count: u32,
    /// Number of tags from last sync
    pub last_tag_count: u32,
    /// Whether any sync cycle is running
    pub syncing: bool,
}

impl SyncState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build sync state from current settings.
    pub fn from_settings(settings: &crate::core::settings::AppSettings) -> Self {
        Self {
            backends: settings
                .sync_backends
                .iter()
                .map(|b| BackendStatus {
                    config: b.clone(),
                    syncing: false,
                    last_message: String::new(),
                    last_error: false,
                })
                .collect(),
            auto_enabled: settings.sync_auto_enabled,
            interval_secs: settings.sync_interval_secs,
            favorites_only: settings.sync_favorites_only,
            ..Default::default()
        }
    }
}
