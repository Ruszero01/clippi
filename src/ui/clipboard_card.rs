//! Clipboard card — renders a single clipboard entry.
//!
//! Matches the original Slint ClipboardList.slint card design:
//! - 10px border-radius, surface bg, 1px border, drop shadow
//! - Content type icon area (left, 36-38px) + content area (right, flex)
//! - Fav indicator bar (left edge, 3px, fav-color, scales with card height)
//! - Selection badge (top-left, 12x12, accent bg)
//! - Bottom info row: tag pills + time label (9px font, 18px pills)
//! - Dynamic height: 68/96/128px based on content type and card-height-mode

use std::rc::Rc;
use std::sync::Mutex;

use base64::Engine;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::text::{TextView, TextViewStyle};

use crate::core::color::detect_color;
use crate::core::frontend::PANEL_OFFSET_X;
use crate::core::html_text;
use crate::core::i18n_keys::I18nKey;
use crate::core::transfer_types::{
    TRANSFER_STATUS_CLOUD_UID, TRANSFER_STATUS_DOWNLOADING_UID, TRANSFER_STATUS_LOCAL_UID,
};
use crate::core::types::{
    format_relative_time, parse_hex_color, url_domain, url_path, url_site_name, ClipboardItem,
    ContentType, DisplayKind, FileData, FileInfo, HotkeyPasteFormat, RichData,
};

use super::components::sensitive_text::SensitiveText;
use super::components::spinner::activity_spinner;
use super::hover_toolbar::{HoverToolbar, HoverToolbarProps};
use super::rich_preview::{self, StyledHtmlSpan};
use super::search_highlight;
use super::theme::ClippiTheme;

type CardClickHandler = Rc<dyn Fn(usize, Modifiers, &mut Window, &mut App)>;
type CardIndexHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;
type CardActionHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;
type CardWindowHandler = Rc<dyn Fn(&mut Window, &mut App)>;
type CardFormatHandler = Rc<dyn Fn(HotkeyPasteFormat, &mut Window, &mut App)>;

const INFO_PILL_FONT_SIZE: f32 = 9.0;
const INFO_PILL_GAP: f32 = 4.0;
const INFO_PILL_BORDER_WIDTH: f32 = 1.0;
const INFO_PILL_PADDING_X: f32 = 5.0;
const HOTKEY_PILL_ICON_WIDTH: f32 = 9.0;
const HOTKEY_PILL_ICON_GAP: f32 = 3.0;
const TIME_PILL_PADDING_X: f32 = 7.0;
const PANEL_BORDER_WIDTH: f32 = 1.0;
const LIST_PADDING_X: f32 = 8.0;
const CARD_PADDING_X: f32 = 10.0;
const CARD_ICON_WIDTH: f32 = 36.0;
const CARD_CONTENT_GAP: f32 = 10.0;
const MAX_ICON_CACHE_JOBS: usize = 4;

#[derive(Clone, Copy)]
enum TransferPillKind {
    Cloud,
    Local,
    Downloading,
}

static ICON_CACHE_JOBS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn try_start_icon_cache_job(key: &str) -> bool {
    let mut jobs = ICON_CACHE_JOBS.lock().unwrap_or_else(|e| e.into_inner());
    if jobs.iter().any(|job| job == key) || jobs.len() >= MAX_ICON_CACHE_JOBS {
        return false;
    }
    jobs.push(key.to_string());
    true
}

fn finish_icon_cache_job(key: &str) {
    ICON_CACHE_JOBS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|job| job != key);
}

fn info_pill_width(text: &str, padding_x: f32, window: &Window) -> f32 {
    let text: SharedString = text.to_owned().into();
    let run = window.text_style().to_run(text.len());
    let line = window
        .text_system()
        .shape_line(text, px(INFO_PILL_FONT_SIZE), &[run], None);

    f32::from(line.width).ceil() + padding_x * 2.0 + INFO_PILL_BORDER_WIDTH * 2.0
}

fn hotkey_pill_width(hotkey: &str, window: &Window) -> f32 {
    info_pill_width(hotkey, INFO_PILL_PADDING_X, window)
        + HOTKEY_PILL_ICON_WIDTH
        + HOTKEY_PILL_ICON_GAP
}

fn info_row_width(pill_width_sum: f32, pill_count: usize) -> f32 {
    pill_width_sum + INFO_PILL_GAP * pill_count.saturating_sub(1) as f32
}

fn visible_tag_count(
    tag_widths: &[f32],
    fixed_widths: &[f32],
    available_width: f32,
    mut overflow_width: impl FnMut(usize) -> f32,
) -> usize {
    let fixed_width_sum = fixed_widths.iter().sum::<f32>();
    let all_tag_width_sum = tag_widths.iter().sum::<f32>();
    let all_pill_count = fixed_widths.len() + tag_widths.len();

    if info_row_width(fixed_width_sum + all_tag_width_sum, all_pill_count) <= available_width {
        return tag_widths.len();
    }

    for visible_count in (0..tag_widths.len()).rev() {
        let hidden_count = tag_widths.len() - visible_count;
        let visible_width_sum = tag_widths[..visible_count].iter().sum::<f32>();
        let pill_count = fixed_widths.len() + visible_count + 1;
        let width = info_row_width(
            fixed_width_sum + visible_width_sum + overflow_width(hidden_count),
            pill_count,
        );
        if width <= available_width {
            return visible_count;
        }
    }

    0
}

fn info_row_available_width(window: &Window) -> f32 {
    let card_width = f32::from(window.viewport_size().width)
        - PANEL_OFFSET_X
        - PANEL_BORDER_WIDTH * 2.0
        - LIST_PADDING_X * 2.0;
    let content_left = CARD_PADDING_X + CARD_ICON_WIDTH + CARD_CONTENT_GAP;
    (card_width - content_left - CARD_PADDING_X).max(0.0)
}

#[cfg(test)]
mod info_row_tests {
    use super::visible_tag_count;

    #[test]
    fn shows_all_tags_when_they_fit() {
        let tags = [30.0, 40.0];
        let fixed = [50.0, 60.0];

        assert_eq!(
            visible_tag_count(&tags, &fixed, 192.0, |_| 24.0),
            tags.len()
        );
    }

    #[test]
    fn reserves_space_for_the_hidden_tag_count() {
        let tags = [30.0, 30.0, 30.0];
        let fixed = [40.0, 50.0];
        let visible = visible_tag_count(&tags, &fixed, 156.0, |_| 24.0);

        assert_eq!(visible, 1);
        assert_eq!(tags.len() - visible, 2);
    }

    #[test]
    fn keeps_fixed_pills_and_count_when_no_custom_tag_fits() {
        let tags = [30.0, 30.0];
        let fixed = [40.0, 50.0];

        assert_eq!(visible_tag_count(&tags, &fixed, 118.0, |_| 20.0), 0);
    }
}

/// Get a content type iconfont glyph for display.
fn type_icon(item: &ClipboardItem) -> &'static str {
    // Use meta-type specific icons for email and phone
    if item.meta_type == "email" {
        return "\u{e604}";
    }
    if item.meta_type == "phone" {
        return "\u{e966}";
    }
    if has_qr_code(item) {
        return "\u{e605}";
    }
    match item.display_kind() {
        DisplayKind::PlainText => "\u{e60e}",
        DisplayKind::Html | DisplayKind::Markdown | DisplayKind::Rtf => "\u{e6ae}",
        DisplayKind::Image => "\u{e626}",
        DisplayKind::File => "\u{e68a}",
        DisplayKind::Link => "\u{e6d7}",
        DisplayKind::Path => "\u{e60f}",
        DisplayKind::Color => "\u{e610}",
        DisplayKind::Email => "\u{e604}",
        DisplayKind::Phone => "\u{e966}",
        DisplayKind::Secret => "\u{e60e}",
    }
}

/// Get a content type display label.
fn type_label(item: &ClipboardItem) -> String {
    use crate::core::types::DisplayKind;

    // QR code is a special sub-type detected from rich_data, not the main kind
    if has_qr_code(item) {
        return I18nKey::CardTypeQr.text().into();
    }

    match item.display_kind() {
        DisplayKind::Html => I18nKey::CardTypeHtml.text().into(),
        DisplayKind::Markdown => I18nKey::CardTypeMd.text().into(),
        DisplayKind::Rtf => I18nKey::CardTypeRtf.text().into(),
        DisplayKind::Email => I18nKey::CardTypeEmail.text().into(),
        DisplayKind::Phone => I18nKey::CardTypePhone.text().into(),
        DisplayKind::Link => I18nKey::CardTypeUrl.text().into(),
        DisplayKind::Path => I18nKey::CardTypePath.text().into(),
        DisplayKind::Color => I18nKey::CardTypeColor.text().into(),
        DisplayKind::Secret => I18nKey::CardTypeSecret.text().into(),
        DisplayKind::File => {
            let fd: FileData = serde_json::from_str(&item.file_data).unwrap_or_default();
            let is_dir = fd.files.first().is_some_and(|f| f.is_dir);
            if fd.files.len() == 1 && is_dir {
                I18nKey::CardTypeFolder.text().into()
            } else {
                I18nKey::CardTypeFile.text().into()
            }
        }
        DisplayKind::Image => I18nKey::CardTypeImage.text().into(),
        DisplayKind::PlainText => I18nKey::CardTypeText.text().into(),
    }
}

/// Label shown on foreign-platform path items (e.g. "Mac" on Windows).
#[cfg(target_os = "windows")]
fn foreign_path_label() -> String {
    "Mac".to_string()
}
#[cfg(target_os = "macos")]
fn foreign_path_label() -> String {
    "Windows".to_string()
}
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn foreign_path_label() -> String {
    I18nKey::CardCrossPlatform.text().to_string()
}

fn has_qr_code(item: &ClipboardItem) -> bool {
    item.content_type == ContentType::Image
        && RichData::from_json(&item.rich_data).qr_text.is_some()
}

fn color_from_hex(hex: &str, fallback: Rgba) -> Rgba {
    parse_hex_color(hex)
        .map(|(r, g, b)| rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32))
        .unwrap_or(fallback)
}

/// Split a filename into stem and extension using OS path parsing.
/// Handles multi-dot names correctly: "archive.tar.gz" → ("archive.tar", ".gz")
fn split_name_ext(filename: &str) -> (String, String) {
    use std::path::Path;
    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let ext_with_dot = if ext.is_empty() {
        String::new()
    } else {
        format!(".{}", ext)
    };
    (stem.to_string(), ext_with_dot)
}

fn swatch_color(text: &str, fallback: Rgba) -> Rgba {
    detect_color(text)
        .map(|color| rgb(((color.r as u32) << 16) | ((color.g as u32) << 8) | color.b as u32))
        .unwrap_or(fallback)
}

fn source_icon_path(item: &ClipboardItem) -> Option<std::path::PathBuf> {
    crate::core::paths::cache_app_icon(&item.source_app_name, &item.source_app_icon)
}

fn image_preview_path(item: &ClipboardItem) -> Option<std::path::PathBuf> {
    if let Some(thumb) = crate::platform::clipboard::image_thumbnail_path(item.content_hash) {
        return Some(thumb);
    }
    if !item.image_path.is_empty() {
        crate::platform::clipboard::ensure_thumbnail_for_image(&item.image_path, item.content_hash);
    }
    None
}

fn image_display_name(item: &ClipboardItem) -> String {
    std::path::Path::new(&item.image_path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            std::path::Path::new(&item.full_text)
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
        })
        .unwrap_or("Image")
        .to_string()
}

fn image_path_is_cache_path(image_path: &str) -> bool {
    !image_path.is_empty()
        && std::path::Path::new(image_path).starts_with(crate::core::paths::images_dir())
}

/// Get a cached file system icon for a given file path.
/// Icons are cached by extension (or "folder" for dirs) in `images_dir()/file_icons/`.
/// Check if a favicon is cached for a URL's domain.
/// Only checks the local cache — no network fetch during rendering.
/// Format a file size in bytes to a human-readable string.
fn format_file_size(bytes: i64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Display label for non-default paste formats attached to a custom hotkey.
/// Uses `DefaultHasher` with fixed keys — deterministic within the same
/// binary, sufficient for a persistent on-disk cache.
fn parse_hotkey_paste_format(format: &str) -> Option<HotkeyPasteFormat> {
    if format.is_empty() {
        return Some(HotkeyPasteFormat::Default);
    }
    serde_json::from_str(format).ok().or(match format {
        "Default" => Some(HotkeyPasteFormat::Default),
        "PlainText" => Some(HotkeyPasteFormat::PlainText),
        "ImageBitmap" => Some(HotkeyPasteFormat::ImageBitmap),
        "ImagePath" => Some(HotkeyPasteFormat::ImagePath),
        "OcrText" => Some(HotkeyPasteFormat::OcrText),
        "FilePath" => Some(HotkeyPasteFormat::FilePath),
        "Rgb" => Some(HotkeyPasteFormat::Rgb),
        "Hex" => Some(HotkeyPasteFormat::Hex),
        _ => None,
    })
}

fn hotkey_paste_format_label(format: HotkeyPasteFormat) -> SharedString {
    match format {
        HotkeyPasteFormat::Default => I18nKey::HotkeyFormatDefault.text().into(),
        HotkeyPasteFormat::PlainText => I18nKey::HotkeyFormatPlainText.text().into(),
        HotkeyPasteFormat::ImageBitmap => I18nKey::HotkeyFormatBitmap.text().into(),
        HotkeyPasteFormat::ImagePath => I18nKey::HotkeyFormatImagePath.text().into(),
        HotkeyPasteFormat::OcrText => I18nKey::HotkeyFormatOcr.text().into(),
        HotkeyPasteFormat::FilePath => I18nKey::HotkeyFormatFilePath.text().into(),
        HotkeyPasteFormat::Rgb => I18nKey::HotkeyFormatRgb.text().into(),
        HotkeyPasteFormat::Hex => I18nKey::HotkeyFormatHex.text().into(),
    }
}

fn hotkey_paste_formats_for_item(item: &ClipboardItem) -> Vec<HotkeyPasteFormat> {
    let mut formats = vec![HotkeyPasteFormat::Default];

    match item.content_type {
        ContentType::Image => {
            formats.push(HotkeyPasteFormat::ImageBitmap);
            formats.push(HotkeyPasteFormat::ImagePath);
            if !item.image_path.is_empty() {
                formats.push(HotkeyPasteFormat::OcrText);
            }
        }
        ContentType::File => {
            formats.push(HotkeyPasteFormat::FilePath);
        }
        ContentType::PlainText | ContentType::RichText => {
            if item.meta_type == "color" {
                formats.push(HotkeyPasteFormat::Rgb);
                formats.push(HotkeyPasteFormat::Hex);
            } else if matches!(
                item.display_kind(),
                DisplayKind::Html | DisplayKind::Markdown | DisplayKind::Rtf
            ) {
                formats.push(HotkeyPasteFormat::PlainText);
            }
        }
    }

    formats
}

fn normalized_hotkey_paste_format(format: &str, item: &ClipboardItem) -> HotkeyPasteFormat {
    let selected = parse_hotkey_paste_format(format).unwrap_or_default();
    if hotkey_paste_formats_for_item(item).contains(&selected) {
        selected
    } else {
        HotkeyPasteFormat::Default
    }
}

/// Produce a stable hex hash of a file path for per-file icon cache keys.
fn hash_file_path(path: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub(crate) fn cached_file_icon_path(file_path: &str, is_dir: bool) -> Option<std::path::PathBuf> {
    use std::path::Path;

    // Determine the cache key.
    let ext_lower = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_else(|| "file".to_string());

    // Per-file cache for extensions that carry unique embedded icons.
    // Do not probe source path existence here: copied paths can point to
    // network/removable volumes, and synchronous metadata checks in render
    // can stall the UI. The background job falls back to a generic icon if
    // the actual file icon cannot be read.
    let (cache_key, use_actual_icon) =
        if crate::platform::source::extension_has_embedded_icon(&ext_lower) {
            let path_hash = hash_file_path(file_path);
            (format!("{ext_lower}_{path_hash}"), true)
        } else if is_dir {
            ("folder".to_string(), false)
        } else {
            (ext_lower, false)
        };

    // File icon directory is pre-created at startup — no fs ops in render.
    let icon_path = crate::core::paths::images_dir()
        .join("file_icons")
        .join(format!("{cache_key}.png"));

    if !icon_path.exists() {
        let job_key = format!("file:{cache_key}");
        if !try_start_icon_cache_job(&job_key) {
            return None;
        }
        // Cache miss: heavy Win32 SHGetFileInfoW → spawn to background,
        // skip icon this frame. Next render hits the cache.
        let fp = file_path.to_string();
        let p = icon_path.clone();
        let finish_key = job_key.clone();
        std::thread::spawn(move || {
            // Prefer the actual embedded icon when the file type warrants
            // per-file caching; fall back to the extension-based icon if
            // the file was deleted between the exists() check and now.
            let icon_base64 = if use_actual_icon {
                crate::platform::source::get_actual_file_icon_base64(&fp)
                    .or_else(|| crate::platform::source::get_file_icon_base64(&fp, is_dir))
            } else {
                crate::platform::source::get_file_icon_base64(&fp, is_dir)
            };

            if let Some(icon_base64) = icon_base64 {
                if let Ok(png) = base64::engine::general_purpose::STANDARD.decode(&icon_base64) {
                    let _ = std::fs::write(&p, png);
                }
            }
            finish_icon_cache_job(&finish_key);
        });
        return None;
    }
    Some(icon_path)
}

enum RichPreview {
    StyledHtml {
        lines: Vec<Vec<StyledHtmlSpan>>,
        visible_text: String,
    },
    Html(String),
    Markdown(String),
    Plain(String),
}

fn rich_preview(item: &ClipboardItem) -> RichPreview {
    use crate::core::types::DisplayKind;

    match item.display_kind() {
        DisplayKind::Html => {
            let rich = RichData::from_json(&item.rich_data);
            let html = rich.html.unwrap_or_else(|| item.full_text.clone());
            let html = rich_preview::normalize_clipboard_html_for_render(&html);
            if let Some(lines) = rich_preview::parse_styled_html_lines(&html) {
                return RichPreview::StyledHtml {
                    lines,
                    visible_text: html_text::visible_text(&html),
                };
            }
            RichPreview::Html(rich_preview::strip_html_links(&html))
        }
        DisplayKind::Markdown => {
            RichPreview::Markdown(rich_preview::strip_markdown_links(&item.full_text))
        }
        DisplayKind::Rtf => {
            let rich = RichData::from_json(&item.rich_data);
            if let Some(rtf) = rich.rtf.filter(|r| !r.trim().is_empty()) {
                RichPreview::Markdown(rtf_to_plain_text(&rtf))
            } else {
                RichPreview::Plain(item.full_text.clone())
            }
        }
        _ => RichPreview::Plain(item.full_text.clone()),
    }
}

fn rtf_to_plain_text(rtf: &str) -> String {
    let mut out = String::new();
    let mut chars = rtf.chars().peekable();
    let mut group_depth_to_skip: Option<usize> = None;
    let mut depth = 0usize;

    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                depth += 1;
                if matches!(chars.peek(), Some('\\')) {
                    let mut probe = chars.clone();
                    let _ = probe.next();
                    if matches!(probe.peek(), Some('*')) {
                        group_depth_to_skip = Some(depth);
                    }
                }
            }
            '}' => {
                if group_depth_to_skip == Some(depth) {
                    group_depth_to_skip = None;
                }
                depth = depth.saturating_sub(1);
            }
            '\\' if group_depth_to_skip.is_none() => match chars.peek().copied() {
                Some('\\' | '{' | '}') => {
                    if let Some(escaped) = chars.next() {
                        out.push(escaped);
                    }
                }
                Some('\'') => {
                    let _ = chars.next();
                    let hi = chars.next();
                    let lo = chars.next();
                    if let (Some(hi), Some(lo)) = (hi, lo) {
                        let hex = format!("{hi}{lo}");
                        if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                            out.push(byte as char);
                        }
                    }
                }
                _ => {
                    let mut word = String::new();
                    while let Some(next) = chars.peek().copied() {
                        if next.is_ascii_alphabetic() {
                            word.push(next);
                            let _ = chars.next();
                        } else {
                            break;
                        }
                    }
                    while let Some(next) = chars.peek().copied() {
                        if next == '-' || next.is_ascii_digit() {
                            let _ = chars.next();
                        } else {
                            break;
                        }
                    }
                    if matches!(chars.peek(), Some(' ')) {
                        let _ = chars.next();
                    }
                    match word.as_str() {
                        "par" | "line" => out.push('\n'),
                        "tab" => out.push('\t'),
                        _ => {}
                    }
                }
            },
            _ if group_depth_to_skip.is_none() => out.push(ch),
            _ => {}
        }
    }

    out.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn card_content_matches_search(item: &ClipboardItem, terms: &[String]) -> bool {
    if terms.is_empty() {
        return false;
    }

    let full_text_matches = if matches!(item.display_kind(), DisplayKind::Html) {
        search_highlight::contains_match(&html_text::visible_text(&item.full_text), terms)
    } else {
        search_highlight::contains_match(&item.full_text, terms)
    };
    if full_text_matches {
        return true;
    }

    let rich = RichData::from_json(&item.rich_data);
    if let Some(html) = rich.html.as_deref() {
        let html = rich_preview::normalize_clipboard_html_for_render(html);
        if search_highlight::contains_match(&html_text::visible_text(&html), terms) {
            return true;
        }
    }

    let rich_text_matches = [
        rich.rtf.as_deref(),
        rich.ocr_text.as_deref(),
        rich.qr_text.as_deref(),
        rich.page_title.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|text| search_highlight::contains_match(text, terms));
    rich_text_matches
}

/// Compute estimated card height for the virtual list.
pub fn estimate_card_height(item: &ClipboardItem, card_height_mode: &str) -> f32 {
    if !item.note.is_empty() {
        return 68.0;
    }
    if card_height_mode == "low" {
        return 68.0;
    }
    if card_height_mode == "high" {
        return 128.0;
    }
    if card_height_mode != "auto" {
        return 96.0;
    }
    match item.content_type {
        ContentType::Image => {
            if item.image_width > 0 && item.image_height > 0 {
                let ratio = item.image_height as f32 / item.image_width as f32 * 100.0;
                if ratio <= 50.0 {
                    68.0
                } else if ratio <= 80.0 {
                    96.0
                } else {
                    128.0
                }
            } else {
                96.0
            }
        }
        ContentType::File => {
            let count = item.full_text.lines().count().max(1);
            if count <= 2 {
                68.0
            } else if count <= 3 {
                96.0
            } else {
                128.0
            }
        }
        _ => {
            // Semantic single-value text cards are compact; check via display_kind.
            if matches!(
                item.display_kind(),
                DisplayKind::Email
                    | DisplayKind::Phone
                    | DisplayKind::Secret
                    | DisplayKind::Link
                    | DisplayKind::Path
            ) {
                return 68.0;
            }
            let len = item.full_text.chars().count();
            if len <= 150 {
                68.0
            } else if len <= 300 {
                96.0
            } else {
                128.0
            }
        }
    }
}

#[derive(IntoElement)]
pub struct ClipboardCard {
    item: Rc<ClipboardItem>,
    selected: bool,
    index: usize,
    theme: ClippiTheme,
    selection_order: usize,
    on_click: Option<CardClickHandler>,
    is_hovered: bool,
    selected_count: usize,
    can_merge_selection: bool,
    on_toolbar_action: Option<CardActionHandler>,
    on_double_click: Option<CardIndexHandler>,
    /// Whether this card is in note-editing mode (shows inline editor).
    editing: bool,
    /// Shared InputState from ClipboardListView (only Some when editing is true).
    note_input: Option<Entity<InputState>>,
    /// Called when note editing is committed (Enter / confirm button).
    on_commit_note: Option<CardWindowHandler>,
    /// Whether this card is in hotkey-recording mode.
    recording_hotkey: bool,
    /// Called when hotkey recording is committed.
    on_commit_hotkey: Option<CardWindowHandler>,
    /// Called when hotkey recording is cancelled.
    on_cancel_hotkey: Option<CardWindowHandler>,
    /// Paste format selected during recording.
    hotkey_paste_format: String,
    /// Called when the hotkey paste format is changed while recording.
    on_hotkey_format_change: Option<CardFormatHandler>,
    show_source_app: bool,
    show_original_on_hover: bool,
    /// When true and a link item has a cached page title, show the title
    /// instead of the URL path in the card content area.
    show_page_title: bool,
    /// Whether the source file is queued for or currently being uploaded.
    source_is_uploading: bool,
    image_cache: Option<Entity<RetainAllImageCache>>,
    search_terms: Vec<String>,
}

impl ClipboardCard {
    pub fn new(item: Rc<ClipboardItem>, selected: bool, index: usize) -> Self {
        Self {
            item,
            selected,
            index,
            theme: ClippiTheme::dark(),
            selection_order: 0,
            on_click: None,
            is_hovered: false,
            selected_count: 0,
            can_merge_selection: false,
            on_toolbar_action: None,
            on_double_click: None,
            editing: false,
            note_input: None,
            on_commit_note: None,
            recording_hotkey: false,
            on_commit_hotkey: None,
            on_cancel_hotkey: None,
            hotkey_paste_format: String::new(),
            on_hotkey_format_change: None,
            show_source_app: false,
            show_original_on_hover: false,
            show_page_title: false,
            source_is_uploading: false,
            image_cache: None,
            search_terms: Vec::new(),
        }
    }

    pub fn on_click(mut self, handler: CardClickHandler) -> Self {
        self.on_click = Some(handler);
        self
    }

    /// Set whether this card is hovered (shows toolbar).
    pub fn hovered(mut self, hovered: bool) -> Self {
        self.is_hovered = hovered;
        self
    }

    pub fn selected_count(mut self, count: usize) -> Self {
        self.selected_count = count;
        self
    }

    pub fn can_merge_selection(mut self, value: bool) -> Self {
        self.can_merge_selection = value;
        self
    }

    pub fn theme(mut self, theme: ClippiTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Set the 1-based selection order for the badge (0 = hidden).
    pub fn selection_order(mut self, order: usize) -> Self {
        self.selection_order = order;
        self
    }

    pub fn on_toolbar_action(
        mut self,
        handler: impl Fn(&str, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toolbar_action = Some(Rc::new(handler));
        self
    }

    pub fn on_double_click(mut self, handler: CardIndexHandler) -> Self {
        self.on_double_click = Some(handler);
        self
    }

    /// Set whether this card is in note-editing mode.
    pub fn editing(mut self, editing: bool) -> Self {
        self.editing = editing;
        self
    }

    /// Set the shared InputState for inline note editing.
    pub fn note_input(mut self, input: Entity<InputState>) -> Self {
        self.note_input = Some(input);
        self
    }

    /// Called when note is committed (Enter / confirm button).
    pub fn on_commit_note(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_commit_note = Some(Rc::new(handler));
        self
    }

    pub fn recording_hotkey(mut self, recording: bool) -> Self {
        self.recording_hotkey = recording;
        self
    }

    pub fn hotkey_paste_format(mut self, format: String) -> Self {
        self.hotkey_paste_format = format;
        self
    }

    pub fn on_hotkey_format_change(
        mut self,
        handler: impl Fn(HotkeyPasteFormat, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_hotkey_format_change = Some(Rc::new(handler));
        self
    }

    pub fn on_commit_hotkey(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_commit_hotkey = Some(Rc::new(handler));
        self
    }

    pub fn on_cancel_hotkey(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_cancel_hotkey = Some(Rc::new(handler));
        self
    }

    pub fn show_source_app(mut self, value: bool) -> Self {
        self.show_source_app = value;
        self
    }

    pub fn show_original_on_hover(mut self, value: bool) -> Self {
        self.show_original_on_hover = value;
        self
    }

    pub fn show_page_title(mut self, value: bool) -> Self {
        self.show_page_title = value;
        self
    }

    pub fn source_is_uploading(mut self, value: bool) -> Self {
        self.source_is_uploading = value;
        self
    }

    pub fn image_cache(mut self, cache: Entity<RetainAllImageCache>) -> Self {
        self.image_cache = Some(cache);
        self
    }

    pub fn search_terms(mut self, terms: Vec<String>) -> Self {
        self.search_terms = terms;
        self
    }
}

impl RenderOnce for ClipboardCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            item,
            selected,
            index,
            theme,
            selection_order,
            on_click,
            is_hovered,
            selected_count,
            can_merge_selection,
            on_toolbar_action,
            on_double_click,
            editing,
            note_input,
            on_commit_note,
            show_source_app,
            show_original_on_hover,
            show_page_title,
            source_is_uploading,
            image_cache,
            search_terms,
            recording_hotkey,
            on_commit_hotkey: _on_commit_hotkey,
            on_cancel_hotkey: _on_cancel_hotkey,
            hotkey_paste_format,
            on_hotkey_format_change,
        } = self;

        let surface = theme.surface;
        let divider = theme.divider;
        let accent = theme.accent;
        let fav_color = theme.fav_color;
        let tag_bg = theme.tag_bg;
        let tag_text = theme.tag_text;
        let text_1 = theme.text_1;
        let text_2 = theme.text_2;
        let text_3 = theme.text_3;
        let danger = theme.danger;
        let path_warn = rgb(0xeab308);
        let is_dark = theme.bg == rgb(0x191a1b);
        let pill_bg = if is_dark {
            rgba(0x232425e8)
        } else {
            rgba(0xffffffe8)
        };
        let pill_border = if is_dark {
            rgba(0xffffff20)
        } else {
            rgba(0x00000014)
        };
        let color_border = if is_dark {
            rgba(0xffffff20)
        } else {
            rgba(0x00000018)
        };
        let subtle_row_bg = if is_dark {
            rgb(0x2b2c2d)
        } else {
            rgb(0xf4f5fb)
        };
        let hover_bg = if is_dark {
            rgba(0xffffff10)
        } else {
            rgba(0x0000000a)
        };
        let highlight_bg = theme.accent_highlight();
        let highlight_text = theme.accent_highlight_text();
        let time_str = format_relative_time(&item.updated_at);
        let is_fav = item.is_favorite;
        let content_type = item.content_type;
        let note = item.note.clone();
        let full_text = &item.full_text; // borrowed — only cloned in the branches that own it
        let img_w = item.image_width;
        let img_h = item.image_height;
        let preview_img_path = image_preview_path(&item);
        let image_name = image_display_name(&item);
        let meta_type = item.meta_type.clone();
        let tags = item.tags.clone();
        let transfer_file_data = if content_type == ContentType::File || meta_type == "transfer" {
            FileData::from_json(&item.file_data)
        } else {
            FileData::default()
        };
        let transfer_is_cloud = meta_type == "transfer"
            && transfer_file_data.is_transfer()
            && transfer_file_data
                .files
                .first()
                .is_none_or(|file| file.path.is_empty());
        let source_is_uploaded = content_type == ContentType::File
            && !transfer_file_data.transfer
            && !transfer_file_data.remote_hash.is_empty();
        let icon = type_icon(&item);
        let has_qr = has_qr_code(&item);
        let show_source = show_source_app && !item.source_app_name.is_empty();
        let label = type_label(&item);
        let source_icon_path = if show_source {
            source_icon_path(&item)
        } else {
            None
        };
        let color_swatch = swatch_color(full_text, accent);

        let border_color = if selected { accent } else { divider };

        let base = div()
            .relative()
            .w_full()
            .h_full()
            .overflow_hidden()
            .capture_any_mouse_up(|_ev, _window, cx| {
                cx.stop_propagation();
            })
            .bg(surface)
            .border(px(1.))
            .border_color(border_color)
            .rounded(px(10.))
            .overflow_hidden()
            .when(is_dark, |el| el.shadow_md())
            .when(!is_dark, |el| el.shadow_sm())
            .flex()
            .flex_row()
            .p(px(10.))
            .gap(px(10.));

        // --- Wire click handler ---
        let base = if let Some(handler) = on_click {
            let double_click_handler = on_double_click.clone();
            base.cursor(CursorStyle::PointingHand).on_mouse_down(
                MouseButton::Left,
                move |ev, window, cx| {
                    if ev.click_count == 2 {
                        // --- Double-click → paste ---
                        if let Some(ref dbl_handler) = double_click_handler {
                            dbl_handler(index, window, cx);
                        }
                    } else {
                        // --- Single click → select ---
                        handler(index, ev.modifiers, window, cx);
                    }
                },
            )
        } else {
            base
        };

        // --- Left: icon area (top-aligned with content) ---
        let icon_kind = item.display_kind();
        let icon_area = match icon_kind {
            DisplayKind::Color => div()
                .w(px(36.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(3.))
                .child(
                    div()
                        .w(px(36.))
                        .h(px(28.))
                        .rounded(px(6.))
                        .overflow_hidden()
                        .bg(tag_bg)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(if let Some(path) = source_icon_path.clone() {
                            gpui::img(path)
                                .when_some(image_cache.clone(), |img, cache| {
                                    img.image_cache(&cache)
                                })
                                .w(px(20.))
                                .h(px(20.))
                                .rounded(px(4.))
                                .into_any_element()
                        } else {
                            div()
                                .w(px(20.))
                                .h(px(20.))
                                .rounded(px(4.))
                                .bg(color_swatch)
                                .border(px(1.))
                                .border_color(color_border)
                                .into_any_element()
                        }),
                )
                .child(if source_icon_path.is_some() {
                    div()
                        .w(px(36.))
                        .h(px(14.))
                        .rounded(px(3.))
                        .bg(color_swatch)
                        .border(px(1.))
                        .border_color(color_border)
                        .p(px(2.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().w_full().h_full().rounded(px(1.)).bg(color_swatch))
                        .into_any_element()
                } else {
                    div()
                        .w(px(36.))
                        .h(px(14.))
                        .rounded(px(3.))
                        .bg(tag_bg)
                        .px(px(3.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(tag_text)
                                .truncate()
                                .child(label.to_string()),
                        )
                        .into_any_element()
                }),
            DisplayKind::Image => div()
                .w(px(36.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(3.))
                .child(
                    div()
                        .w(px(36.))
                        .h(px(28.))
                        .rounded(px(6.))
                        .overflow_hidden()
                        .bg(tag_bg)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(if let Some(path) = source_icon_path.clone() {
                            gpui::img(path)
                                .when_some(image_cache.clone(), |img, cache| {
                                    img.image_cache(&cache)
                                })
                                .w(px(20.))
                                .h(px(20.))
                                .rounded(px(4.))
                                .into_any_element()
                        } else {
                            div()
                                .text_size(px(18.))
                                .font_family("iconfont")
                                .text_color(tag_text)
                                .child(icon.to_string())
                                .into_any_element()
                        }),
                )
                .child(
                    div()
                        .w(px(36.))
                        .h(px(14.))
                        .rounded(px(3.))
                        .bg(tag_bg)
                        .px(px(3.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(tag_text)
                                .truncate()
                                .child(label.to_string()),
                        ),
                ),
            DisplayKind::File => {
                // --- Single file: prefer file system icon over source app icon ---
                let file_icon = if transfer_is_cloud {
                    None
                } else {
                    serde_json::from_str::<FileData>(&item.file_data)
                        .ok()
                        .and_then(|fd| {
                            if fd.files.len() == 1 {
                                fd.files
                                    .first()
                                    .and_then(|fi| cached_file_icon_path(&fi.path, fi.is_dir))
                            } else {
                                None
                            }
                        })
                };
                let effective_icon = file_icon.or(source_icon_path);
                let fallback_icon = if transfer_is_cloud { "\u{e794}" } else { icon };
                div()
                    .w(px(36.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(3.))
                    .child(
                        div()
                            .w(px(36.))
                            .h(px(28.))
                            .rounded(px(6.))
                            .overflow_hidden()
                            .bg(tag_bg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if let Some(path) = effective_icon {
                                gpui::img(path)
                                    .when_some(image_cache.clone(), |img, cache| {
                                        img.image_cache(&cache)
                                    })
                                    .w(px(20.))
                                    .h(px(20.))
                                    .rounded(px(4.))
                                    .into_any_element()
                            } else {
                                div()
                                    .text_size(px(18.))
                                    .font_family("iconfont")
                                    .text_color(tag_text)
                                    .child(fallback_icon.to_string())
                                    .into_any_element()
                            }),
                    )
                    .child(
                        div()
                            .w(px(36.))
                            .h(px(14.))
                            .rounded(px(3.))
                            .bg(tag_bg)
                            .px(px(3.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(tag_text)
                                    .truncate()
                                    .child(label.to_string()),
                            ),
                    )
            }
            DisplayKind::Link => {
                // --- Favicon priority: cache → source app icon → type icon ---
                // --- Only checks local cache — network fetch happens async ---
                // --- in the clipboard detection thread (ensure_favicon_cached). ---
                let domain = url_domain(full_text);
                let favicon_path = if domain.is_empty() {
                    None
                } else {
                    crate::services::favicon::favicon_cache_path(&domain)
                        .map(std::path::PathBuf::from)
                };
                // --- Source app icon only when the setting is enabled ---
                let app_icon = if show_source { source_icon_path } else { None };
                let effective_icon = favicon_path.or(app_icon);
                div()
                    .w(px(36.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(3.))
                    .child(
                        div()
                            .w(px(36.))
                            .h(px(28.))
                            .rounded(px(6.))
                            .overflow_hidden()
                            .bg(tag_bg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if let Some(path) = effective_icon {
                                gpui::img(path)
                                    .when_some(image_cache.clone(), |img, cache| {
                                        img.image_cache(&cache)
                                    })
                                    .w(px(20.))
                                    .h(px(20.))
                                    .rounded(px(4.))
                                    .into_any_element()
                            } else {
                                div()
                                    .text_size(px(18.))
                                    .font_family("iconfont")
                                    .text_color(tag_text)
                                    .child(icon.to_string())
                                    .into_any_element()
                            }),
                    )
                    .child(
                        div()
                            .w(px(36.))
                            .h(px(14.))
                            .rounded(px(3.))
                            .bg(tag_bg)
                            .px(px(3.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(tag_text)
                                    .truncate()
                                    .child(label.to_string()),
                            ),
                    )
            }
            _ => div()
                .w(px(36.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(3.))
                .child(
                    div()
                        .w(px(36.))
                        .h(px(28.))
                        .rounded(px(6.))
                        .overflow_hidden()
                        .bg(tag_bg)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(if let Some(path) = source_icon_path.clone() {
                            gpui::img(path)
                                .when_some(image_cache.clone(), |img, cache| {
                                    img.image_cache(&cache)
                                })
                                .w(px(20.))
                                .h(px(20.))
                                .rounded(px(4.))
                                .into_any_element()
                        } else {
                            div()
                                .text_size(px(18.))
                                .font_family("iconfont")
                                .text_color(tag_text)
                                .child(icon.to_string())
                                .into_any_element()
                        }),
                )
                .child(
                    div()
                        .w(px(36.))
                        .h(px(14.))
                        .rounded(px(3.))
                        .bg(tag_bg)
                        .px(px(3.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(tag_text)
                                .truncate()
                                .child(label.to_string()),
                        ),
                ),
        };

        let note_matches =
            !search_terms.is_empty() && search_highlight::contains_match(&note, &search_terms);
        let content_matches = card_content_matches_search(&item, &search_terms);
        let show_note_preview = !(note.is_empty()
            || show_original_on_hover && is_hovered
            || (!search_terms.is_empty() && content_matches && !note_matches));

        // --- Right: content area ---
        let content = if recording_hotkey {
            // --- Inline hotkey recording UI ---
            let record_text = I18nKey::LatestHotkeyRecording.text();
            let selected_format = normalized_hotkey_paste_format(&hotkey_paste_format, &item);
            let format_options = hotkey_paste_formats_for_item(&item);
            let format_handler = on_hotkey_format_change.clone();

            div()
                .flex_1()
                .flex()
                .flex_col()
                .justify_center()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.))
                        .child(
                            div()
                                .font_family("iconfont")
                                .text_size(px(14.))
                                .text_color(accent)
                                .child("\u{e66b}"),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(accent)
                                .child(record_text),
                        ),
                )
                .when(format_options.len() > 1, |el| {
                    el.child(
                        div()
                            .h(px(18.))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.))
                            .children(format_options.into_iter().map(|format| {
                                let selected = format == selected_format;
                                let handler = format_handler.clone();
                                div()
                                    .h(px(18.))
                                    .rounded(px(9.))
                                    .bg(if selected { accent } else { pill_bg })
                                    .border(px(1.))
                                    .border_color(if selected { accent } else { pill_border })
                                    .px(px(6.))
                                    .flex()
                                    .items_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(move |style| {
                                        if selected {
                                            style
                                        } else {
                                            style.border_color(accent)
                                        }
                                    })
                                    .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                        cx.stop_propagation();
                                        if let Some(ref handler) = handler {
                                            handler(format, window, cx);
                                        }
                                    })
                                    .child(
                                        div()
                                            .text_size(px(9.))
                                            .text_color(if selected {
                                                rgb(0xffffff)
                                            } else {
                                                text_2
                                            })
                                            .child(hotkey_paste_format_label(format)),
                                    )
                            })),
                    )
                })
        } else if editing {
            // --- Inline note editor ---
            let commit = on_commit_note.clone();
            let note_input_ref = note_input.clone();

            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    // --- Single-line text input ---
                    div().w_full().h(px(24.)).child({
                        let input_entity =
                            note_input_ref.expect("note_input must be set when editing");
                        Input::new(&input_entity)
                            .appearance(false)
                            .bordered(false)
                            .focus_bordered(false)
                            .w_full()
                            .h_full()
                            .text_size(px(12.))
                    }),
                )
                .child(
                    // --- Confirm button (checkmark icon \u{e611}) ---
                    div().flex().flex_row().justify_end().child(
                        div()
                            .w(px(20.))
                            .h(px(20.))
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(move |style| style.bg(hover_bg))
                            .on_mouse_down(MouseButton::Left, {
                                let commit = commit.clone();
                                move |_ev, window, cx| {
                                    cx.stop_propagation();
                                    if let Some(ref handler) = commit {
                                        handler(window, cx);
                                    }
                                }
                            })
                            .child(
                                div()
                                    .font_family("iconfont")
                                    .text_size(px(12.))
                                    .text_color(accent)
                                    .child("\u{e611}"), // checkmark icon
                            ),
                    ),
                )
                .on_key_down({
                    let commit = commit.clone();
                    move |ev: &KeyDownEvent, window, cx| {
                        if ev.keystroke.key.as_str() == "enter" {
                            if let Some(ref handler) = commit {
                                handler(window, cx);
                            }
                            cx.stop_propagation();
                        }
                    }
                })
        } else if show_note_preview {
            // Note present, not hovering → display note text (single line, card at min height)
            div().flex_1().flex().items_center().child(
                div()
                    .w_full()
                    .text_size(px(12.))
                    .text_color(text_2)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(search_highlight::render_highlighted_inline(
                        note,
                        &search_terms,
                        text_2,
                        highlight_bg,
                        highlight_text,
                        12.0,
                        None,
                    )),
            )
        } else {
            // No note, or note present + hovering with show_original → render full content.
            // Card height is clamped to 68px by estimate_card_height when note exists,
            // and overflow_hidden on the base div naturally clips oversized content.
            let content_kind = item.display_kind();
            // Link and Path rendering (now sub-types of PlainText via meta_type)
            // must be checked before the catch-all PlainText arm below.
            if matches!(content_kind, DisplayKind::Link | DisplayKind::Path) {
                if matches!(content_kind, DisplayKind::Link) {
                    let domain = url_domain(full_text);
                    let masked_url =
                        crate::core::secret::sensitive_preview_to_text(full_text, "link");
                    let path = url_path(&masked_url);

                    // When page-title fetching is enabled and a title is cached,
                    // simplify the domain to just the site name and show the
                    // title separated by a dash.  Otherwise show the raw URL path
                    // (which naturally starts with '/', no separator needed).
                    let (label, subtitle, show_sep) = if show_page_title {
                        let rd = RichData::from_json(&item.rich_data);
                        match rd.page_title {
                            Some(title) => (
                                url_site_name(full_text),
                                title,
                                true, // "Site - Title"
                            ),
                            None => (domain, path, false), // fallback
                        }
                    } else {
                        (domain, path, false)
                    };

                    // URL: bold domain/site-name + dimmed subtitle (title or path)
                    div()
                        .flex_1()
                        .flex()
                        .flex_row()
                        .items_center()
                        .overflow_hidden()
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(text_1)
                                .child(search_highlight::render_highlighted_inline(
                                    label,
                                    &search_terms,
                                    text_1,
                                    highlight_bg,
                                    highlight_text,
                                    13.0,
                                    Some(FontWeight::BOLD),
                                )),
                        )
                        .when(show_sep, |this| {
                            this.child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(text_3)
                                    .mx(px(2.))
                                    .child(" - "),
                            )
                        })
                        .child(div().text_size(px(13.)).text_color(text_3).child(
                            search_highlight::render_highlighted_auxiliary_inline(
                                subtitle,
                                &search_terms,
                                text_3,
                                highlight_bg,
                                highlight_text,
                                13.0,
                            ),
                        ))
                } else {
                    // File system path: bold last component + dimmed full path.
                    // Non-existent paths get a red tint + reduced opacity
                    // (UNC network paths skip the existence check).
                    let path_foreign = !crate::core::types::path_is_native(&item.full_text);
                    let path_invalid =
                        !path_foreign && !crate::core::types::path_exists(&item.full_text);
                    let label_color = if path_invalid {
                        danger
                    } else if path_foreign {
                        path_warn
                    } else {
                        text_1
                    };
                    let path_text = item.full_text.trim_end_matches(['\\', '/']).to_string();
                    let (leaf, show_full) = match path_text.rfind(['\\', '/']) {
                        Some(pos) if pos + 1 < path_text.len() => {
                            (path_text[pos + 1..].to_string(), true)
                        }
                        _ => (path_text, false),
                    };
                    if show_full {
                        let full = item.full_text.clone();
                        div()
                            .flex_1()
                            .flex()
                            .flex_row()
                            .items_center()
                            .overflow_hidden()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(label_color)
                                    .child(search_highlight::render_highlighted_inline(
                                        leaf,
                                        &search_terms,
                                        label_color,
                                        highlight_bg,
                                        highlight_text,
                                        13.0,
                                        Some(FontWeight::BOLD),
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(text_3)
                                    .mx(px(2.))
                                    .child(" - "),
                            )
                            .child(div().text_size(px(13.)).text_color(text_3).child(
                                search_highlight::render_highlighted_auxiliary_inline(
                                    full,
                                    &search_terms,
                                    text_3,
                                    highlight_bg,
                                    highlight_text,
                                    13.0,
                                ),
                            ))
                    } else {
                        div().flex_1().flex().items_center().child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(label_color)
                                .overflow_hidden()
                                .child(search_highlight::render_highlighted_inline(
                                    item.full_text.clone(),
                                    &search_terms,
                                    label_color,
                                    highlight_bg,
                                    highlight_text,
                                    13.0,
                                    Some(FontWeight::BOLD),
                                )),
                        )
                    }
                }
            } else if matches!(
                content_kind,
                DisplayKind::Email | DisplayKind::Phone | DisplayKind::Secret,
            ) {
                let parts = crate::core::secret::sensitive_preview_parts(full_text, &meta_type);
                div().flex_1().flex().items_center().child(
                    SensitiveText::new(parts)
                        .search_terms(search_terms.clone())
                        .text_color(text_1)
                        .mask_color(text_3)
                        .highlight_bg(highlight_bg)
                        .highlight_text(highlight_text)
                        .font_size(13.0)
                        .font_weight(FontWeight::BOLD),
                )
            } else if matches!(content_kind, DisplayKind::Color) {
                div().flex_1().flex().items_center().child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(text_1)
                        .overflow_hidden()
                        .child(search_highlight::render_highlighted_inline(
                            item.full_text.clone(),
                            &search_terms,
                            text_1,
                            highlight_bg,
                            highlight_text,
                            12.0,
                            Some(FontWeight::BOLD),
                        )),
                )
            } else {
                match content_type {
                    ContentType::Image => {
                        let img_missing = !item.image_path.is_empty()
                            && !std::path::Path::new(&item.image_path).exists();
                        let img_not_loaded =
                            img_missing && image_path_is_cache_path(&item.image_path);
                        let img_stale = img_missing && !img_not_loaded;
                        // Show previews only when the full image exists locally. Synced images
                        // regenerate thumbnails after the blob is downloaded.
                        if let Some(preview_img_path) =
                            preview_img_path.clone().filter(|_| !img_missing)
                        {
                            let object_fit = if has_qr {
                                ObjectFit::Contain
                            } else {
                                ObjectFit::Cover
                            };
                            div()
                                .relative()
                                .flex_1()
                                .w_full()
                                .h_full()
                                .rounded(px(8.))
                                .overflow_hidden()
                                .bg(tag_bg)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    gpui::img(preview_img_path)
                                        .when_some(image_cache.clone(), |img, cache| {
                                            img.image_cache(&cache)
                                        })
                                        .w_full()
                                        .h_full()
                                        .rounded(px(8.))
                                        .object_fit(object_fit),
                                )
                                .child(
                                    // --- Rounded border overlay — masks sharp image corners ---
                                    // --- since GPUI overflow_hidden does not clip img elements ---
                                    // --- Negative inset makes the overlay extend outward so the ---
                                    // --- border (which draws inward) covers the image corners.  ---
                                    div()
                                        .absolute()
                                        .inset(px(-3.))
                                        .rounded(px(11.))
                                        .border(px(4.))
                                        .border_color(surface),
                                )
                        } else if img_missing || preview_img_path.is_none() {
                            let (placeholder_color, placeholder_icon) = if img_stale {
                                (danger, "\u{e607}")
                            } else {
                                (text_3, "\u{e626}")
                            };
                            div()
                                .flex_1()
                                .w_full()
                                .h_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .mr(px(CARD_ICON_WIDTH + CARD_CONTENT_GAP))
                                .child(
                                    div()
                                        .size(px(40.))
                                        .rounded(px(8.))
                                        .bg(subtle_row_bg)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            div()
                                                .text_size(px(24.))
                                                .font_family("iconfont")
                                                .text_color(placeholder_color)
                                                .child(placeholder_icon),
                                        ),
                                )
                        } else {
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .mr(px(CARD_ICON_WIDTH + CARD_CONTENT_GAP))
                                .child(
                                    div()
                                        .w_full()
                                        .mb(px(6.))
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .justify_center()
                                        .gap(px(2.))
                                        .child(
                                            div()
                                                .h(px(24.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_size(px(22.))
                                                .font_family("iconfont")
                                                .text_color(text_2)
                                                .child("\u{e626}"),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .h(px(15.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(
                                                    div()
                                                        .max_w_full()
                                                        .text_size(px(11.))
                                                        .line_height(px(14.))
                                                        .text_color(text_3)
                                                        .overflow_hidden()
                                                        .whitespace_nowrap()
                                                        .truncate()
                                                        .child(image_name),
                                                ),
                                        ),
                                )
                        }
                    }
                    ContentType::PlainText | ContentType::RichText => {
                        let content_box = div()
                            .flex_1()
                            .w_full()
                            .text_size(px(12.))
                            .text_color(text_1)
                            .line_height(px(18.))
                            .overflow_hidden();

                        let style = TextViewStyle::default()
                            .paragraph_gap(rems(0.25))
                            .heading_font_size(|_level, base| base);
                        match rich_preview(&item) {
                            RichPreview::Html(html) => {
                                if search_terms.is_empty() {
                                    content_box.child(
                                        TextView::html(
                                            ("clipboard-card-html", item.content_hash),
                                            html,
                                            window,
                                            cx,
                                        )
                                        .style(style)
                                        .selectable(false),
                                    )
                                } else {
                                    let visible_text = html_text::visible_text(&html);
                                    content_box.child(search_highlight::render_highlighted_block(
                                        visible_text,
                                        &search_terms,
                                        text_1,
                                        highlight_bg,
                                        highlight_text,
                                        12.0,
                                        18.0,
                                    ))
                                }
                            }
                            RichPreview::Markdown(markdown) => {
                                if search_terms.is_empty() {
                                    content_box.child(
                                        TextView::markdown(
                                            ("clipboard-card-markdown", item.content_hash),
                                            markdown,
                                            window,
                                            cx,
                                        )
                                        .style(style)
                                        .selectable(false),
                                    )
                                } else {
                                    content_box.child(search_highlight::render_highlighted_block(
                                        markdown,
                                        &search_terms,
                                        text_1,
                                        highlight_bg,
                                        highlight_text,
                                        12.0,
                                        18.0,
                                    ))
                                }
                            }
                            RichPreview::StyledHtml {
                                lines,
                                visible_text,
                            } => {
                                if search_terms.is_empty() {
                                    content_box.child(rich_preview::render_styled_html_lines(
                                        lines, text_1,
                                    ))
                                } else {
                                    let lines =
                                        rich_preview::focus_styled_html_lines(lines, &search_terms);
                                    let lines = rich_preview::highlight_styled_html_lines(
                                        lines,
                                        &search_terms,
                                        highlight_bg,
                                        highlight_text,
                                    );
                                    if rich_preview::has_highlighted_span(&lines, highlight_bg) {
                                        content_box.child(rich_preview::render_styled_html_lines(
                                            lines, text_1,
                                        ))
                                    } else {
                                        content_box.child(
                                            search_highlight::render_highlighted_block(
                                                visible_text,
                                                &search_terms,
                                                text_1,
                                                highlight_bg,
                                                highlight_text,
                                                12.0,
                                                18.0,
                                            ),
                                        )
                                    }
                                }
                            }
                            RichPreview::Plain(preview) => {
                                if search_terms.is_empty() {
                                    content_box.child(preview.chars().take(300).collect::<String>())
                                } else {
                                    content_box.child(search_highlight::render_highlighted_block(
                                        preview,
                                        &search_terms,
                                        text_1,
                                        highlight_bg,
                                        highlight_text,
                                        12.0,
                                        18.0,
                                    ))
                                }
                            }
                        }
                    }
                    ContentType::File => {
                        let file_data: FileData =
                            serde_json::from_str(&item.file_data).unwrap_or_default();
                        let files: Vec<FileInfo> = file_data.files;
                        let multi = files.len() > 1;
                        let file_missing = !transfer_is_cloud
                            && !multi
                            && files.first().is_some_and(|file| {
                                crate::services::file_status::cached_file_exists(&file.path)
                                    == Some(false)
                            });
                        if file_missing {
                            let fi = &files[0];
                            let (stem, ext) = if fi.is_dir {
                                (fi.name.clone(), String::new())
                            } else {
                                split_name_ext(&fi.name)
                            };
                            let bad_icon = if fi.is_dir { "\u{e60f}" } else { "\u{e646}" };
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .rounded(px(4.))
                                        .bg(subtle_row_bg)
                                        .px(px(6.))
                                        .py(px(4.))
                                        .flex()
                                        .flex_row()
                                        .gap(px(4.))
                                        .items_center()
                                        .overflow_hidden()
                                        .child(
                                            div()
                                                .font_family("iconfont")
                                                .text_size(px(12.))
                                                .text_color(danger)
                                                .child(bad_icon),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .flex()
                                                .flex_row()
                                                .gap(px(0.))
                                                .overflow_hidden()
                                                .child(
                                                    div()
                                                        .text_size(px(10.))
                                                        .text_color(danger)
                                                        .whitespace_nowrap()
                                                        .overflow_hidden()
                                                        .child(stem),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(10.))
                                                        .text_color(danger)
                                                        .child(ext),
                                                ),
                                        ),
                                )
                        } else {
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .overflow_hidden()
                                .children(files.iter().take(4).map(|fi| {
                                    let (stem, ext) = if fi.is_dir {
                                        (fi.name.clone(), String::new())
                                    } else {
                                        split_name_ext(&fi.name)
                                    };
                                    let cached_icon = cached_file_icon_path(&fi.path, fi.is_dir);
                                    let fallback_icon =
                                        if fi.is_dir { "\u{e60f}" } else { "\u{e646}" };
                                    let row = div()
                                        .rounded(px(4.))
                                        .bg(subtle_row_bg)
                                        .px(px(6.))
                                        .py(px(4.))
                                        .flex()
                                        .flex_row()
                                        .gap(px(4.))
                                        .items_center()
                                        .overflow_hidden();
                                    let row = if multi {
                                        row.child(
                                            div()
                                                .w(px(14.))
                                                .h(px(14.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(if let Some(path) = cached_icon {
                                                    gpui::img(path)
                                                        .when_some(
                                                            image_cache.clone(),
                                                            |img, cache| img.image_cache(&cache),
                                                        )
                                                        .w(px(14.))
                                                        .h(px(14.))
                                                        .rounded(px(2.))
                                                        .into_any_element()
                                                } else {
                                                    div()
                                                        .font_family("iconfont")
                                                        .text_size(px(12.))
                                                        .text_color(text_3)
                                                        .child(fallback_icon)
                                                        .into_any_element()
                                                }),
                                        )
                                    } else {
                                        row
                                    };
                                    row.child(
                                        div()
                                            .flex_1()
                                            .flex()
                                            .flex_row()
                                            .gap(px(0.))
                                            .overflow_hidden()
                                            .child(
                                                div()
                                                    .text_size(px(10.))
                                                    .text_color(text_1)
                                                    .whitespace_nowrap()
                                                    .overflow_hidden()
                                                    .child(stem),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.))
                                                    .text_color(if file_missing {
                                                        danger
                                                    } else {
                                                        text_2
                                                    })
                                                    .child(ext),
                                            ),
                                    )
                                }))
                        }
                    } // else: multi-file or normal single file
                }
            } // closes if-else block
        };

        // --- Size label for types that have measurable content ---
        let mut size_label_danger = false;
        let mut size_label_warn = false;
        let size_label: Option<String> = match content_type {
            ContentType::PlainText | ContentType::RichText => {
                // Path items show the drive label (or "已失效" / platform label).
                if item.meta_type == "path" {
                    // Check foreign platform first — a Mac path on Windows
                    // will never exist here regardless of what's on disk.
                    if !crate::core::types::path_is_native(&item.full_text) {
                        size_label_warn = true;
                        Some(foreign_path_label())
                    } else if !crate::core::types::path_exists(&item.full_text) {
                        size_label_danger = true;
                        Some(I18nKey::CardStaleFile.text().to_string())
                    } else {
                        let rd = RichData::from_json(&item.rich_data);
                        rd.drive_label
                    }
                // Link items show the protocol instead of character count.
                } else if item.meta_type == "link" {
                    if item.full_text.starts_with("https://") {
                        Some("HTTPS".to_string())
                    } else if item.full_text.starts_with("http://") {
                        Some("HTTP".to_string())
                    } else {
                        None
                    }
                } else {
                    let count = item.size.max(0) as u64;
                    if count > 0 {
                        Some(I18nKey::CardChars.fmt(&[&count.to_string()]))
                    } else {
                        None
                    }
                }
            }
            ContentType::Image => {
                let img_missing =
                    !item.image_path.is_empty() && !std::path::Path::new(&item.image_path).exists();
                let img_not_loaded = img_missing && image_path_is_cache_path(&item.image_path);
                if img_not_loaded {
                    Some(I18nKey::CardImageNotLoaded.text().to_string())
                } else if img_missing {
                    size_label_danger = true;
                    Some(I18nKey::CardStaleFile.text().to_string())
                } else if img_w > 0 && img_h > 0 {
                    Some(format!("{}×{}", img_w, img_h))
                } else {
                    None
                }
            }
            ContentType::File => {
                let fd: FileData = serde_json::from_str(&item.file_data).unwrap_or_default();
                let count = fd.files.len();
                // Only check single-file items for missing sources.
                let file_missing = !transfer_is_cloud
                    && count == 1
                    && fd.files.first().is_some_and(|file| {
                        crate::services::file_status::cached_file_exists(&file.path) == Some(false)
                    });
                if file_missing {
                    size_label_danger = true;
                    Some(I18nKey::CardStaleFile.text().to_string())
                } else if count > 1 {
                    Some(I18nKey::CardFilesCount.fmt(&[&count.to_string()]))
                } else if item.size > 0 {
                    Some(format_file_size(item.size))
                } else {
                    None
                }
            }
        };

        // --- Bottom info row: tags → size label → time ---
        // Detect transfer items and filter out status tags (handled separately)
        let is_transfer_status = |uid: &str| {
            matches!(
                uid,
                TRANSFER_STATUS_LOCAL_UID
                    | TRANSFER_STATUS_CLOUD_UID
                    | TRANSFER_STATUS_DOWNLOADING_UID
            )
        };
        let is_transfer =
            item.meta_type == "transfer" && tags.iter().any(|tag| is_transfer_status(&tag.uid));
        let transfer_is_local =
            is_transfer && tags.iter().any(|tag| tag.uid == TRANSFER_STATUS_LOCAL_UID);
        let transfer_is_downloading = is_transfer
            && tags
                .iter()
                .any(|tag| tag.uid == TRANSFER_STATUS_DOWNLOADING_UID);
        let display_tags: Vec<_> = if is_transfer {
            tags.iter()
                .filter(|tag| !is_transfer_status(&tag.uid))
                .cloned()
                .collect()
        } else {
            tags.clone()
        };
        let tag_widths = display_tags
            .iter()
            .map(|tag| info_pill_width(&tag.name, INFO_PILL_PADDING_X, window))
            .collect::<Vec<_>>();
        let mut fixed_widths = Vec::with_capacity(3);
        // Hotkey pill (if item has a custom hotkey)
        let hotkey_pill = if !item.custom_hotkey.is_empty() {
            Some(item.custom_hotkey.clone())
        } else {
            None
        };
        if let Some(ref hk) = hotkey_pill {
            fixed_widths.push(hotkey_pill_width(hk, window));
        }
        // Transfer status pill (for transfer station items)
        let transfer_pill: Option<(String, TransferPillKind)> = if is_transfer {
            if transfer_is_downloading {
                Some((
                    I18nKey::TransferDownloading.text().to_string(),
                    TransferPillKind::Downloading,
                ))
            } else if transfer_is_local {
                Some((
                    I18nKey::TransferLocal.text().to_string(),
                    TransferPillKind::Local,
                ))
            } else {
                Some((
                    I18nKey::TransferCloud.text().to_string(),
                    TransferPillKind::Cloud,
                ))
            }
        } else {
            None
        };
        if let Some((label, _)) = transfer_pill.as_ref() {
            fixed_widths.push(info_pill_width(
                label,
                INFO_PILL_PADDING_X + 16., // extra space for icon
                window,
            ));
        }
        if source_is_uploading || source_is_uploaded {
            fixed_widths.push(18.);
        }
        if let Some(label) = size_label.as_deref() {
            fixed_widths.push(info_pill_width(label, INFO_PILL_PADDING_X, window));
        }
        fixed_widths.push(info_pill_width(&time_str, TIME_PILL_PADDING_X, window));
        let visible_tag_count = visible_tag_count(
            &tag_widths,
            &fixed_widths,
            info_row_available_width(window),
            |hidden_count| {
                info_pill_width(&format!("+{hidden_count}"), INFO_PILL_PADDING_X, window)
            },
        );
        let hidden_tag_count = display_tags.len() - visible_tag_count;

        let bottom_info = div()
            .absolute()
            .right(px(10.))
            .bottom(px(6.))
            .h(px(18.))
            .flex()
            .flex_row()
            .gap(px(4.))
            .items_center()
            .when(hidden_tag_count > 0, |el| {
                el.child(
                    div()
                        .h(px(18.))
                        .rounded(px(9.))
                        .bg(pill_bg)
                        .border(px(1.))
                        .border_color(pill_border)
                        .px(px(5.))
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .text_size(px(9.))
                                .text_color(text_2)
                                .child(format!("+{hidden_tag_count}")),
                        ),
                )
            })
            .children(tags.iter().take(visible_tag_count).map(|tag| {
                let tag_color = color_from_hex(&tag.color, text_2);
                div()
                    .h(px(18.))
                    .rounded(px(9.))
                    .bg(pill_bg)
                    .border(px(1.))
                    .border_color(tag_color)
                    .px(px(5.))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(9.))
                            .text_color(tag_color)
                            .child(tag.name.clone()),
                    )
            }))
            .when_some(hotkey_pill, |el, hk| {
                el.child(
                    div()
                        .h(px(18.))
                        .rounded(px(9.))
                        .bg(pill_bg)
                        .border(px(1.))
                        .border_color(accent)
                        .px(px(5.))
                        .flex()
                        .items_center()
                        .gap(px(HOTKEY_PILL_ICON_GAP))
                        .child(
                            div()
                                .font_family("iconfont")
                                .text_size(px(9.))
                                .text_color(accent)
                                .child("\u{e66b}"),
                        )
                        .child(div().text_size(px(9.)).text_color(accent).child(hk)),
                )
            })
            .when_some(transfer_pill, |el, (label, kind)| {
                let pill_text_color = match kind {
                    TransferPillKind::Local => rgb(0x22C55E),
                    TransferPillKind::Cloud | TransferPillKind::Downloading => rgb(0x3B82F6),
                };
                let animation_id: SharedString =
                    format!("transfer-download-spinner-{}", item.id).into();
                el.child(
                    div()
                        .h(px(18.))
                        .rounded(px(9.))
                        .bg(pill_bg)
                        .border(px(1.))
                        .border_color(pill_border)
                        .px(px(5.))
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .child(if matches!(kind, TransferPillKind::Downloading) {
                            activity_spinner(animation_id, pill_text_color, 12.)
                        } else {
                            div()
                                .font_family("iconfont")
                                .text_size(px(9.))
                                .text_color(pill_text_color)
                                .child(match kind {
                                    TransferPillKind::Cloud => "\u{e601}",
                                    TransferPillKind::Local => "\u{e794}",
                                    TransferPillKind::Downloading => unreachable!(),
                                })
                                .into_any_element()
                        })
                        .child(
                            div()
                                .text_size(px(9.))
                                .text_color(pill_text_color)
                                .child(label),
                        ),
                )
            })
            .when(source_is_uploading || source_is_uploaded, |el| {
                let uploading_spinner_id: SharedString =
                    format!("transfer-upload-spinner-{}", item.id).into();
                el.child(
                    div()
                        .size(px(18.))
                        .rounded(px(9.))
                        .bg(pill_bg)
                        .border(px(1.))
                        .border_color(pill_border)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(0x3B82F6))
                        .child(if source_is_uploading {
                            activity_spinner(uploading_spinner_id, rgb(0x3B82F6), 14.)
                        } else {
                            div()
                                .font_family("iconfont")
                                .text_size(px(10.))
                                .child("\u{e794}")
                                .into_any_element()
                        }),
                )
            })
            .when_some(size_label, |el, label| {
                let pill_text_color = if size_label_danger {
                    danger
                } else if size_label_warn {
                    path_warn
                } else {
                    text_2
                };
                el.child(
                    div()
                        .h(px(18.))
                        .rounded(px(9.))
                        .bg(pill_bg)
                        .border(px(1.))
                        .border_color(pill_border)
                        .px(px(5.))
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .text_size(px(9.))
                                .text_color(pill_text_color)
                                .child(label),
                        ),
                )
            })
            .child(
                div()
                    .h(px(18.))
                    .rounded(px(9.))
                    .bg(pill_bg)
                    .border(px(1.))
                    .border_color(pill_border)
                    .px(px(7.))
                    .flex()
                    .items_center()
                    .child(div().text_size(px(9.)).text_color(text_2).child(time_str)),
            );

        // --- Assemble card ---
        let card = base.child(icon_area).child(content);
        // --- Hide bottom tags/time row during inline editing/recording. ---
        let card = if !editing && !recording_hotkey {
            card.child(bottom_info)
        } else {
            card
        };

        // --- Fav indicator bar (left edge, scales with card height) ---
        let card = if is_fav {
            card.child(
                div()
                    .absolute()
                    .left(px(0.))
                    .top(px(4.))
                    .bottom(px(4.))
                    .w(px(3.))
                    .rounded(px(2.))
                    .bg(fav_color),
            )
        } else {
            card
        };

        // Selection badge — small circle centered at card top-left corner (0,0).
        // --- Only shown when multi-selecting (>1). ---
        let card = if selected && selected_count > 1 {
            card.child(
                div()
                    .absolute()
                    .left(px(0.))
                    .top(px(0.))
                    .w(px(16.))
                    .h(px(16.))
                    .rounded_full()
                    .bg(accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_size(px(8.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0xffffff))
                            .child(format!("{}", selection_order)),
                    ),
            )
        } else {
            card
        };

        // --- Hover toolbar (hidden during inline editing/recording). ---
        if is_hovered && !editing && !recording_hotkey {
            let toolbar_props = HoverToolbarProps::from_item(&item, selected_count, selected)
                .can_merge_selection(can_merge_selection);
            card.child(
                div().absolute().top(px(3.)).right(px(4.)).child(
                    HoverToolbar::new(toolbar_props)
                        .theme(theme.clone())
                        .on_action(move |action, _window, cx| {
                            if let Some(ref handler) = on_toolbar_action {
                                handler(action, _window, cx);
                            }
                        }),
                ),
            )
        } else {
            card
        }
    }
}
