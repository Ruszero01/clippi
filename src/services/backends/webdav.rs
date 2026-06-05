//! WebDAV sync backend.
//!
//! Reads/writes `clippi_sync.json` to a WebDAV server via HTTP.
//! Uses ETag/If-None-Match for cache-aware pulls (analogous to
//! mtime-based caching in local_folder).

use crate::core::i18n;
use crate::core::settings::BackendConfig;
use crate::core::sync::{BackendStatus, BackendType, SyncBackend, SyncPayload};
use base64::Engine;
use std::sync::Mutex;
use std::time::Duration;

const SYNC_FILENAME: &str = "clippi_sync.json";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct WebDAVBackend {
    config: BackendConfig,
    /// Cached ETag from the last GET, used for If-None-Match.
    last_etag: Mutex<Option<String>>,
    agent: ureq::Agent,
}

impl WebDAVBackend {
    pub fn new(config: BackendConfig) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(REQUEST_TIMEOUT)
            .timeout_read(REQUEST_TIMEOUT)
            .timeout_write(REQUEST_TIMEOUT)
            .build();
        Self {
            config,
            last_etag: Mutex::new(None),
            agent,
        }
    }

    fn file_url(&self) -> String {
        let base = self.config.webdav_url.trim_end_matches('/');
        format!("{base}/{SYNC_FILENAME}")
    }

    fn auth_header(&self) -> String {
        let raw = format!(
            "{}:{}",
            self.config.webdav_username, self.config.webdav_password
        );
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(&raw)
        )
    }
}

impl SyncBackend for WebDAVBackend {
    fn id(&self) -> &str {
        &self.config.id
    }

    fn name(&self) -> &str {
        &self.config.name
    }

    fn backend_type(&self) -> BackendType {
        BackendType::WebDAV
    }

    fn sync_interval(&self) -> u64 {
        self.config.sync_interval_secs.unwrap_or(600)
    }

    fn check_status(&self) -> BackendStatus {
        if self.config.webdav_url.is_empty() {
            return BackendStatus::Error(i18n::tr("未配置 URL", "URL not configured").into());
        }
        let url = self.file_url();
        let auth = self.auth_header();
        match self.agent.head(&url).set("Authorization", &auth).call() {
            Ok(resp) => {
                let status = resp.status();
                if (200..400).contains(&status) {
                    BackendStatus::Online
                } else if status == 401 || status == 403 {
                    BackendStatus::Error(i18n::tr("认证失败", "Authentication failed").into())
                } else {
                    BackendStatus::Error(format!("HTTP {status}"))
                }
            }
            Err(ureq::Error::Status(code, _)) => {
                if code == 401 || code == 403 {
                    BackendStatus::Error(i18n::tr("认证失败", "Authentication failed").into())
                } else if code == 404 {
                    // File doesn't exist yet — treat as online (will create on first push)
                    // Check parent collection instead
                    let base = self.config.webdav_url.trim_end_matches('/');
                    match self.agent.head(base).set("Authorization", &auth).call() {
                        Ok(_) => BackendStatus::Online,
                        Err(_) => BackendStatus::Error(format!("HTTP {code}")),
                    }
                } else {
                    BackendStatus::Error(format!("HTTP {code}"))
                }
            }
            Err(e) => BackendStatus::Error(format!(
                "{}: {e}",
                i18n::tr("连接失败", "Connection failed")
            )),
        }
    }

    fn pull(&self, bypass_cache: bool) -> Result<SyncPayload, String> {
        let url = self.file_url();
        let auth = self.auth_header();

        let mut req = self.agent.get(&url).set("Authorization", &auth);

        // Use If-None-Match for etag-based caching
        if !bypass_cache {
            if let Some(etag) = self.last_etag.lock().unwrap().as_ref() {
                req = req.set("If-None-Match", etag);
            }
        }

        match req.call() {
            Ok(resp) => {
                // Cache the new ETag
                if let Some(etag) = resp.header("ETag") {
                    *self.last_etag.lock().unwrap() = Some(etag.to_string());
                }
                let body = resp.into_string().map_err(|e| {
                    format!(
                        "{}: {e}",
                        i18n::tr("读取响应失败", "Failed to read response")
                    )
                })?;
                serde_json::from_str::<SyncPayload>(&body).map_err(|e| {
                    format!(
                        "{}: {e}",
                        i18n::tr("解析同步文件失败", "Failed to parse sync file")
                    )
                })
            }
            Err(ureq::Error::Status(304, _)) => {
                // Not Modified — no changes
                Err("@@unchanged".into())
            }
            Err(ureq::Error::Status(404, _)) => {
                Err(i18n::tr("同步文件不存在", "Sync file not found").into())
            }
            Err(e) => Err(format!(
                "{}: {e}",
                i18n::tr("拉取同步文件失败", "Failed to pull sync file")
            )),
        }
    }

    fn push(&self, payload: &SyncPayload) -> Result<(), String> {
        let url = self.file_url();
        let auth = self.auth_header();
        let json = serde_json::to_string_pretty(payload)
            .map_err(|e| format!("{}: {e}", i18n::tr("序列化失败", "Serialization failed")))?;

        match self
            .agent
            .put(&url)
            .set("Authorization", &auth)
            .set("Content-Type", "application/json")
            .send_bytes(json.as_bytes())
        {
            Ok(resp) => {
                // Cache the new ETag
                if let Some(etag) = resp.header("ETag") {
                    *self.last_etag.lock().unwrap() = Some(etag.to_string());
                }
                Ok(())
            }
            Err(e) => Err(format!(
                "{}: {e}",
                i18n::tr("推送同步文件失败", "Failed to push sync file")
            )),
        }
    }
}
