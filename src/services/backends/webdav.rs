//! --- WebDAV sync backend. ---
//!
//! --- Reads/writes `clippi_sync.json` to a WebDAV server via HTTP. ---
//! Uses ETag/If-None-Match for cache-aware pulls (analogous to
//! --- mtime-based caching in local_folder). ---

use crate::core::i18n_keys::I18nKey;
use crate::core::settings::BackendConfig;
use crate::core::sync::{BackendStatus, SyncBackend, SyncPayload};
use base64::Engine;
use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;

const SYNC_FILENAME: &str = "clippi_sync.json";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

enum CollectionCheck {
    Exists,
    Missing,
    Error(BackendStatus),
}

fn is_success_status(status: u16) -> bool {
    (200..400).contains(&status)
}

fn is_auth_status(status: u16) -> bool {
    status == 401 || status == 403
}

pub fn check_webdav_connection(
    agent: &ureq::Agent,
    url: &str,
    auth: &str,
) -> Result<(), BackendStatus> {
    let base = url.trim_end_matches('/');
    let file_url = format!("{base}/{SYNC_FILENAME}");

    match check_collection(agent, base, auth) {
        CollectionCheck::Exists => return Ok(()),
        CollectionCheck::Missing => {
            if create_collection_path(agent, base, auth).is_ok() {
                return Ok(());
            }
        }
        CollectionCheck::Error(status) => return Err(status),
    }

    // Fallback for WebDAV-compatible endpoints that do not support PROPFIND.
    match agent
        .request("PROPFIND", base)
        .set("Authorization", auth)
        .set("Depth", "0")
        .call()
    {
        Ok(response) if is_success_status(response.status()) => return Ok(()),
        Ok(response) if is_auth_status(response.status()) => {
            return Err(BackendStatus::Error(I18nKey::SyncErrAuth.text().into()));
        }
        Err(ureq::Error::Status(code, _)) if is_auth_status(code) => {
            return Err(BackendStatus::Error(I18nKey::SyncErrAuth.text().into()));
        }
        _ => {}
    }

    for test_url in [file_url.as_str(), base] {
        match agent.head(test_url).set("Authorization", auth).call() {
            Ok(response) if is_success_status(response.status()) => return Ok(()),
            Ok(response) if is_auth_status(response.status()) => {
                return Err(BackendStatus::Error(I18nKey::SyncErrAuth.text().into()));
            }
            Err(ureq::Error::Status(404, _)) => continue,
            Err(ureq::Error::Status(405, _)) if test_url == base => return Ok(()),
            Err(ureq::Error::Status(code, _)) if is_auth_status(code) => {
                return Err(BackendStatus::Error(I18nKey::SyncErrAuth.text().into()));
            }
            Err(error) => {
                return Err(BackendStatus::Error(format!(
                    "{}: {error}",
                    I18nKey::SyncErrConnect.text()
                )));
            }
            _ => {}
        }
    }

    Err(BackendStatus::Error(I18nKey::SyncErrConnect.text().into()))
}

fn check_collection(agent: &ureq::Agent, url: &str, auth: &str) -> CollectionCheck {
    match agent
        .request("PROPFIND", url)
        .set("Authorization", auth)
        .set("Depth", "0")
        .call()
    {
        Ok(response) if is_success_status(response.status()) => CollectionCheck::Exists,
        Ok(response) if is_auth_status(response.status()) => {
            CollectionCheck::Error(BackendStatus::Error(I18nKey::SyncErrAuth.text().into()))
        }
        Err(ureq::Error::Status(404, _)) | Err(ureq::Error::Status(409, _)) => {
            CollectionCheck::Missing
        }
        Err(ureq::Error::Status(405, _)) => CollectionCheck::Exists,
        Err(ureq::Error::Status(code, _)) if is_auth_status(code) => {
            CollectionCheck::Error(BackendStatus::Error(I18nKey::SyncErrAuth.text().into()))
        }
        Err(error) => CollectionCheck::Error(BackendStatus::Error(format!(
            "{}: {error}",
            I18nKey::SyncErrConnect.text()
        ))),
        _ => CollectionCheck::Missing,
    }
}

fn create_collection_path(agent: &ureq::Agent, url: &str, auth: &str) -> Result<(), BackendStatus> {
    let Some((origin, path_segments)) = split_url_path(url) else {
        return Err(BackendStatus::Error(I18nKey::SyncErrConnect.text().into()));
    };
    if path_segments.is_empty() {
        return Ok(());
    }

    let mut missing = Vec::new();
    for end in (1..=path_segments.len()).rev() {
        let candidate = join_url(&origin, &path_segments[..end]);
        match check_collection(agent, &candidate, auth) {
            CollectionCheck::Exists => {
                missing = path_segments[end..].to_vec();
                break;
            }
            CollectionCheck::Missing => {
                if end == 1 {
                    missing = path_segments.clone();
                }
            }
            CollectionCheck::Error(status) => return Err(status),
        }
    }

    let existing_count = path_segments.len().saturating_sub(missing.len());
    let mut current = path_segments[..existing_count].to_vec();
    for segment in missing {
        current.push(segment);
        let collection_url = join_url(&origin, &current);
        match agent
            .request("MKCOL", &collection_url)
            .set("Authorization", auth)
            .call()
        {
            Ok(response) if (200..300).contains(&response.status()) => {}
            Err(ureq::Error::Status(405, _)) => {}
            Err(ureq::Error::Status(409, _)) => {}
            Err(ureq::Error::Status(code, _)) if is_auth_status(code) => {
                return Err(BackendStatus::Error(I18nKey::SyncErrAuth.text().into()));
            }
            Err(error) => {
                return Err(BackendStatus::Error(format!(
                    "{}: {error}",
                    I18nKey::SyncErrConnect.text()
                )));
            }
            _ => return Err(BackendStatus::Error(I18nKey::SyncErrConnect.text().into())),
        }
    }

    match check_collection(agent, url, auth) {
        CollectionCheck::Exists => Ok(()),
        CollectionCheck::Missing => {
            Err(BackendStatus::Error(I18nKey::SyncErrConnect.text().into()))
        }
        CollectionCheck::Error(status) => Err(status),
    }
}

fn split_url_path(url: &str) -> Option<(String, Vec<String>)> {
    let scheme_end = url.find("://")? + 3;
    let path_start = url[scheme_end..]
        .find('/')
        .map(|index| scheme_end + index)
        .unwrap_or(url.len());
    let origin = url[..path_start].to_string();
    let path = url[path_start..].trim_matches('/');
    let segments = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').map(ToOwned::to_owned).collect()
    };
    Some((origin, segments))
}

fn join_url(origin: &str, segments: &[String]) -> String {
    if segments.is_empty() {
        origin.to_string()
    } else {
        format!("{}/{}", origin, segments.join("/"))
    }
}

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
    fn sync_interval(&self) -> u64 {
        self.config.sync_interval_secs.unwrap_or(600)
    }

    fn check_status(&self) -> BackendStatus {
        if self.config.webdav_url.is_empty() {
            return BackendStatus::Error(I18nKey::SyncErrNoUrl.text().into());
        }
        let auth = self.auth_header();
        match check_webdav_connection(&self.agent, &self.config.webdav_url, &auth) {
            Ok(()) => BackendStatus::Online,
            Err(status) => status,
        }
    }

    fn pull(&self, bypass_cache: bool) -> Result<SyncPayload, String> {
        let url = self.file_url();
        let auth = self.auth_header();

        let cached_etag = if bypass_cache {
            None
        } else {
            self.last_etag
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        };
        let attempts = if cached_etag.is_some() { 2 } else { 1 };

        for attempt in 0..attempts {
            let use_cached_etag = attempt == 0;
            let mut req = self.agent.get(&url).set("Authorization", &auth);
            if use_cached_etag {
                if let Some(etag) = cached_etag.as_deref() {
                    req = req.set("If-None-Match", etag);
                }
            }

            match req.call() {
                Ok(resp) => {
                    // --- Cache the new ETag ---
                    if let Some(etag) = resp.header("ETag") {
                        *self.last_etag.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(etag.to_string());
                    }
                    let body = resp
                        .into_string()
                        .map_err(|e| format!("{}: {e}", I18nKey::SyncErrReadResp.text()))?;
                    return serde_json::from_str::<SyncPayload>(&body)
                        .map_err(|e| format!("{}: {e}", I18nKey::SyncErrParse.text()));
                }
                Err(ureq::Error::Status(304, _)) => {
                    // --- Not Modified — no changes ---
                    return Err("@@unchanged".into());
                }
                Err(ureq::Error::Status(404, _)) => {
                    return Err(I18nKey::SyncErrNotFound.text().into());
                }
                Err(ureq::Error::Status(code, response))
                    if use_cached_etag && !is_auth_status(code) =>
                {
                    log::warn!(
                        "[sync] WebDAV conditional GET failed with status {}; retrying without ETag",
                        response.status()
                    );
                    continue;
                }
                Err(e) => return Err(format!("{}: {e}", I18nKey::SyncErrPull.text())),
            }
        }

        Err(I18nKey::SyncErrPull.text().into())
    }

    fn push(&self, payload: &SyncPayload) -> Result<(), String> {
        let url = self.file_url();
        let auth = self.auth_header();
        let json = serde_json::to_string_pretty(payload)
            .map_err(|e| format!("{}: {e}", I18nKey::SyncErrSerialize.text()))?;

        match self
            .agent
            .put(&url)
            .set("Authorization", &auth)
            .set("Content-Type", "application/json")
            .send_bytes(json.as_bytes())
        {
            Ok(resp) => {
                // --- Cache the new ETag ---
                if let Some(etag) = resp.header("ETag") {
                    *self.last_etag.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(etag.to_string());
                }
                Ok(())
            }
            Err(e) => Err(format!("{}: {e}", I18nKey::SyncErrPush.text())),
        }
    }

    fn upload_blob(&self, hash_hex: &str, ext: &str, data: &[u8]) -> Result<(), String> {
        let base = self.config.webdav_url.trim_end_matches('/');
        let images_url = format!("{base}/images");
        let blob_url = format!("{images_url}/{hash_hex}.{ext}");
        let auth = self.auth_header();

        // Ensure images/ directory exists (try MKCOL, ignore if already exists)
        let _ = self
            .agent
            .request("MKCOL", &images_url)
            .set("Authorization", &auth)
            .call();

        let content_type = match ext {
            "jpg" | "jpeg" => "image/jpeg",
            _ => "image/png",
        };

        match self
            .agent
            .put(&blob_url)
            .set("Authorization", &auth)
            .set("Content-Type", content_type)
            .send_bytes(data)
        {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("blob upload failed: {e}")),
        }
    }

    fn download_blob(&self, hash_hex: &str, ext: &str) -> Result<Vec<u8>, String> {
        let base = self.config.webdav_url.trim_end_matches('/');
        let blob_url = format!("{base}/images/{hash_hex}.{ext}");
        let auth = self.auth_header();

        match self.agent.get(&blob_url).set("Authorization", &auth).call() {
            Ok(resp) => {
                let mut buf = Vec::new();
                resp.into_reader()
                    .read_to_end(&mut buf)
                    .map_err(|e| format!("blob read failed: {e}"))?;
                Ok(buf)
            }
            Err(ureq::Error::Status(404, _)) => Err("blob not found".into()),
            Err(e) => Err(format!("blob download failed: {e}")),
        }
    }

    fn list_remote_blobs(&self) -> Result<Vec<String>, String> {
        let base = self.config.webdav_url.trim_end_matches('/');
        let images_url = format!("{base}/images/");
        let auth = self.auth_header();

        match self
            .agent
            .request("PROPFIND", &images_url)
            .set("Authorization", &auth)
            .set("Depth", "1")
            .call()
        {
            Ok(resp) => {
                let body = resp
                    .into_string()
                    .map_err(|e| format!("PROPFIND read failed: {e}"))?;
                // Extract filenames from href elements in the XML response
                let mut files = Vec::new();
                for line in body.lines() {
                    if let Some(href) = extract_href_filename(line) {
                        if !href.is_empty() && href != "images" && !href.ends_with('/') {
                            files.push(href);
                        }
                    }
                }
                Ok(files)
            }
            Err(ureq::Error::Status(404, _)) => Ok(Vec::new()),
            Err(e) => Err(format!("PROPFIND failed: {e}")),
        }
    }
}

/// Extract the filename from a DAV:href XML element.
/// Handles both `<D:href>filename.png</D:href>` and `<d:href>...</d:href>`.
fn extract_href_filename(line: &str) -> Option<String> {
    let line = line.trim();
    // Match <href>...</href> or <D:href>...</D:href> or <d:href>...</d:href>
    let start_tag_end = line.find("href>")?;
    let content_start = start_tag_end + 5; // "href>".len()
    let rest = &line[content_start..];
    let content_end = rest.find("</")?;
    let href = rest[..content_end].trim();

    if href.is_empty() {
        return None;
    }

    // Extract just the filename from the path
    let filename = href.rsplit('/').next().unwrap_or(href);
    if filename.is_empty() {
        return None;
    }

    Some(
        percent_encoding::percent_decode_str(filename)
            .decode_utf8_lossy()
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_url_path_keeps_webdav_segments() {
        let (origin, segments) =
            split_url_path("https://dav.jianguoyun.com/dav/我的坚果云/Clippi/").unwrap();

        assert_eq!(origin, "https://dav.jianguoyun.com");
        assert_eq!(segments, vec!["dav", "我的坚果云", "Clippi"]);
    }

    #[test]
    fn join_url_rebuilds_collection_url() {
        let segments = vec!["dav".to_string(), "我的坚果云".to_string()];

        assert_eq!(
            join_url("https://dav.jianguoyun.com", &segments),
            "https://dav.jianguoyun.com/dav/我的坚果云"
        );
    }
}
