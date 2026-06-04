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

use gpui::*;
use gpui_component::input::InputState;
use gpui_component::text::{TextView, TextViewStyle};

use crate::core::color::detect_color;
use crate::core::types::{
    format_relative_time, is_email, is_phone, mask_sensitive_preview, url_domain, url_path,
    ClipboardItem, ContentType, FileData, FileInfo, RichData,
};

use super::hover_toolbar::{HoverToolbar, HoverToolbarProps};

/// Get a content type iconfont glyph for display.
fn type_icon(item: &ClipboardItem) -> &'static str {
    // Use meta-type specific icons for email
    if item.meta_type == "email" {
        return "\u{e604}";
    }
    match item.content_type {
        ContentType::PlainText => "\u{e60e}",
        ContentType::RichText => "\u{e6ae}",
        ContentType::Image => "\u{e626}",
        ContentType::File => "\u{e646}",
        ContentType::Link => "\u{e6d7}",
        ContentType::Path => "\u{e60f}",
        ContentType::Color => "\u{e610}",
    }
}

/// Get a content type display label.
fn type_label(item: &ClipboardItem) -> String {
    if item.meta_type == "email" {
        return "Email".into();
    }
    if item.meta_type == "phone" {
        return "Phone".into();
    }
    match item.content_type {
        ContentType::PlainText => "Text".into(),
        ContentType::RichText => "RTF".into(),
        ContentType::Image => "Image".into(),
        ContentType::File => {
            let fd: FileData = serde_json::from_str(&item.file_data).unwrap_or_default();
            if fd.files.len() <= 1 {
                // Single file: show extension label
                let ext = std::path::Path::new(
                    &fd.files.first().map(|f| f.name.clone()).unwrap_or_default(),
                )
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_uppercase();
                if ext.is_empty() {
                    "File".into()
                } else {
                    ext
                }
            } else {
                format!("{} Files", fd.files.len())
            }
        }
        ContentType::Link => "URL".into(),
        ContentType::Path => "Path".into(),
        ContentType::Color => "Color".into(),
    }
}

fn color_from_hex(hex: &str, fallback: Rgba) -> Rgba {
    let hex = hex.trim().trim_start_matches('#');
    if hex.len() == 6 {
        if let Ok(value) = u32::from_str_radix(hex, 16) {
            return rgb(value);
        }
    }
    fallback
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

enum RichPreview {
    StyledHtml(Vec<Vec<StyledHtmlSpan>>),
    Html(String),
    Markdown(String),
    Plain(String),
}

#[derive(Clone)]
struct StyledHtmlSpan {
    text: String,
    color: Option<Rgba>,
}

fn rich_preview(item: &ClipboardItem) -> RichPreview {
    if item.content_type == ContentType::RichText {
        let rich = RichData::from_json(&item.rich_data);
        if let Some(html) = rich.html.filter(|html| !html.trim().is_empty()) {
            let html = normalize_clipboard_html_for_render(&html);
            if let Some(lines) = parse_styled_html_lines(&html) {
                return RichPreview::StyledHtml(lines);
            }
            return RichPreview::Html(html);
        }
        if is_markdown_like(&item.full_text) {
            return RichPreview::Markdown(item.full_text.clone());
        }
        if let Some(rtf) = rich.rtf.filter(|rtf| !rtf.trim().is_empty()) {
            return RichPreview::Markdown(rtf_to_plain_text(&rtf));
        }
    } else if is_markdown_like(&item.full_text) {
        return RichPreview::Markdown(item.full_text.clone());
    }

    RichPreview::Plain(item.full_text.chars().take(300).collect())
}

fn parse_styled_html_lines(html: &str) -> Option<Vec<Vec<StyledHtmlSpan>>> {
    if !html.contains("style=") || !html.contains("color") {
        return None;
    }

    let mut lines: Vec<Vec<StyledHtmlSpan>> = vec![Vec::new()];
    let mut color_stack: Vec<Option<Rgba>> = vec![None];
    let mut found_color = false;
    let mut idx = 0usize;

    while idx < html.len() {
        let rest = &html[idx..];
        if let Some(tag_start_rel) = rest.find('<') {
            let text = &rest[..tag_start_rel];
            push_html_text(&mut lines, text, *color_stack.last().unwrap_or(&None));
            idx += tag_start_rel;

            let Some(tag_end_rel) = html[idx..].find('>') else {
                break;
            };
            let tag = &html[idx + 1..idx + tag_end_rel];
            let tag_lower = tag.trim().to_ascii_lowercase();

            if tag_lower.starts_with("span") {
                let color = parse_style_color(tag);
                found_color |= color.is_some();
                color_stack.push(color.or_else(|| *color_stack.last().unwrap_or(&None)));
            } else if tag_lower.starts_with("/span") {
                if color_stack.len() > 1 {
                    color_stack.pop();
                }
            } else if tag_lower.starts_with("br")
                || tag_lower.starts_with("/div")
                || tag_lower.starts_with("/p")
                || tag_lower.starts_with("/pre")
            {
                push_newline(&mut lines);
            }

            idx += tag_end_rel + 1;
        } else {
            push_html_text(&mut lines, rest, *color_stack.last().unwrap_or(&None));
            break;
        }
    }

    trim_empty_styled_lines(&mut lines);

    if found_color && !lines.is_empty() {
        Some(lines)
    } else {
        None
    }
}

fn push_html_text(lines: &mut Vec<Vec<StyledHtmlSpan>>, text: &str, color: Option<Rgba>) {
    let decoded = decode_html_text(text);
    if decoded.is_empty() {
        return;
    }

    for (line_idx, part) in decoded.split('\n').enumerate() {
        if line_idx > 0 {
            push_newline(lines);
        }
        if lines.len() == 1 && lines[0].is_empty() && part.trim().is_empty() {
            continue;
        }
        if !part.is_empty() {
            lines
                .last_mut()
                .expect("styled html lines should contain a row")
                .push(StyledHtmlSpan {
                    text: part.to_string(),
                    color,
                });
        }
    }
}

fn push_newline(lines: &mut Vec<Vec<StyledHtmlSpan>>) {
    if lines.last().is_some_and(|line| line.is_empty()) {
        return;
    }
    lines.push(Vec::new());
}

fn trim_empty_styled_lines(lines: &mut Vec<Vec<StyledHtmlSpan>>) {
    while lines
        .first()
        .is_some_and(|line| line.iter().all(|span| span.text.trim().is_empty()))
    {
        lines.remove(0);
    }
    while lines
        .last()
        .is_some_and(|line| line.iter().all(|span| span.text.trim().is_empty()))
    {
        lines.pop();
    }
}

fn decode_html_text(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn parse_style_color(tag: &str) -> Option<Rgba> {
    let style_pos = tag.find("style=")?;
    let style = &tag[style_pos + "style=".len()..];
    let quote = style.chars().next()?;
    let style_body = if quote == '"' || quote == '\'' {
        let rest = &style[quote.len_utf8()..];
        let end = rest.find(quote)?;
        &rest[..end]
    } else {
        style.split_whitespace().next().unwrap_or("")
    };

    style_body.split(';').find_map(|decl| {
        let mut parts = decl.splitn(2, ':');
        let key = parts.next()?.trim().to_ascii_lowercase();
        let value = parts.next()?.trim();
        if key == "color" {
            parse_css_color(value)
        } else {
            None
        }
    })
}

fn parse_css_color(value: &str) -> Option<Rgba> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() == 6 {
            return u32::from_str_radix(hex, 16).ok().map(rgb);
        }
    }

    let rgb_values = value
        .strip_prefix("rgb(")
        .and_then(|s| s.strip_suffix(')'))
        .or_else(|| {
            value
                .strip_prefix("rgba(")
                .and_then(|s| s.strip_suffix(')'))
        })?;
    let channels: Vec<u8> = rgb_values
        .split(',')
        .take(3)
        .filter_map(|part| part.trim().parse::<u8>().ok())
        .collect();
    if channels.len() == 3 {
        Some(rgb(((channels[0] as u32) << 16)
            | ((channels[1] as u32) << 8)
            | channels[2] as u32))
    } else {
        None
    }
}

fn normalize_clipboard_html_for_render(html: &str) -> String {
    let Some(header_end) = html.find("<html").or_else(|| html.find("<!DOCTYPE")) else {
        return html.to_string();
    };

    let header = &html[..header_end];
    if !header.lines().any(|line| line.starts_with("Version:")) {
        return html.to_string();
    }

    if let (Some(start), Some(end)) = (
        parse_cf_html_offset(header, "StartFragment:"),
        parse_cf_html_offset(header, "EndFragment:"),
    ) {
        if start < end && end <= html.len() {
            return String::from_utf8_lossy(&html.as_bytes()[start..end])
                .trim()
                .to_string();
        }
    }

    html[header_end..]
        .replace("<!--StartFragment-->", "")
        .replace("<!--EndFragment-->", "")
        .trim()
        .to_string()
}

fn parse_cf_html_offset(header: &str, key: &str) -> Option<usize> {
    header.lines().find_map(|line| {
        line.strip_prefix(key)
            .and_then(|value| value.trim().parse::<usize>().ok())
    })
}

fn is_markdown_like(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 3 {
        return false;
    }

    trimmed.contains("**")
        || trimmed.contains("__")
        || trimmed.contains("```")
        || (trimmed.contains('[') && trimmed.contains("]("))
        || trimmed.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("# ")
                || line.starts_with("## ")
                || line.starts_with("- ")
                || line.starts_with("* ")
                || line.starts_with("> ")
        })
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
        ContentType::Link | ContentType::Path => 68.0,
        _ => {
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
    selection_order: usize,
    on_click: Option<Rc<dyn Fn(usize, Modifiers, &mut Window, &mut App)>>,
    on_right_click: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,
    is_hovered: bool,
    selected_count: usize,
    on_toolbar_action: Option<Rc<dyn Fn(&str, &mut Window, &mut App)>>,
    on_double_click: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,
    /// Whether this card is in note-editing mode (shows inline editor).
    editing: bool,
    /// Shared InputState from ClipboardListView (only Some when editing is true).
    note_input: Option<Entity<InputState>>,
    /// Called when note editing is committed (Enter / confirm button).
    on_commit_note: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}

impl ClipboardCard {
    pub fn new(item: Rc<ClipboardItem>, selected: bool, index: usize) -> Self {
        Self {
            item,
            selected,
            index,
            selection_order: 0,
            on_click: None,
            on_right_click: None,
            is_hovered: false,
            selected_count: 0,
            on_toolbar_action: None,
            on_double_click: None,
            editing: false,
            note_input: None,
            on_commit_note: None,
        }
    }

    pub fn on_click(
        mut self,
        handler: Rc<dyn Fn(usize, Modifiers, &mut Window, &mut App)>,
    ) -> Self {
        self.on_click = Some(handler);
        self
    }

    pub fn on_right_click(mut self, handler: Rc<dyn Fn(usize, &mut Window, &mut App)>) -> Self {
        self.on_right_click = Some(handler);
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

    pub fn on_double_click(
        mut self,
        handler: Rc<dyn Fn(usize, &mut Window, &mut App)>,
    ) -> Self {
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
    pub fn on_commit_note(
        mut self,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_commit_note = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ClipboardCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            item,
            selected,
            index,
            selection_order,
            on_click,
            on_right_click,
            is_hovered,
            selected_count,
            on_toolbar_action,
            on_double_click,
            editing: _,
            note_input: _,
            on_commit_note: _,
        } = self;

        let surface = rgb(0x232425);
        let divider = rgb(0x2b2c2d);
        let accent = rgb(0x7ecba3);
        let fav_color = rgb(0xd8a155);
        let tag_bg = rgb(0x2c2e2f);
        let tag_text = rgb(0xddf5e4);
        let text_1 = rgb(0xeaebec);
        let text_2 = rgb(0x919496);
        let text_3 = rgb(0x5f6264);
        let pill_bg = rgba(0x232425e8);
        let pill_border = rgba(0xffffff20);
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
        let label = type_label(&item);
        let color_swatch = swatch_color(&full_text, accent);

        let border_color = if selected { accent } else { divider };

        let base = div()
            .relative()
            .w_full()
            .h_full()
            .overflow_hidden()
            .bg(surface)
            .border(px(1.))
            .border_color(border_color)
            .rounded(px(10.))
            .shadow_md()
            .flex()
            .flex_row()
            .p(px(10.))
            .gap(px(10.));

        // Wire click handler
        let base = if let Some(handler) = on_click {
            let double_click_handler = on_double_click.clone();
            base.cursor(CursorStyle::PointingHand).on_mouse_down(
                MouseButton::Left,
                move |ev, window, cx| {
                    if ev.click_count == 2 {
                        // Double-click → paste
                        if let Some(ref dbl_handler) = double_click_handler {
                            dbl_handler(index, window, cx);
                        }
                    } else {
                        // Single click → select
                        handler(index, ev.modifiers, window, cx);
                    }
                },
            )
        } else {
            base
        };

        // Wire right-click handler
        let base = if let Some(handler) = on_right_click {
            base.on_mouse_down(MouseButton::Right, move |_ev, window, cx| {
                handler(index, window, cx);
            })
        } else {
            base
        };

        // ── Left: icon area (top-aligned with content) ──
        let icon_area = match content_type {
            ContentType::Color => div()
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
                        .bg(tag_bg)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .w(px(20.))
                                .h(px(20.))
                                .rounded(px(4.))
                                .bg(color_swatch)
                                .border(px(1.))
                                .border_color(rgba(0xffffff20)),
                        ),
                )
                .child(
                    div()
                        .w(px(36.))
                        .h(px(14.))
                        .rounded(px(3.))
                        .bg(color_swatch)
                        .border(px(1.))
                        .border_color(rgba(0xffffff20))
                        .p(px(2.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().w_full().h_full().rounded(px(1.)).bg(color_swatch)),
                ),
            ContentType::Image => div()
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
                        .bg(tag_bg)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_family("iconfont")
                                .text_color(tag_text)
                                .child(icon.to_string()),
                        ),
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
                        .bg(tag_bg)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_family("iconfont")
                                .text_color(tag_text)
                                .child(icon.to_string()),
                        ),
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

        // ── Right: content area ──
        let content = if !note.is_empty() {
            div().flex_1().flex().items_center().child(
                div()
                    .w_full()
                    .text_size(px(12.))
                    .text_color(text_2)
                    .overflow_hidden()
                    .child(note),
            )
        } else {
            match content_type {
                ContentType::Image => {
                    // Show image preview if path is available, otherwise show dimensions
                    if !img_path.is_empty() {
                        div().flex_1().flex().items_center().justify_center().child(
                            gpui::img(std::path::Path::new(&img_path))
                                .w_full()
                                .h(px(48.))
                                .object_fit(ObjectFit::Cover),
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
                                content_box.child(div().flex().flex_col().gap(px(1.)).children(
                                    lines.into_iter().take(5).map(|line| {
                                        div().flex().flex_row().overflow_hidden().children(
                                            line.into_iter().map(|span| {
                                                div()
                                                    .text_size(px(12.))
                                                    .font_family("Consolas")
                                                    .text_color(span.color.unwrap_or(text_1))
                                                    .whitespace_nowrap()
                                                    .child(span.text)
                                            }),
                                        )
                                    }),
                                ))
                            }
                            RichPreview::Plain(preview) => content_box.child(preview),
                        }
                    }
                }
                ContentType::Link | ContentType::Path => {
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
                }
                ContentType::Color => div().flex_1().flex().items_center().child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(text_1)
                        .overflow_hidden()
                        .child(full_text),
                ),
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
                        .children(files.iter().take(3).map(|fi| {
                            let (stem, ext) = if fi.is_dir {
                                (fi.name.clone(), String::new())
                            } else {
                                split_name_ext(&fi.name)
                            };
                            let icon = if fi.is_dir { "\u{e60f}" } else { "\u{e646}" };
                            let row = div()
                                .rounded(px(4.))
                                .bg(rgb(0x2b2c2d))
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
                                        .child(
                                            div()
                                                .font_family("iconfont")
                                                .text_size(px(12.))
                                                .text_color(text_3)
                                                .child(icon),
                                        ),
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
                                    .child(div().text_size(px(10.)).text_color(text_3).child(ext)),
                            )
                        }))
                }
            }
        };

        // ── Bottom info row ──
        let bottom_info = div()
            .absolute()
            .right(px(10.))
            .bottom(px(6.))
            .h(px(18.))
            .flex()
            .flex_row()
            .gap(px(4.))
            .items_center()
            .children(tags.iter().take(3).map(|tag| {
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

        // ── Assemble card ──
        let card = base.child(icon_area).child(content).child(bottom_info);

        // Fav indicator bar (left edge, scales with card height)
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
        // Only shown when multi-selecting (>1).
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

        // ── Hover toolbar ──
        if is_hovered {
            let toolbar_props =
                HoverToolbarProps::from_item(&item, selected_count, selected);
            card.child(
                div()
                    .absolute()
                    .top(px(3.))
                    .right(px(4.))
                    .occlude()
                    .child(HoverToolbar::new(toolbar_props).on_action(
                        move |action, _window, cx| {
                            if let Some(ref handler) = on_toolbar_action {
                                handler(action, _window, cx);
                            }
                        },
                    )),
            )
        } else {
            card
        }
    }
}
