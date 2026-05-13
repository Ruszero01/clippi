//! Core types - platform-agnostic

use chrono::{DateTime, Utc};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

/// Content type of clipboard items
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    PlainText,
    RichText,
    Image,
    Link,
    Color,
    File,
    Path,
}

impl ContentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ContentType::PlainText => "plain_text",
            ContentType::RichText => "rich_text",
            ContentType::Image => "image",
            ContentType::Link => "link",
            ContentType::Color => "color",
            ContentType::File => "file",
            ContentType::Path => "path",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "plain_text" | "text" => ContentType::PlainText,
            "rich_text" | "html" => ContentType::RichText,
            "image" => ContentType::Image,
            "link" => ContentType::Link,
            "color" => ContentType::Color,
            "file" => ContentType::File,
            "path" => ContentType::Path,
            _ => ContentType::PlainText,
        }
    }
}

/// Source application info extracted when clipboard content is first captured
#[derive(Debug, Clone)]
pub struct SourceAppInfo {
    pub app_name: String,
    pub icon_base64: String, // PNG icon encoded as base64
}

/// A clipboard item
#[derive(Debug, Clone)]
pub struct ClipboardItem {
    pub id: i64,
    pub content_type: ContentType,
    pub full_text: String,
    pub content_hash: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub image_path: String,
    pub rich_data: String, // JSON: {"html":"...","rtf":"..."} or empty
    pub file_data: String, // JSON: [{"name":"...","path":"...","is_dir":false}, ...]
    pub is_favorite: bool,
    pub note: String,
    pub source_app_name: String,
    pub source_app_icon: String, // base64-encoded PNG icon
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
pub struct RichData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtf: Option<String>,
}

impl RichData {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }

}

/// Info for a single file within a clipboard file group
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

/// File group data serialized as JSON in the database
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct FileData {
    pub files: Vec<FileInfo>,
}

impl FileData {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }

    pub fn display_text(&self) -> String {
        self.files.iter()
            .map(|f| f.name.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Return a human-readable label for a file extension or directory
pub fn get_extension_label(name: &str) -> String {
    if let Some(idx) = name.rfind('.') {
        name[idx..].to_lowercase()
    } else {
        "文件".to_string()
    }
}

/// Check if a file path has a common image extension
pub fn is_image_extension(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".gif")
        || lower.ends_with(".bmp")
        || lower.ends_with(".webp")
        || lower.ends_with(".ico")
        || lower.ends_with(".tiff")
        || lower.ends_with(".tif")
}

impl ClipboardItem {
    pub fn new_text(id: i64, text: &str, content_type: ContentType, source: Option<&SourceAppInfo>, rich_data: Option<&RichData>) -> Self {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let now = Utc::now();
        let (app_name, icon) = source.map_or((String::new(), String::new()), |s| (s.app_name.clone(), s.icon_base64.clone()));
        let rd = rich_data.map(|r| r.to_json()).unwrap_or_default();
        Self {
            id,
            content_type,
            full_text: text.to_string(),
            content_hash: hasher.finish(),
            created_at: now,
            updated_at: now,
            image_path: String::new(),
            rich_data: rd,
            file_data: String::new(),
            is_favorite: false,
            note: String::new(),
            source_app_name: app_name,
            source_app_icon: icon,
        }
    }

    pub fn new_image(id: i64, image_path: &str, hash: u64, source: Option<&SourceAppInfo>) -> Self {
        let now = Utc::now();
        let (app_name, icon) = source.map_or((String::new(), String::new()), |s| (s.app_name.clone(), s.icon_base64.clone()));
        Self {
            id,
            content_type: ContentType::Image,
            full_text: image_path.to_string(),
            content_hash: hash,
            created_at: now,
            updated_at: now,
            image_path: image_path.to_string(),
            rich_data: String::new(),
            file_data: String::new(),
            is_favorite: false,
            note: String::new(),
            source_app_name: app_name,
            source_app_icon: icon,
        }
    }

    pub fn new_color(id: i64, text: &str, hash: u64, source: Option<&SourceAppInfo>) -> Self {
        let now = Utc::now();
        let (app_name, icon) = source.map_or((String::new(), String::new()), |s| (s.app_name.clone(), s.icon_base64.clone()));
        Self {
            id,
            content_type: ContentType::Color,
            full_text: text.to_string(),
            content_hash: hash,
            created_at: now,
            updated_at: now,
            image_path: String::new(),
            rich_data: String::new(),
            file_data: String::new(),
            is_favorite: false,
            note: String::new(),
            source_app_name: app_name,
            source_app_icon: icon,
        }
    }

    pub fn new_file(id: i64, file_data: &FileData, hash: u64, source: Option<&SourceAppInfo>) -> Self {
        let now = Utc::now();
        let display = file_data.display_text();
        let (app_name, icon) = source.map_or((String::new(), String::new()), |s| (s.app_name.clone(), s.icon_base64.clone()));
        Self {
            id,
            content_type: ContentType::File,
            full_text: display,
            content_hash: hash,
            created_at: now,
            updated_at: now,
            image_path: String::new(),
            rich_data: String::new(),
            file_data: file_data.to_json(),
            is_favorite: false,
            note: String::new(),
            source_app_name: app_name,
            source_app_icon: icon,
        }
    }
}


/// Format elapsed time as human-readable string
pub fn format_relative_time(captured_at: &DateTime<Utc>) -> String {
    let elapsed = Utc::now().signed_duration_since(*captured_at);
    let secs = elapsed.num_seconds();
    if secs < 60 {
        "刚刚".to_string()
    } else if secs < 3600 {
        format!("{}分钟前", secs / 60)
    } else if secs < 86400 {
        format!("{}小时前", secs / 3600)
    } else if secs < 604800 {
        format!("{}天前", secs / 86400)
    } else {
        format!("{}周前", secs / 604800)
    }
}

/// Check if text is a web URL (http:// or https:// only).
pub fn is_url(text: &str) -> bool {
    let text = text.trim();
    if text.contains('\n') {
        return false;
    }
    (text.starts_with("http://") || text.starts_with("https://")) && text.len() > 10
}

/// Check if text is a file system path (Windows absolute, UNC, or Unix absolute).
pub fn is_path(text: &str) -> bool {
    let text = text.trim();
    if text.contains('\n') {
        return false;
    }
    // Windows absolute path: C:\..., D:/...
    if text.len() >= 3
        && text.as_bytes()[0].is_ascii_alphabetic()
        && text.as_bytes()[1] == b':'
        && (text.as_bytes()[2] == b'\\' || text.as_bytes()[2] == b'/')
    {
        return true;
    }
    // UNC network path: \\server\share\... or \\192.168.1.1\...
    if text.starts_with("\\\\") && text.len() > 2 {
        return true;
    }
    // Unix absolute path: /Users/..., /etc/..., /tmp/...
    if text.starts_with('/') && text.len() >= 3 && text.as_bytes()[1] != b'/' {
        return true;
    }
    false
}

/// Extract the domain portion from a URL for display.
/// "https://www.github.com/user/repo" -> "www.github.com"
pub fn url_domain(text: &str) -> String {
    let s = text.trim();
    let no_scheme = s.strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    match no_scheme.find(|c: char| c == '/' || c == '?' || c == '#') {
        Some(pos) => no_scheme[..pos].to_string(),
        None => no_scheme.to_string(),
    }
}

/// Extract the path, query, and fragment from a URL for display.
/// "https://www.github.com/user/repo?tab=stars" -> "/user/repo?tab=stars"
/// Returns empty string if the URL has no path portion.
pub fn url_path(text: &str) -> String {
    let s = text.trim();
    let no_scheme = s.strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    match no_scheme.find(|c: char| c == '/' || c == '?' || c == '#') {
        Some(pos) => no_scheme[pos..].to_string(),
        None => String::new(),
    }
}

/// Extract the domain from a URL for favicon lookup (same as url_domain).
pub fn url_to_domain(text: &str) -> String {
    url_domain(text)
}
