//! Config-sync service — one-shot upload / download of portable settings.
//!
//! This service is independent of `GpuiSyncService`. It runs a single
//! blocking background operation at a time and reports results back to the
//! main thread via a shared mutex.
//!
//! # Concurrency
//!
//! A single `Mutex<State>` tracks the lifecycle so there is no window in
//! which the UI sees "idle" while a completed result is still waiting to be
//! consumed: the state only returns to `Idle` when the result is taken.
//! Starting a new operation while one is running or awaiting consumption is
//! rejected.

use std::sync::{Arc, Mutex};

use crate::core::config_sync::{ConfigSnapshot, ConfigSyncError, MAX_CONFIG_SNAPSHOT_BYTES};
use crate::core::settings::AppSettings;
use crate::services::backends::ConfigSnapshotBackend;

/// Result of a completed config-sync operation.
#[derive(Debug, Clone)]
pub enum ConfigSyncResult {
    /// Upload completed successfully.
    Uploaded,
    /// Download + validation succeeded; the snapshot is ready for the
    /// confirmation dialog.
    Downloaded(Box<ConfigSnapshot>),
    /// The operation failed.
    Error(ConfigSyncError),
}

/// Lifecycle of the single config-sync operation slot.
enum State {
    Idle,
    Running,
    /// A result is ready but not yet consumed by the main thread.
    Completed(ConfigSyncResult),
}

/// One-shot config-sync service.
///
/// Only one operation (upload or download) can be in flight at a time, and a
/// finished result must be consumed via [`ConfigSyncService::take_result`]
/// before the next operation can start. This guarantees the UI never loses a
/// success toast or a downloaded-snapshot confirmation.
pub struct ConfigSyncService {
    state: Arc<Mutex<State>>,
}

impl ConfigSyncService {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::Idle)),
        }
    }

    /// Whether an operation is running or its result is awaiting collection.
    pub fn is_busy(&self) -> bool {
        let Ok(state) = self.state.lock() else {
            return false;
        };
        !matches!(*state, State::Idle)
    }

    /// Start an upload in the background. Returns `true` if the operation
    /// was started; `false` if the slot is busy.
    pub fn start_upload(
        &self,
        settings: AppSettings,
        backend: Arc<dyn ConfigSnapshotBackend>,
    ) -> bool {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return false,
        };
        if !matches!(*state, State::Idle) {
            return false;
        }
        *state = State::Running;

        let shared = self.state.clone();
        std::thread::spawn(move || {
            let result = do_upload(&settings, &*backend);
            if let Ok(mut state) = shared.lock() {
                *state = State::Completed(result);
            }
        });

        true
    }

    /// Start a download in the background. Returns `true` if the operation
    /// was started; `false` if the slot is busy.
    pub fn start_download(&self, backend: Arc<dyn ConfigSnapshotBackend>) -> bool {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return false,
        };
        if !matches!(*state, State::Idle) {
            return false;
        }
        *state = State::Running;

        let shared = self.state.clone();
        std::thread::spawn(move || {
            let result = do_download(&*backend);
            if let Ok(mut state) = shared.lock() {
                *state = State::Completed(result);
            }
        });

        true
    }

    /// Collect a completed result (if any), returning the slot to `Idle`.
    pub fn take_result(&self) -> Option<ConfigSyncResult> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return None,
        };
        match std::mem::replace(&mut *state, State::Idle) {
            State::Completed(result) => Some(result),
            // Operation still running (or nothing pending): restore state.
            other => {
                *state = other;
                None
            }
        }
    }
}

// ── Background operations ─────────────────────────────────────────────────

fn do_upload(settings: &AppSettings, backend: &dyn ConfigSnapshotBackend) -> ConfigSyncResult {
    let snapshot = ConfigSnapshot::from_local(settings, env!("CARGO_PKG_VERSION"));

    let json = match snapshot.to_vec() {
        Ok(data) => data,
        Err(e) => return ConfigSyncResult::Error(e),
    };

    match backend.upload_config_snapshot(&json) {
        Ok(()) => ConfigSyncResult::Uploaded,
        Err(e) => ConfigSyncResult::Error(e),
    }
}

fn do_download(backend: &dyn ConfigSnapshotBackend) -> ConfigSyncResult {
    let raw = match backend.download_config_snapshot(MAX_CONFIG_SNAPSHOT_BYTES) {
        Ok(Some(data)) => data,
        Ok(None) => return ConfigSyncResult::Error(ConfigSyncError::RemoteNotFound),
        Err(e) => return ConfigSyncResult::Error(e),
    };

    match ConfigSnapshot::from_slice(&raw) {
        Ok(snapshot) => ConfigSyncResult::Downloaded(Box::new(snapshot)),
        Err(e) => ConfigSyncResult::Error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scripted backend that returns pre-configured results without touching
    /// the filesystem or network.
    struct FakeBackend {
        download: Result<Option<Vec<u8>>, ConfigSyncError>,
        upload: Result<(), ConfigSyncError>,
    }

    impl ConfigSnapshotBackend for FakeBackend {
        fn download_config_snapshot(
            &self,
            _max_bytes: u64,
        ) -> Result<Option<Vec<u8>>, ConfigSyncError> {
            self.download.clone()
        }

        fn upload_config_snapshot(&self, _data: &[u8]) -> Result<(), ConfigSyncError> {
            self.upload.clone()
        }

        fn backend_name(&self) -> String {
            "fake".into()
        }
    }

    fn valid_snapshot_bytes() -> Vec<u8> {
        ConfigSnapshot::from_local(&AppSettings::default(), "test")
            .to_vec()
            .unwrap()
    }

    #[test]
    fn download_backend_oversized_error_is_not_remapped_to_transport() {
        let backend = FakeBackend {
            download: Err(ConfigSyncError::TooLarge),
            upload: Ok(()),
        };
        assert!(matches!(
            do_download(&backend),
            ConfigSyncResult::Error(ConfigSyncError::TooLarge)
        ));
    }

    #[test]
    fn download_missing_snapshot_reports_remote_not_found() {
        let backend = FakeBackend {
            download: Ok(None),
            upload: Ok(()),
        };
        assert!(matches!(
            do_download(&backend),
            ConfigSyncResult::Error(ConfigSyncError::RemoteNotFound)
        ));
    }

    #[test]
    fn download_valid_snapshot_is_delivered() {
        let backend = FakeBackend {
            download: Ok(Some(valid_snapshot_bytes())),
            upload: Ok(()),
        };
        assert!(matches!(
            do_download(&backend),
            ConfigSyncResult::Downloaded(_)
        ));
    }

    #[test]
    fn download_invalid_json_reports_invalid_snapshot() {
        let backend = FakeBackend {
            download: Ok(Some(b"not json".to_vec())),
            upload: Ok(()),
        };
        assert!(matches!(
            do_download(&backend),
            ConfigSyncResult::Error(ConfigSyncError::InvalidSnapshot(_))
        ));
    }

    #[test]
    fn upload_success_reports_uploaded() {
        let backend = FakeBackend {
            download: Ok(None),
            upload: Ok(()),
        };
        assert!(matches!(
            do_upload(&AppSettings::default(), &backend),
            ConfigSyncResult::Uploaded
        ));
    }

    #[test]
    fn upload_backend_error_preserves_error_kind() {
        let backend = FakeBackend {
            download: Ok(None),
            upload: Err(ConfigSyncError::Transport("boom".into())),
        };
        assert!(matches!(
            do_upload(&AppSettings::default(), &backend),
            ConfigSyncResult::Error(ConfigSyncError::Transport(msg)) if msg == "boom"
        ));
    }
}
