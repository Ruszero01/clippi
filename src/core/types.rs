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
}

impl ContentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ContentType::PlainText => "plain_text",
            ContentType::RichText => "rich_text",
            ContentType::Image => "image",
            ContentType::Link => "link",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "plain_text" | "text" => ContentType::PlainText,
            "rich_text" | "html" => ContentType::RichText,
            "image" => ContentType::Image,
            "link" => ContentType::Link,
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
    pub is_favorite: bool,
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
            is_favorite: false,
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
            is_favorite: false,
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

/// Check if text is a URL (http/https)
pub fn is_url(text: &str) -> bool {
    let text = text.trim();
    (text.starts_with("http://") || text.starts_with("https://"))
        && !text.contains('\n')
        && text.len() > 10
}
