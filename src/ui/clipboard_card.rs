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

use base64::Engine;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::text::{TextView, TextViewStyle};

use crate::core::color::detect_color;
use crate::core::frontend::PANEL_OFFSET_X;
use crate::core::i18n_keys::I18nKey;
use crate::core::types::{
    format_relative_time, is_email, is_phone,
    mask_sensitive_preview, parse_hex_color, url_domain, url_path, ClipboardItem, ContentType,
    DisplayKind, FileData, FileInfo, RichData,
};

use super::hover_toolbar::{HoverToolbar, HoverToolbarProps};
use super::rich_preview::{self, StyledHtmlSpan};
use super::theme::ClippiTheme;

type CardClickHandler = Rc<dyn Fn(usize, Modifiers, &mut Window, &mut App)>;
type CardIndexHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;
type CardActionHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;
type CardWindowHandler = Rc<dyn Fn(&mut Window, &mut App)>;

const INFO_PILL_FONT_SIZE: f32 = 9.0;
const INFO_PILL_GAP: f32 = 4.0;
const INFO_PILL_BORDER_WIDTH: f32 = 1.0;
const INFO_PILL_PADDING_X: f32 = 5.0;
const TIME_PILL_PADDING_X: f32 = 7.0;
const PANEL_BORDER_WIDTH: f32 = 1.0;
const LIST_PADDING_X: f32 = 8.0;
const CARD_PADDING_X: f32 = 10.0;
const CARD_ICON_WIDTH: f32 = 36.0;
const CARD_CONTENT_GAP: f32 = 10.0;

fn info_pill_width(text: &str, padding_x: f32, window: &Window) -> f32 {
    let text: SharedString = text.to_owned().into();
    let run = window.text_style().to_run(text.len());
    let line = window
        .text_system()
        .shape_line(text, px(INFO_PILL_FONT_SIZE), &[run], None);

    f32::from(line.width).ceil() + padding_x * 2.0 + INFO_PILL_BORDER_WIDTH * 2.0
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
        DisplayKind::File => "\u{e646}",
        DisplayKind::Link => "\u{e6d7}",
        DisplayKind::Path => "\u{e60f}",
        DisplayKind::Color => "\u{e610}",
        DisplayKind::Email => "\u{e604}",
        DisplayKind::Phone => "\u{e966}",
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

fn cached_source_icon_path(item: &ClipboardItem) -> Option<std::path::PathBuf> {
    if item.source_app_name.is_empty() || item.source_app_icon.is_empty() {
        return None;
    }
    let icon_dir = crate::core::paths::images_dir().join("icons");
    let _ = std::fs::create_dir_all(&icon_dir);
    let safe_name: String = item
        .source_app_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe_name.is_empty() {
        return None;
    }
    let path = icon_dir.join(format!("{safe_name}.png"));
    if !path.exists() {
        let png = base64::engine::general_purpose::STANDARD
            .decode(&item.source_app_icon)
            .ok()?;
        std::fs::write(&path, png).ok()?;
    }
    Some(path)
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

fn cached_file_icon_path(file_path: &str, is_dir: bool) -> Option<std::path::PathBuf> {
    use std::path::Path;
    let cache_key = if is_dir {
        "folder".to_string()
    } else {
        Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("file")
            .to_string()
    };
    let icon_dir = crate::core::paths::images_dir().join("file_icons");
    let _ = std::fs::create_dir_all(&icon_dir);
    let icon_path = icon_dir.join(format!("{cache_key}.png"));
    if !icon_path.exists() {
        let icon_base64 = crate::platform::source::get_file_icon_base64(file_path, is_dir)?;
        let png = base64::engine::general_purpose::STANDARD
            .decode(&icon_base64)
            .ok()?;
        std::fs::write(&icon_path, png).ok()?;
    }
    Some(icon_path)
}

enum RichPreview {
    StyledHtml(Vec<Vec<StyledHtmlSpan>>),
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
                return RichPreview::StyledHtml(lines);
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
                RichPreview::Plain(item.full_text.chars().take(300).collect())
            }
        }
        _ => RichPreview::Plain(item.full_text.chars().take(300).collect()),
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
            // Link/Path are compact — check via display_kind
            if matches!(item.display_kind(), DisplayKind::Link | DisplayKind::Path) {
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
    on_toolbar_action: Option<CardActionHandler>,
    on_double_click: Option<CardIndexHandler>,
    /// Whether this card is in note-editing mode (shows inline editor).
    editing: bool,
    /// Shared InputState from ClipboardListView (only Some when editing is true).
    note_input: Option<Entity<InputState>>,
    /// Called when note editing is committed (Enter / confirm button).
    on_commit_note: Option<CardWindowHandler>,
    show_source_app: bool,
    show_original_on_hover: bool,
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
            on_toolbar_action: None,
            on_double_click: None,
            editing: false,
            note_input: None,
            on_commit_note: None,
            show_source_app: false,
            show_original_on_hover: false,
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

    pub fn show_source_app(mut self, value: bool) -> Self {
        self.show_source_app = value;
        self
    }

    pub fn show_original_on_hover(mut self, value: bool) -> Self {
        self.show_original_on_hover = value;
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
            on_toolbar_action,
            on_double_click,
            editing,
            note_input,
            on_commit_note,
            show_source_app,
            show_original_on_hover,
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
        let time_str = format_relative_time(&item.updated_at);
        let is_fav = item.is_favorite;
        let content_type = item.content_type;
        let note = item.note.clone();
        let full_text = item.full_text.clone();
        let img_w = item.image_width;
        let img_h = item.image_height;
        let img_path = item.image_path.clone();
        let meta_type = item.meta_type.clone();
        let tags = item.tags.clone();
        let icon = type_icon(&item);
        let has_qr = has_qr_code(&item);
        let show_source = show_source_app && !item.source_app_name.is_empty();
        let label = type_label(&item);
        let source_icon_path = if show_source {
            cached_source_icon_path(&item)
        } else {
            None
        };
        let color_swatch = swatch_color(&full_text, accent);

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
                            gpui::img(path).w(px(20.)).h(px(20.)).rounded(px(4.)).into_any_element()
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
                            gpui::img(path).w(px(20.)).h(px(20.)).rounded(px(4.)).into_any_element()
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
                let file_icon = serde_json::from_str::<FileData>(&item.file_data)
                    .ok()
                    .and_then(|fd| {
                        if fd.files.len() == 1 {
                            fd.files.first().and_then(|fi| cached_file_icon_path(&fi.path, fi.is_dir))
                        } else {
                            None
                        }
                    });
                let effective_icon = file_icon.or(source_icon_path);
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
                                gpui::img(path).w(px(20.)).h(px(20.)).rounded(px(4.)).into_any_element()
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
            DisplayKind::Link => {
                // --- Favicon priority: cache → source app icon → type icon ---
                // --- Only checks local cache — network fetch happens async ---
                // --- in the clipboard detection thread (ensure_favicon_cached). ---
                let domain = url_domain(&full_text);
                let favicon_path = if domain.is_empty() {
                    None
                } else {
                    crate::services::favicon::favicon_cache_path(&domain)
                        .map(std::path::PathBuf::from)
                };
                // --- Source app icon only when the setting is enabled ---
                let app_icon = if show_source {
                    source_icon_path
                } else {
                    None
                };
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
                                gpui::img(path).w(px(20.)).h(px(20.)).rounded(px(4.)).into_any_element()
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
                            gpui::img(path).w(px(20.)).h(px(20.)).rounded(px(4.)).into_any_element()
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

        // --- Right: content area ---
        let content = if editing {
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
        } else if !(note.is_empty() || show_original_on_hover && is_hovered) {
            // Note present, not hovering → display note text (single line, card at min height)
            div().flex_1().flex().items_center().child(
                div()
                    .w_full()
                    .text_size(px(12.))
                    .text_color(text_2)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(note),
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
                    // URL: bold domain + dimmed path
                    let domain = url_domain(&full_text);
                    let path = url_path(&full_text);
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
                                .child(domain),
                        )
                        .child(div().text_size(px(13.)).text_color(text_3).child(path))
                } else {
                    // File system path: show full text as-is
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(text_1)
                                .overflow_hidden()
                                .child(full_text),
                        )
                }
            } else if matches!(content_kind, DisplayKind::Color) {
                div().flex_1().flex().items_center().child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(text_1)
                        .overflow_hidden()
                        .child(full_text),
                )
            } else {
                match content_type {
                    ContentType::Image => {
                        // Show image preview if path is available, otherwise show dimensions
                    if !img_path.is_empty() {
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
                                gpui::img(std::path::Path::new(&img_path))
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
                    } else {
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(22.))
                                    .font_family("iconfont")
                                    .text_color(text_2)
                                    .child("\u{e626}"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(text_3)
                                    .child(format!("{} × {}", img_w, img_h)),
                            )
                    }
                }
                ContentType::PlainText | ContentType::RichText => {
                    // Mask sensitive data for email/phone
                    let content_box = div()
                        .flex_1()
                        .w_full()
                        .text_size(px(12.))
                        .text_color(text_1)
                        .line_height(px(18.))
                        .overflow_hidden();

                    if is_email(&full_text) || is_phone(&full_text) {
                        content_box.child(mask_sensitive_preview(&full_text, &meta_type))
                    } else {
                        let style = TextViewStyle::default()
                            .paragraph_gap(rems(0.25))
                            .heading_font_size(|_level, base| base);
                        match rich_preview(&item) {
                            RichPreview::Html(html) => content_box.child(
                                TextView::html(
                                    ("clipboard-card-html", item.content_hash),
                                    html,
                                    window,
                                    cx,
                                )
                                .style(style)
                                .selectable(false),
                            ),
                            RichPreview::Markdown(markdown) => content_box.child(
                                TextView::markdown(
                                    ("clipboard-card-markdown", item.content_hash),
                                    markdown,
                                    window,
                                    cx,
                                )
                                .style(style)
                                .selectable(false),
                            ),
                            RichPreview::StyledHtml(lines) => {
                                content_box.child(
                                    rich_preview::render_styled_html_lines(lines, text_1),
                                )
                            }
                            RichPreview::Plain(preview) => content_box.child(preview),
                        }
                    }
                }
                ContentType::File => {
                    let file_data: FileData =
                        serde_json::from_str(&item.file_data).unwrap_or_default();
                    let files: Vec<FileInfo> = file_data.files;
                    let multi = files.len() > 1;
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
                                    .child(div().text_size(px(10.)).text_color(text_2).child(ext)),
                            )
                        }))
                }
            }
            } // closes if-else block
        };

        // --- Size label for types that have measurable content ---
        let size_label: Option<String> = match content_type {
            ContentType::PlainText | ContentType::RichText => {
                let count = item.size.max(0) as u64;
                if count > 0 {
                    Some(I18nKey::CardChars.fmt(&[&count.to_string()]))
                } else {
                    None
                }
            }
            ContentType::Image => {
                if img_w > 0 && img_h > 0 {
                    Some(format!("{}×{}", img_w, img_h))
                } else {
                    None
                }
            }
            ContentType::File => {
                let fd: FileData = serde_json::from_str(&item.file_data).unwrap_or_default();
                let count = fd.files.len();
                if count > 1 {
                    Some(I18nKey::CardFilesCount.fmt(&[&count.to_string()]))
                } else if item.size > 0 {
                    Some(format_file_size(item.size))
                } else {
                    None
                }
            }
        };

        // --- Bottom info row: tags → size label → time ---
        let tag_widths = tags
            .iter()
            .map(|tag| info_pill_width(&tag.name, INFO_PILL_PADDING_X, window))
            .collect::<Vec<_>>();
        let mut fixed_widths = Vec::with_capacity(2);
        if let Some(label) = size_label.as_deref() {
            fixed_widths.push(info_pill_width(label, INFO_PILL_PADDING_X, window));
        }
        fixed_widths.push(info_pill_width(
            &time_str,
            TIME_PILL_PADDING_X,
            window,
        ));
        let visible_tag_count = visible_tag_count(
            &tag_widths,
            &fixed_widths,
            info_row_available_width(window),
            |hidden_count| {
                info_pill_width(
                    &format!("+{hidden_count}"),
                    INFO_PILL_PADDING_X,
                    window,
                )
            },
        );
        let hidden_tag_count = tags.len() - visible_tag_count;

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
            .when_some(size_label, |el, label| {
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
        // --- Hide bottom tags/time row during note editing ---
        let card = if !editing {
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

        // --- Hover toolbar (hidden during note editing) ---
        if is_hovered && !editing {
            let toolbar_props = HoverToolbarProps::from_item(&item, selected_count, selected);
            card.child(
                div().absolute().top(px(3.)).right(px(4.)).occlude().child(
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
