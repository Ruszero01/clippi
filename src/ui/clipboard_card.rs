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

use crate::core::color::detect_color;
use crate::core::types::{ClipboardItem, ContentType, FileData, FileInfo, format_relative_time, is_email, is_phone, mask_sensitive_preview, url_domain, url_path};

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
                let ext = std::path::Path::new(&fd.files.first().map(|f| f.name.clone()).unwrap_or_default())
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_uppercase();
                if ext.is_empty() { "File".into() } else { ext }
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
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(filename);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let ext_with_dot = if ext.is_empty() { String::new() } else { format!(".{}", ext) };
    (stem.to_string(), ext_with_dot)
}

fn swatch_color(text: &str, fallback: Rgba) -> Rgba {
    detect_color(text)
        .map(|color| {
            rgb(((color.r as u32) << 16) | ((color.g as u32) << 8) | color.b as u32)
        })
        .unwrap_or(fallback)
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
                if ratio <= 50.0 { 68.0 } else if ratio <= 80.0 { 96.0 } else { 128.0 }
            } else {
                96.0
            }
        }
        ContentType::File => {
            let count = item.full_text.lines().count().max(1);
            if count <= 2 { 68.0 } else if count <= 3 { 96.0 } else { 128.0 }
        }
        ContentType::Link | ContentType::Path => 68.0,
        _ => {
            let len = item.full_text.chars().count();
            if len <= 150 { 68.0 } else if len <= 300 { 96.0 } else { 128.0 }
        }
    }
}

#[derive(IntoElement)]
pub struct ClipboardCard {
    item: ClipboardItem,
    selected: bool,
    index: usize,
    selection_order: usize,
    on_click: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,
    on_right_click: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,
}

impl ClipboardCard {
    pub fn new(item: ClipboardItem, selected: bool, index: usize) -> Self {
        Self {
            item,
            selected,
            index,
            selection_order: 0,
            on_click: None,
            on_right_click: None,
        }
    }

    pub fn on_click(mut self, handler: Rc<dyn Fn(usize, &mut Window, &mut App)>) -> Self {
        self.on_click = Some(handler);
        self
    }

    pub fn on_right_click(mut self, handler: Rc<dyn Fn(usize, &mut Window, &mut App)>) -> Self {
        self.on_right_click = Some(handler);
        self
    }
}

impl RenderOnce for ClipboardCard {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Self {
            item,
            selected,
            index,
            selection_order,
            on_click,
            on_right_click,
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
            base.cursor(CursorStyle::PointingHand)
                .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                    handler(index, window, cx);
                })
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
                        .child(
                            div()
                                .w_full()
                                .h_full()
                                .rounded(px(1.))
                                .bg(color_swatch),
                        ),
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
            div()
                .flex_1()
                .flex()
                .items_center()
                .child(
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
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
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
                    let preview: String = if is_email(&full_text) || is_phone(&full_text) {
                        mask_sensitive_preview(&full_text, &meta_type)
                    } else {
                        full_text.chars().take(300).collect()
                    };
                    div()
                        .flex_1()
                        .w_full()
                        .text_size(px(12.))
                        .text_color(text_1)
                        .line_height(px(18.))
                        .overflow_hidden()
                        .child(preview)
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
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(text_3)
                                .child(path),
                        )
                }
                ContentType::Color => {
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(text_1)
                                .overflow_hidden()
                                .child(full_text),
                        )
                }
                ContentType::File => {
                    let file_data: FileData = serde_json::from_str(&item.file_data).unwrap_or_default();
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
                                .flex().flex_row().gap(px(4.)).items_center()
                                .overflow_hidden();
                            let row = if multi {
                                row.child(div().w(px(14.)).h(px(14.)).flex().items_center().justify_center()
                                    .child(div().font_family("iconfont").text_size(px(12.)).text_color(text_3).child(icon)))
                            } else { row };
                            row.child(
                                div().flex_1().flex().flex_row().gap(px(0.)).overflow_hidden()
                                    .child(div().text_size(px(10.)).text_color(text_1).whitespace_nowrap().overflow_hidden().child(stem))
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
                    .child(
                        div()
                            .text_size(px(9.))
                            .text_color(text_2)
                            .child(time_str),
                    ),
            );

        // ── Assemble card ──
        let card = base
            .child(icon_area)
            .child(content)
            .child(bottom_info);

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

        // Selection badge (top-left)
        if selected && selection_order > 0 {
            card.child(
                div()
                    .absolute()
                    .left(px(2.))
                    .top(px(2.))
                    .w(px(12.))
                    .h(px(12.))
                    .rounded(px(3.))
                    .bg(accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_size(px(7.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0xffffff))
                            .child(format!("{}", selection_order)),
                    ),
            )
        } else {
            card
        }
    }
}
