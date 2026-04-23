use chrono::{DateTime, Utc};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

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

#[derive(Debug, Clone)]
pub struct ClipboardItem {
    pub id: i64,
    pub content_type: ContentType,
    pub text_preview: String,
    pub full_text: String,
    pub content_hash: u64,
    pub captured_at: DateTime<Utc>,
}

impl ClipboardItem {
    pub fn new_text(id: i64, text: &str) -> Self {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let preview = if text.len() > 200 {
            &text[..200]
        } else {
            text
        };
        Self {
            id,
            content_type: ContentType::Text,
            text_preview: preview.to_string(),
            full_text: text.to_string(),
            content_hash: hasher.finish(),
            captured_at: Utc::now(),
        }
    }
}
