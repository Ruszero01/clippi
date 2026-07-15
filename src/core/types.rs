//! --- Core types - platform-agnostic ---

use chrono::{DateTime, Utc};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Content type of clipboard items.
///
/// There are four primary types. Semantic sub-types (link, path, color, email,
/// phone, markdown, html) are recorded in the `meta_type` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    PlainText,
    RichText,
    Image,
    File,
}

impl ContentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ContentType::PlainText => "plain_text",
            ContentType::RichText => "rich_text",
            ContentType::Image => "image",
            ContentType::File => "file",
        }
    }

    /// Parse a content type string from the database or sync payload.
    ///
    /// Legacy values (`"link"`, `"path"`, `"color"`) map to `PlainText` — the
    /// migration (v4) should have updated those rows. Unknown values fall back
    /// to `PlainText` with a warning log.
    pub fn from_str(s: &str) -> Self {
        match s {
            "plain_text" | "text" => ContentType::PlainText,
            "rich_text" | "html" => ContentType::RichText,
            "image" => ContentType::Image,
            "file" => ContentType::File,
            // Legacy: link/path/color migrated to plain_text + meta_type in v4
            "link" | "path" | "color" => ContentType::PlainText,
            other => {
                log::warn!("Unknown content_type in DB: {other:?}, falling back to plain_text");
                ContentType::PlainText
            }
        }
    }
}

/// Unified content display classification.
///
/// Single source of truth for UI components to decide how to render or edit
/// clipboard content. Derived from `content_type`, `meta_type`, and `rich_data`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayKind {
    Html,
    Markdown,
    Rtf,
    Email,
    Phone,
    Link,
    Path,
    Color,
    File,
    Image,
    PlainText,
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
    #[serde(default)]
    pub uid: String,
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

/// Paste format preference for custom hotkeys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum HotkeyPasteFormat {
    #[default]
    Default,
    PlainText,
    ImageBitmap,
    ImagePath,
    OcrText,
    FilePath,
    Rgb,
    Hex,
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
    pub meta_type: String, // subtype: "" | "email" | "phone" | "markdown" | "html" | "link" | "path" | "color"
    pub custom_hotkey: String,
    pub custom_hotkey_format: String,
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
    /// Page title fetched from the URL target (only for link-type items).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_title: Option<String>,
    /// Drive / volume label for file-system paths (e.g. "配置(D:)").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drive_label: Option<String>,
}

impl RichData {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("RichData should always serialize")
    }

    pub fn from_json(s: &str) -> Self {
        // Empty rich_data is the normal state for plain-text/image/file items.
        // Skip deserialization silently instead of logging a warning on every call.
        if s.trim().is_empty() {
            return Self::default();
        }
        serde_json::from_str(s).unwrap_or_else(|e| {
            log::warn!("Failed to deserialize RichData: {e}");
            Self::default()
        })
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
        serde_json::to_string(self).expect("FileData should always serialize")
    }

    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_else(|e| {
            log::warn!("Failed to deserialize FileData: {e}");
            Self::default()
        })
    }

    pub fn display_text(&self) -> String {
        self.files
            .iter()
            .map(|f| f.name.clone())
            .collect::<Vec<_>>()
            .join("\n")
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
            custom_hotkey: String::new(),
            custom_hotkey_format: String::new(),
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
            custom_hotkey: String::new(),
            custom_hotkey_format: String::new(),
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
            custom_hotkey: String::new(),
            custom_hotkey_format: String::new(),
        }
    }

    /// Unified content classification for UI rendering decisions.
    ///
    /// Single source of truth — card preview, type tag, and edit panel all
    /// use this method to determine how to display or edit content.
    pub fn display_kind(&self) -> DisplayKind {
        // ── meta_type takes priority (explicit subtype from detection) ──
        match self.meta_type.as_str() {
            "markdown" => return DisplayKind::Markdown,
            "html" => return DisplayKind::Html,
            "email" => return DisplayKind::Email,
            "phone" => return DisplayKind::Phone,
            "link" => return DisplayKind::Link,
            "path" => return DisplayKind::Path,
            "color" => return DisplayKind::Color,
            _ => {}
        }
        // ── Fall back to content_type + rich_data inspection ──
        match self.content_type {
            ContentType::RichText => {
                let rich = RichData::from_json(&self.rich_data);
                if rich
                    .html
                    .as_deref()
                    .is_some_and(|html| !html.trim().is_empty())
                {
                    DisplayKind::Html
                } else if rich
                    .rtf
                    .as_deref()
                    .is_some_and(|rtf| !rtf.trim().is_empty())
                {
                    DisplayKind::Rtf
                } else if is_markdown_like(&self.full_text) {
                    DisplayKind::Markdown
                } else {
                    DisplayKind::PlainText
                }
            }
            ContentType::File => DisplayKind::File,
            ContentType::Image => DisplayKind::Image,
            _ => DisplayKind::PlainText,
        }
    }
}

/// Format elapsed time as human-readable string
pub fn format_relative_time(captured_at: &DateTime<Utc>) -> String {
    let elapsed = Utc::now().signed_duration_since(*captured_at);
    let secs = elapsed.num_seconds();
    use crate::core::i18n_keys::I18nKey;

    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;

    if secs < MINUTE {
        I18nKey::FormatJustNow.text().to_string()
    } else if secs < HOUR {
        I18nKey::FormatMinutesAgo.fmt(&[&(secs / MINUTE).to_string()])
    } else if secs < DAY {
        I18nKey::FormatHoursAgo.fmt(&[&(secs / HOUR).to_string()])
    } else if secs < WEEK {
        I18nKey::FormatDaysAgo.fmt(&[&(secs / DAY).to_string()])
    } else if secs < MONTH {
        I18nKey::FormatWeeksAgo.fmt(&[&(secs / WEEK).to_string()])
    } else if secs < YEAR {
        I18nKey::FormatMonthsAgo.fmt(&[&(secs / MONTH).to_string()])
    } else {
        I18nKey::FormatYearsAgo.fmt(&[&(secs / YEAR).to_string()])
    }
}

/// Check if text is solely a web URL.
///
/// Recognises full URLs (`http://` / `https://`) as well as protocol-less URLs
/// in the `domain.tld/path` form (e.g. `pic.ghxi.com/roadmap`).  Protocol-less
/// detection requires a TLD that is at least two ASCII letters so that IP
/// addresses and version-like tokens are not mistaken for URLs.
pub fn is_url(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    if text.contains('\n') || text.contains(' ') {
        return false;
    }
    // ── Full URL with scheme ──────────────────────────────────────────
    if (text.starts_with("http://") || text.starts_with("https://")) && text.len() > 10 {
        return true;
    }
    // ── Protocol-less URL: domain.tld/path ────────────────────────────
    // Must have a '/' separating the domain from the path, the domain must
    // contain at least one dot, and the TLD (after the last dot) must be
    // purely alphabetic with a minimum length of 2.
    if let Some(slash_pos) = text.find('/') {
        let domain = &text[..slash_pos];
        // Domain must be at least 4 chars (e.g. "a.co"), contain a dot,
        // and NOT start with a dot (rejects "../file", "./script.sh").
        if domain.len() >= 4 && domain.contains('.') && !domain.starts_with('.') {
            if let Some(last_dot) = domain.rfind('.') {
                let tld = &domain[last_dot + 1..];
                if tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()) {
                    return true;
                }
            }
        }
    }
    false
}

/// Check if text is solely an email address.
pub fn is_email(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.contains('\n') || text.contains(' ') {
        return false;
    }
    // --- Must contain exactly one @ not at start or end ---
    let at_pos = match text.find('@') {
        Some(p) if p > 0 && p < text.len() - 1 => p,
        _ => return false,
    };
    // --- Local part: alphanumeric + limited special chars, no consecutive dots ---
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
    // --- Domain part: must have a dot, no consecutive dots, valid chars ---
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
    // --- TLD must start with a letter and be at least 2 chars ---
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
    // --- Strip common separators ---
    let cleaned: String = text
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '(' | ')' | '\t'))
        .collect();
    // --- Chinese mobile: 1[3-9]xxxxxxxxx (11 digits, starts with 1) ---
    if cleaned.len() == 11
        && cleaned.starts_with('1')
        && cleaned.chars().all(|c| c.is_ascii_digit())
        && cleaned.as_bytes()[1] >= b'3'
        && cleaned.as_bytes()[1] <= b'9'
    {
        return true;
    }
    // --- International: +country region number (7-15 digit phone body) ---
    if cleaned.starts_with('+')
        && cleaned.len() >= 10
        && cleaned.len() <= 17
        && cleaned[1..].chars().all(|c| c.is_ascii_digit())
    {
        return true;
    }
    false
}

/// Check if plain text uses common Markdown structure.
///
/// This is intentionally conservative so ordinary prose with punctuation does
/// not get promoted to rich text by accident.
pub fn is_markdown_like(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 3 {
        return false;
    }

    trimmed.contains("```")
        || trimmed.contains("**")
        || trimmed.contains("__")
        || (trimmed.contains('[') && trimmed.contains("]("))
        || trimmed.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("# ")
                || line.starts_with("## ")
                || line.starts_with("### ")
                || line.starts_with("- ")
                || line.starts_with("* ")
                || line.starts_with("> ")
                || line.starts_with("1. ")
        })
}

/// Check if text is solely a file system path (Windows absolute, UNC, or Unix absolute).
/// Rejects text that starts with a path but has extra descriptive content after it.
pub fn is_path(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.contains('\n') {
        return false;
    }

    // Fast path: known prefixes bypass the generic space heuristic (they are
    // unambiguously paths). The space heuristic still applies to unrecognised
    // prefixes to avoid treating prose with slashes as a path.
    let is_known_prefix = text.len() >= 3
        && text.as_bytes()[0].is_ascii_alphabetic()
        && text.as_bytes()[1] == b':'
        && (text.as_bytes()[2] == b'\\' || text.as_bytes()[2] == b'/')  // C:\ or D:/
        || text.starts_with("\\\\") && text.len() > 2                    // \\server
        || text.starts_with('/')
            && text.len() >= 3
            && text.as_bytes()[1] != b'/'
            && text[1..].contains('/')                                   // /abs/path
        || looks_like_ipv4_path(text); // 192.168.x.x\…

    if !is_known_prefix {
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
    }

    // --- Windows absolute path: C:\..., D:/... ---
    if text.len() >= 3
        && text.as_bytes()[0].is_ascii_alphabetic()
        && text.as_bytes()[1] == b':'
        && (text.as_bytes()[2] == b'\\' || text.as_bytes()[2] == b'/')
    {
        return true;
    }
    // --- UNC network path: \\server\share\... or \\192.168.1.1\... ---
    if text.starts_with("\\\\") && text.len() > 2 {
        return true;
    }
    // --- IP-address-based network path: 192.168.1.1\share\... ---
    if looks_like_ipv4_path(text) {
        return true;
    }
    // --- Unix absolute path: /Users/..., /etc/..., /tmp/... ---
    // Require at least one "/" after the first char to avoid matching
    // --- slash commands like /clear, /help, etc. ---
    if text.starts_with('/')
        && text.len() >= 3
        && text.as_bytes()[1] != b'/'
        && text[1..].contains('/')
    {
        return true;
    }
    false
}

/// Returns `true` when `text` starts with an IPv4 address immediately followed
/// by a backslash or forward slash (e.g. `192.168.1.1\share`).
fn looks_like_ipv4_path(text: &str) -> bool {
    // Find the first path separator.
    let sep_pos = match text.find('\\').or_else(|| text.find('/')) {
        Some(p) if p > 6 => p, // shortest IPv4: "1.1.1.1" = 7 chars
        _ => return false,
    };
    let ip = &text[..sep_pos];
    let mut octets = ip.split('.');
    let mut count = 0;
    for octet in octets.by_ref() {
        if count >= 4 {
            return false; // too many octets
        }
        // Each octet must be 1-3 digits and in the range 0-255.
        if octet.is_empty() || octet.len() > 3 || !octet.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        let val: u32 = match octet.parse() {
            Ok(v) => v,
            Err(_) => return false,
        };
        if val > 255 {
            return false;
        }
        count += 1;
    }
    // Exactly 4 octets and a trailing path separator.
    count == 4
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

/// Extract a human-readable site name from a URL.
///
/// Strips `www.`, takes the second-level domain (the segment before the
/// TLD), and capitalises the first letter.
///
/// ```
/// # use clippi::core::types::url_site_name;
/// assert_eq!(url_site_name("https://www.github.com/user"), "Github");
/// assert_eq!(url_site_name("github.com/repo"), "Github");
/// assert_eq!(url_site_name("https://docs.rs/clippi"), "Docs");
/// ```
pub fn url_site_name(text: &str) -> String {
    let domain = url_domain(text);
    let domain = domain.strip_prefix("www.").unwrap_or(&domain);
    let parts: Vec<&str> = domain.split('.').collect();
    let name = if parts.len() >= 2 {
        parts[parts.len() - 2] // segment just before TLD
    } else {
        domain
    };
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => {
            let rest: String = chars.as_str().to_lowercase();
            format!("{}{}", first.to_uppercase(), rest)
        }
        None => String::new(),
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

/// Extract the drive root from a file-system path.
///
/// Windows: `C:\foo\bar` → `Some("C:")`
/// UNC:     `\\server\share\dir` → `Some("\\server")`
/// Unix:    `/home/user` → `None`
pub fn path_drive_root(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // ── UNC path: \\server\... → "\\server" ──
    if let Some(rest) = text.strip_prefix("\\\\") {
        match rest.find('\\') {
            Some(pos) => Some(format!("\\\\{}", &rest[..pos])),
            None => Some(text.to_string()), // entire thing is server name
        }
    }
    // ── Windows drive letter: C:\... or D:/... → "C:" ──
    else if text.len() >= 2
        && text.as_bytes()[0].is_ascii_alphabetic()
        && text.as_bytes()[1] == b':'
    {
        Some(text[..2].to_string())
    }
    // ── Unix / IP-based: no obvious drive root ──
    else {
        None
    }
}

/// Get the volume name for a Windows drive root (e.g. "C:" → "系统").
/// Returns empty string on failure or non-Windows platforms.
fn volume_name(drive_root: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        // drive_root is e.g. "C:" — need "C:\" for GetVolumeInformationW
        let root_path = format!("{}\\", drive_root);
        let root_utf16: Vec<u16> = root_path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut name_buf = [0u16; 128]; // MAX_PATH + 1
        unsafe {
            let ok = windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW(
                root_utf16.as_ptr(),
                name_buf.as_mut_ptr(),
                name_buf.len() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            );
            if ok != 0 {
                let end = name_buf
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(name_buf.len());
                return String::from_utf16_lossy(&name_buf[..end]);
            }
        }
        String::new()
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = drive_root;
        String::new()
    }
}

/// Build a human-readable drive label for a file-system path.
///
/// Returns `"配置(D:)"` when the volume name is available,
/// `"D:"` as a fallback, or `None` if the path has no drive root.
pub fn path_drive_label(text: &str) -> Option<String> {
    let root = path_drive_root(text)?;
    let name = volume_name(&root);
    // UNC paths: strip leading \\ for display (user doesn't need to see it).
    let display_root = root.strip_prefix("\\\\").unwrap_or(&root);
    let label = if name.is_empty() {
        display_root.to_uppercase()
    } else {
        format!("{}({})", name, display_root.to_uppercase())
    };
    Some(label)
}

/// Whether a file-system path matches the *current* platform's native format.
///
/// Used to hide jump buttons for foreign paths (they can't be opened) and
/// optionally filter them from the list.
pub fn path_is_native(text: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        let text = text.trim();
        // Windows drive letter: C:\...
        (text.len() >= 3
            && text.as_bytes()[0].is_ascii_alphabetic()
            && text.as_bytes()[1] == b':'
            && (text.as_bytes()[2] == b'\\' || text.as_bytes()[2] == b'/'))
        // UNC path: \\server\...
        || text.starts_with("\\\\")
        // IP-based path: 192.168.x.x\...
        || looks_like_ipv4_path(text)
    }
    #[cfg(target_os = "macos")]
    {
        // macOS native: Unix absolute paths (/Users/...), exclude //
        let text = text.trim();
        text.starts_with('/') && !text.starts_with("//")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = text;
        true // Linux: accept all paths
    }
}

/// Check whether a path still exists on disk.
/// UNC paths (starting with `\\\\`) skip the check and always return `true`
/// to avoid network delays.
pub fn path_exists(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    // UNC network paths — skip existence check
    if text.starts_with("\\\\") {
        return true;
    }
    std::path::Path::new(text).exists()
}

/// Mask sensitive content for preview display.
/// Email: show first 3 chars of local part + "****" + domain (e.g. "abc****@gmail.com")
/// Phone: show first 3 chars + "****" + last 4 chars (e.g. "138****5678")
pub fn mask_sensitive_preview(text: &str, meta_type: &str) -> String {
    match meta_type {
        "email" => {
            if let Some(at) = text.find('@') {
                let local = &text[..at];
                let domain = &text[at..];
                let visible = local.chars().take(3).collect::<String>();
                format!("{}****{}", visible, domain)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_relative_time_supports_months_and_years() {
        let month_old = Utc::now() - chrono::Duration::days(45);
        let year_old = Utc::now() - chrono::Duration::days(400);

        assert_eq!(format_relative_time(&month_old), "1月前");
        assert_eq!(format_relative_time(&year_old), "1年前");
    }

    // ── is_url ──────────────────────────────────────────────────────────

    #[test]
    fn test_is_url_full_https() {
        assert!(is_url("https://pic.ghxi.com/roadmap"));
        assert!(is_url("https://github.com/user/repo"));
        assert!(is_url("http://example.com/path?q=1"));
    }

    #[test]
    fn test_is_url_protocol_less() {
        assert!(is_url("pic.ghxi.com/roadmap"));
        assert!(is_url("github.com/user/repo"));
        assert!(is_url("example.co/path"));
    }

    #[test]
    fn test_is_url_rejects_non_urls() {
        assert!(!is_url("just text"));
        assert!(!is_url("not.a.url")); // no slash after TLD
        assert!(!is_url("../relative/path")); // starts with dot
        assert!(!is_url("./script.sh")); // starts with dot
        assert!(!is_url("192.168.1.1/share")); // IP: TLD is numeric
        assert!(!is_url("v1.2.3/file")); // version-like, no alpha TLD
        assert!(!is_url("C:\\Windows\\System32")); // Windows path
        assert!(!is_url("")); // empty
        assert!(!is_url("https://x")); // too short: 9 chars, len > 10 fails
    }

    #[test]
    fn test_is_url_rejects_text_with_spaces() {
        assert!(!is_url("https://example.com with description"));
        assert!(!is_url("pic.ghxi.com /roadmap")); // space in domain
        assert!(is_url("pic.ghxi.com/roadmap ")); // trailing space trimmed
        assert!(is_url(" https://example.com/path")); // leading space trimmed
    }

    // ── is_path ─────────────────────────────────────────────────────────

    #[test]
    fn test_is_path_windows_drive() {
        assert!(is_path("C:\\Windows\\System32"));
        assert!(is_path("D:/Projects/rust"));
        assert!(is_path("E:\\folder\\file.txt"));
    }

    #[test]
    fn test_is_path_unc() {
        assert!(is_path("\\\\server\\share\\folder"));
        assert!(is_path("\\\\192.168.1.1\\data"));
    }

    #[test]
    fn test_is_path_ipv4() {
        assert!(is_path("192.168.18.222\\mssv_share\\folder"));
        assert!(is_path("10.0.0.1/data/path"));
        assert!(is_path("172.16.254.1\\share"));
    }

    #[test]
    fn test_is_path_unix_absolute() {
        assert!(is_path("/Users/john/Documents"));
        assert!(is_path("/etc/nginx/nginx.conf"));
        assert!(is_path("/tmp/build/output"));
    }

    #[test]
    fn test_is_path_rejects_non_paths() {
        assert!(!is_path("not a path"));
        assert!(!is_path("/")); // root only
        assert!(!is_path("/clear")); // slash command
        assert!(!is_path("")); // empty
        assert!(!is_path("/a")); // too short (len < 3)
        assert!(!is_path("//comment")); // double-slash without share
    }

    #[test]
    fn test_is_path_rejects_invalid_ipv4() {
        assert!(!is_path("256.1.1.1\\share")); // octet > 255
        assert!(!is_path("1.2.3.4")); // no path separator
        assert!(!is_path("1.2.3\\share")); // only 3 octets
        assert!(!is_path("abc.def.ghi.jkl\\s")); // non-numeric octets
    }

    #[test]
    fn test_is_path_ipv4_with_spaces() {
        // IP path prefix bypasses the generic space heuristic.
        assert!(is_path("192.168.18.222\\mssv_各组协同\\2025"));
    }

    // ── looks_like_ipv4_path ────────────────────────────────────────────

    #[test]
    fn test_looks_like_ipv4_path_valid() {
        assert!(looks_like_ipv4_path("192.168.1.1\\share"));
        assert!(looks_like_ipv4_path("10.0.0.1/data"));
        assert!(looks_like_ipv4_path("172.16.254.1\\"));
    }

    #[test]
    fn test_looks_like_ipv4_path_invalid() {
        assert!(!looks_like_ipv4_path("192.168.1.1")); // no separator
        assert!(!looks_like_ipv4_path("1.1.1")); // not IP
        assert!(!looks_like_ipv4_path("999.1.1.1\\share")); // octet > 255
        assert!(!looks_like_ipv4_path("1.2.3.4.5\\share")); // 5 octets
    }
}
