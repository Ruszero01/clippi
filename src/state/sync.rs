//! --- GPUI-facing sync state. ---

use crate::core::settings::{AppSettings, BackendConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendStatus {
    pub config: BackendConfig,
    pub status: String,
    pub status_message: String,
    pub syncing: bool,
    pub service_label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncState {
    pub backends: Vec<BackendStatus>,
    pub auto_enabled: bool,
    pub favorites_only: bool,
    pub last_message: String,
}

impl SyncState {
    pub fn from_settings(settings: &AppSettings) -> Self {
        Self {
            backends: settings
                .sync_backends
                .iter()
                .cloned()
                .map(|config| BackendStatus {
                    service_label: service_label(&config),
                    config,
                    status: "offline".into(),
                    status_message: String::new(),
                    syncing: false,
                })
                .collect(),
            auto_enabled: settings.sync_auto_enabled,
            favorites_only: settings.sync_favorites_only,
            last_message: String::new(),
        }
    }
}

pub fn service_label(config: &BackendConfig) -> String {
    if config.backend_type == "webdav" {
        return config
            .webdav_url
            .strip_prefix("https://")
            .or_else(|| config.webdav_url.strip_prefix("http://"))
            .and_then(|url| url.split('/').next())
            .filter(|domain| !domain.is_empty())
            .unwrap_or("WebDAV")
            .to_string();
    }

    let lower = config.folder_path.to_lowercase();
    if lower.contains("onedrive") {
        "OneDrive".into()
    } else if lower.contains("icloud") {
        "iCloud".into()
    } else {
        "Local".into()
    }
}
