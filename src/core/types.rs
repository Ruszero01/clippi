//! Core types - platform-agnostic

use chrono::{DateTime, Utc};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

/// Content type of clipboard items
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Text,
    Html,
    Image,
}

impl ContentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ContentType::Text => "text",
            ContentType::Html => "html",
            ContentType::Image => "image",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "html" => ContentType::Html,
            "image" => ContentType::Image,
            _ => ContentType::Text,
        }
    }
}

/// A clipboard item
#[derive(Debug, Clone)]
pub struct ClipboardItem {
    pub id: i64,
    pub content_type: ContentType,
    pub text_preview: String,
    pub full_text: String,
    pub content_hash: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ClipboardItem {
    pub fn new_text(id: i64, text: &str) -> Self {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let preview: String = text.chars().take(200).collect();
        let now = Utc::now();
        Self {
            id,
            content_type: ContentType::Text,
            text_preview: preview,
            full_text: text.to_string(),
            content_hash: hasher.finish(),
            created_at: now,
            updated_at: now,
        }
    }
}

unsafe impl Send for ClipboardItem {}
unsafe impl Sync for ClipboardItem {}
