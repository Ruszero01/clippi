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

/// A clipboard item
#[derive(Debug, Clone)]
pub struct ClipboardItem {
    pub id: i64,
    pub content_type: ContentType,
    pub full_text: String,
    pub searchable_text: String,
    pub content_hash: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub image_path: String,
    pub is_favorite: bool,
}

impl ClipboardItem {
    pub fn new_text(id: i64, text: &str, content_type: ContentType) -> Self {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let now = Utc::now();
        Self {
            id,
            content_type,
            full_text: text.to_string(),
            searchable_text: text.to_string(),
            content_hash: hasher.finish(),
            created_at: now,
            updated_at: now,
            image_path: String::new(),
            is_favorite: false,
        }
    }

    pub fn new_image(id: i64, image_path: &str, hash: u64) -> Self {
        let now = Utc::now();
        Self {
            id,
            content_type: ContentType::Image,
            full_text: image_path.to_string(),
            searchable_text: String::new(),
            content_hash: hash,
            created_at: now,
            updated_at: now,
            image_path: image_path.to_string(),
            is_favorite: false,
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
