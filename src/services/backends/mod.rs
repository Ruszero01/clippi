pub mod local_folder;
pub mod webdav;

use crate::core::config_sync::ConfigSyncError;
use crate::core::settings::BackendConfig;
use std::sync::Arc;

/// Trait for backends that support manual config-snapshot upload/download.
///
/// This is independent of `SyncBackend` — config snapshots use a separate
/// file (`clippi_settings.json`) and are never pushed/pulled by the
/// automatic clipboard sync cycle.
pub trait ConfigSnapshotBackend: Send + Sync {
    /// Download the remote `clippi_settings.json` as raw bytes.
    ///
    /// Returns `Ok(None)` when the file does not exist on the remote.
    /// The caller is responsible for JSON parsing and validation.
    fn download_config_snapshot(&self, max_bytes: u64) -> Result<Option<Vec<u8>>, ConfigSyncError>;

    /// Upload `data` as the remote `clippi_settings.json`, overwriting any
    /// existing snapshot. The backend should use atomic replacement where
    /// possible (temp file + rename for local folders, single PUT for
    /// WebDAV).
    fn upload_config_snapshot(&self, data: &[u8]) -> Result<(), ConfigSyncError>;

    /// Human-readable label for the backend (used in confirmation dialogs).
    #[allow(dead_code)]
    fn backend_name(&self) -> String;
}

/// Create a `ConfigSnapshotBackend` from a `BackendConfig`.
pub fn create_config_snapshot_backend(
    config: &BackendConfig,
) -> Option<Arc<dyn ConfigSnapshotBackend>> {
    match config.backend_type.as_str() {
        "local_folder" => Some(Arc::new(local_folder::LocalFolderBackend::new(
            config.clone(),
        ))),
        "webdav" => Some(Arc::new(webdav::WebDAVBackend::new(config.clone()))),
        _ => None,
    }
}
