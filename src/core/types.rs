//! Core types - platform-agnostic

use chrono::{DateTime, Utc};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

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

/// User-defined tag with name and color
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TagInfo {
    pub id: i64,
    pub name: String,
    pub color: String, // 6-digit uppercase hex, e.g. "FF5733"
    #[serde(default)]
    pub updated_at: String, // RFC3339, used for sync conflict resolution
}

/// 12 preset tag colors: (name, hex)
pub fn tag_preset_colors() -> &'static [(&'static str, &'static str)] {
    use crate::core::i18n;
    if i18n::is_en() {
        &[
            ("Red", "#EF4444"),
            ("Orange", "#F97316"),
            ("Yellow", "#EAB308"),
            ("Green", "#22C55E"),
            ("Cyan", "#06B6D4"),
            ("Blue", "#3B82F6"),
            ("Indigo", "#6366F1"),
            ("Purple", "#A855F7"),
            ("Pink", "#EC4899"),
            ("Gray", "#6B7280"),
            ("Brown", "#92400E"),
            ("Sky", "#0EA5E9"),
        ]
    } else {
        &[
            ("红色", "#EF4444"),
            ("橙色", "#F97316"),
            ("黄色", "#EAB308"),
            ("绿色", "#22C55E"),
            ("青色", "#06B6D4"),
            ("蓝色", "#3B82F6"),
            ("靛蓝", "#6366F1"),
            ("紫色", "#A855F7"),
            ("粉色", "#EC4899"),
            ("灰色", "#6B7280"),
            ("棕色", "#92400E"),
            ("天蓝", "#0EA5E9"),
        ]
    }
}

/// Pick the next color from presets in round-robin order
pub fn next_tag_color(index: usize) -> &'static str {
    let colors = tag_preset_colors();
    colors[index % colors.len()].1
}

/// Parse hex color "#EF4444" → (r, g, b)
pub fn parse_hex_color(hex: &str) -> Option<(u8, u8, u8)> {
    let s = hex.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    Some((
        u8::from_str_radix(&s[0..2], 16).ok()?,
        u8::from_str_radix(&s[2..4], 16).ok()?,
        u8::from_str_radix(&s[4..6], 16).ok()?,
    ))
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
    pub image_width: u32,
    pub image_height: u32,
    pub rich_data: String, // JSON: {"html":"...","rtf":"..."} or empty
    pub file_data: String, // JSON: [{"name":"...","path":"...","is_dir":false}, ...]
    pub is_favorite: bool,
    pub note: String,
    pub source_app_name: String,
    pub source_app_icon: String, // base64-encoded PNG icon
    pub size: i64,               // byte count for files, char count for text
    pub tags: Vec<TagInfo>,
    pub meta_type: String, // subtype for plain_text: "" | "email" | "phone"
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
pub struct RichData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtf: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qr_text: Option<String>,
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
        self.files
            .iter()
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
        use crate::core::i18n;
        i18n::tr("文件", "File").to_string()
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
    pub fn new_text(
        id: i64,
        text: &str,
        content_type: ContentType,
        source: Option<&SourceAppInfo>,
        rich_data: Option<&RichData>,
    ) -> Self {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let now = Utc::now();
        let (app_name, icon) = source.map_or((String::new(), String::new()), |s| {
            (s.app_name.clone(), s.icon_base64.clone())
        });
        let rd = rich_data.map(|r| r.to_json()).unwrap_or_default();
        Self {
            id,
            content_type,
            full_text: text.to_string(),
            content_hash: hasher.finish(),
            created_at: now,
            updated_at: now,
            image_path: String::new(),
            image_width: 0,
            image_height: 0,
            rich_data: rd,
            file_data: String::new(),
            is_favorite: false,
            note: String::new(),
            source_app_name: app_name,
            source_app_icon: icon,
            size: 0,
            tags: Vec::new(),
            meta_type: String::new(),
        }
    }

    pub fn new_image(
        id: i64,
        image_path: &str,
        hash: u64,
        w: u32,
        h: u32,
        source: Option<&SourceAppInfo>,
    ) -> Self {
        let now = Utc::now();
        let (app_name, icon) = source.map_or((String::new(), String::new()), |s| {
            (s.app_name.clone(), s.icon_base64.clone())
        });
        Self {
            id,
            content_type: ContentType::Image,
            full_text: image_path.to_string(),
            content_hash: hash,
            created_at: now,
            updated_at: now,
            image_path: image_path.to_string(),
            image_width: w,
            image_height: h,
            rich_data: String::new(),
            file_data: String::new(),
            is_favorite: false,
            note: String::new(),
            source_app_name: app_name,
            source_app_icon: icon,
            size: 0,
            tags: Vec::new(),
            meta_type: String::new(),
        }
    }

    pub fn new_color(id: i64, text: &str, hash: u64, source: Option<&SourceAppInfo>) -> Self {
        let now = Utc::now();
        let (app_name, icon) = source.map_or((String::new(), String::new()), |s| {
            (s.app_name.clone(), s.icon_base64.clone())
        });
        Self {
            id,
            content_type: ContentType::Color,
            full_text: text.to_string(),
            content_hash: hash,
            created_at: now,
            updated_at: now,
            image_path: String::new(),
            image_width: 0,
            image_height: 0,
            rich_data: String::new(),
            file_data: String::new(),
            is_favorite: false,
            note: String::new(),
            source_app_name: app_name,
            source_app_icon: icon,
            size: 0,
            tags: Vec::new(),
            meta_type: String::new(),
        }
    }

    pub fn new_file(
        id: i64,
        file_data: &FileData,
        hash: u64,
        source: Option<&SourceAppInfo>,
        size: i64,
    ) -> Self {
        let now = Utc::now();
        let display = file_data.display_text();
        let (app_name, icon) = source.map_or((String::new(), String::new()), |s| {
            (s.app_name.clone(), s.icon_base64.clone())
        });
        Self {
            id,
            content_type: ContentType::File,
            full_text: display,
            content_hash: hash,
            created_at: now,
            updated_at: now,
            image_path: String::new(),
            image_width: 0,
            image_height: 0,
            rich_data: String::new(),
            file_data: file_data.to_json(),
            is_favorite: false,
            note: String::new(),
            source_app_name: app_name,
            source_app_icon: icon,
            size,
            tags: Vec::new(),
            meta_type: String::new(),
        }
    }
}

/// Format elapsed time as human-readable string
pub fn format_relative_time(captured_at: &DateTime<Utc>) -> String {
    let elapsed = Utc::now().signed_duration_since(*captured_at);
    let secs = elapsed.num_seconds();
    use crate::core::i18n;
    if secs < 60 {
        i18n::tr("刚刚", "Just now").to_string()
    } else if secs < 3600 {
        if i18n::is_en() {
            format!("{} min ago", secs / 60)
        } else {
            format!("{}分钟前", secs / 60)
        }
    } else if secs < 86400 {
        if i18n::is_en() {
            format!("{} hr ago", secs / 3600)
        } else {
            format!("{}小时前", secs / 3600)
        }
    } else if secs < 604800 {
        if i18n::is_en() {
            format!("{} days ago", secs / 86400)
        } else {
            format!("{}天前", secs / 86400)
        }
    } else {
        if i18n::is_en() {
            format!("{} wks ago", secs / 604800)
        } else {
            format!("{}周前", secs / 604800)
        }
    }
}

/// Check if text is solely a web URL (http:// or https:// only).
/// Rejects text that merely starts with a URL but contains extra content.
pub fn is_url(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    if text.contains('\n') || text.contains(' ') {
        return false;
    }
    (text.starts_with("http://") || text.starts_with("https://")) && text.len() > 10
}

/// Check if text is solely an email address.
pub fn is_email(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.contains('\n') || text.contains(' ') {
        return false;
    }
    // Must contain exactly one @ not at start or end
    let at_pos = match text.find('@') {
        Some(p) if p > 0 && p < text.len() - 1 => p,
        _ => return false,
    };
    // Local part: alphanumeric + limited special chars, no consecutive dots
    let local = &text[..at_pos];
    if local.is_empty() || local.len() > 64 || local.contains("..") {
        return false;
    }
    if !local
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || ".!#$%&'*+/=?^_`{|}~-".contains(c))
    {
        return false;
    }
    // Domain part: must have a dot, no consecutive dots, valid chars
    let domain = &text[at_pos + 1..];
    if domain.is_empty() || domain.len() > 255 || domain.contains("..") {
        return false;
    }
    if !domain.contains('.') {
        return false;
    }
    if !domain
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c))
    {
        return false;
    }
    // TLD must start with a letter and be at least 2 chars
    let tld = match domain.rfind('.') {
        Some(p) => &domain[p + 1..],
        None => return false,
    };
    tld.len() >= 2 && tld.starts_with(|c: char| c.is_ascii_alphabetic())
}

/// Check if text is solely a phone number (Chinese mobile / international format).
pub fn is_phone(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.contains('\n') {
        return false;
    }
    // Strip common separators
    let cleaned: String = text
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '(' | ')' | '\t'))
        .collect();
    // Chinese mobile: 1[3-9]xxxxxxxxx (11 digits, starts with 1)
    if cleaned.len() == 11
        && cleaned.starts_with('1')
        && cleaned.chars().all(|c| c.is_ascii_digit())
        && cleaned.as_bytes()[1] >= b'3'
        && cleaned.as_bytes()[1] <= b'9'
    {
        return true;
    }
    // International: +country region number (7-15 digit phone body)
    if cleaned.starts_with('+')
        && cleaned.len() >= 10
        && cleaned.len() <= 17
        && cleaned[1..].chars().all(|c| c.is_ascii_digit())
    {
        return true;
    }
    false
}

/// Check if text is solely a file system path (Windows absolute, UNC, or Unix absolute).
/// Rejects text that starts with a path but has extra descriptive content after it.
pub fn is_path(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.contains('\n') {
        return false;
    }
    // If the text contains a space, the content after the last path separator
    // must not itself contain a space — otherwise it's likely "path + description".
    if text.contains(' ') {
        let last_sep = text.rfind('\\').or_else(|| text.rfind('/'));
        if let Some(pos) = last_sep {
            if text[pos + 1..].contains(' ') {
                return false;
            }
        }
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
    // Require at least one "/" after the first char to avoid matching
    // slash commands like /clear, /help, etc.
    if text.starts_with('/')
        && text.len() >= 3
        && text.as_bytes()[1] != b'/'
        && text[1..].contains('/')
    {
        return true;
    }
    false
}

/// Extract the domain portion from a URL for display.
/// "https://www.github.com/user/repo" -> "www.github.com"
pub fn url_domain(text: &str) -> String {
    let s = text.trim();
    let no_scheme = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    match no_scheme.find(['/', '?', '#']) {
        Some(pos) => no_scheme[..pos].to_string(),
        None => no_scheme.to_string(),
    }
}

/// Extract the path, query, and fragment from a URL for display.
/// "https://www.github.com/user/repo?tab=stars" -> "/user/repo?tab=stars"
/// Returns empty string if the URL has no path portion.
pub fn url_path(text: &str) -> String {
    let s = text.trim();
    let no_scheme = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    match no_scheme.find(['/', '?', '#']) {
        Some(pos) => no_scheme[pos..].to_string(),
        None => String::new(),
    }
}

/// Extract the domain from a URL for favicon lookup (same as url_domain).
pub fn url_to_domain(text: &str) -> String {
    url_domain(text)
}

/// Mask sensitive content for preview display.
/// Email: show first 2 chars of local part + "***" + domain (e.g. "ab***@gmail.com")
/// Phone: show first 3 chars + "****" + last 4 chars (e.g. "138****5678")
pub fn mask_sensitive_preview(text: &str, meta_type: &str) -> String {
    match meta_type {
        "email" => {
            if let Some(at) = text.find('@') {
                let local = &text[..at];
                let domain = &text[at..];
                let visible = local.chars().take(2).collect::<String>();
                format!("{}***{}", visible, domain)
            } else {
                text.to_string()
            }
        }
        "phone" => {
            let cleaned: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            if cleaned.len() <= 7 {
                return text.to_string();
            }
            let prefix: String = cleaned.chars().take(3).collect();
            let suffix: String = cleaned
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("{}****{}", prefix, suffix)
        }
        _ => text.to_string(),
    }
}
