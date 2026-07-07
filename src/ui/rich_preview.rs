//! Shared rich-text preview helpers for clipboard cards and the edit panel.
//!
//! Extracted from `clipboard_card.rs` so the edit panel can reuse HTML color-tag
//! parsing and rendering.

use gpui::*;

use super::search_highlight;

const STYLED_SEARCH_PREVIEW_CHARS: usize = 220;

/// A single styled text span with optional color, font weight, font style,
/// and background color.
#[derive(Clone)]
pub struct StyledHtmlSpan {
    pub text: String,
    pub color: Option<Rgba>,
    pub font_weight: Option<FontWeight>,
    pub font_style: Option<FontStyle>,
    pub background_color: Option<Rgba>,
}

/// Tags whose inline styles we track for inheritance. Closing tags
/// pop the stack so styling is scoped correctly.
fn is_style_container_tag(tag: &str) -> bool {
    matches!(
        tag,
        "span" | "a" | "font" | "b" | "strong" | "i" | "em" | "u"
    )
}

/// Parse an HTML string and extract styled lines with color, font-weight,
/// font-style, and background-color from inline `style` attributes as well
/// as classic HTML `color` attributes (e.g. `<font color="red">`).
///
/// Returns `None` when the HTML contains no recognised styles, so the caller
/// can fall back to `TextView::html()`.
pub fn parse_styled_html_lines(html: &str) -> Option<Vec<Vec<StyledHtmlSpan>>> {
    let html_lower = html.to_ascii_lowercase();
    if !html_lower.contains("style=")
        && !html_lower.contains("color=")
        && !html_lower.contains("<b")
        && !html_lower.contains("<strong")
        && !html_lower.contains("<i")
        && !html_lower.contains("<em")
    {
        return None;
    }

    let mut lines: Vec<Vec<StyledHtmlSpan>> = vec![Vec::new()];
    let mut style_stack: Vec<ParsedInlineStyle> = vec![ParsedInlineStyle::default()];
    let mut found_style = false;
    let mut idx = 0usize;

    while idx < html.len() {
        let rest = &html[idx..];
        if let Some(tag_start_rel) = rest.find('<') {
            let text = &rest[..tag_start_rel];
            push_html_text(&mut lines, text, style_stack.last().unwrap());
            idx += tag_start_rel;

            let Some(tag_end_rel) = html[idx..].find('>') else {
                break;
            };
            let tag = &html[idx + 1..idx + tag_end_rel];
            let tag_lower = tag.trim().to_ascii_lowercase();

            if tag_lower.starts_with('/') {
                // Closing tag — pop style stack for container tags
                let tag_name = tag_lower.trim_start_matches('/');
                if is_style_container_tag(tag_name) && style_stack.len() > 1 {
                    style_stack.pop();
                }
                if tag_lower.starts_with("/div")
                    || tag_lower.starts_with("/p")
                    || tag_lower.starts_with("/pre")
                {
                    push_newline(&mut lines);
                }
            } else {
                // Opening tag — extract pure tag name (first token before space / > / /)
                let tag_name = tag_lower
                    .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
                    .next()
                    .unwrap_or("");
                let tag_style = parse_inline_style(tag_name, tag);
                found_style |= tag_style.has_any_style();

                if is_style_container_tag(tag_name) {
                    // Inherit from parent for properties not set on this tag
                    let parent = style_stack.last().unwrap();
                    let merged = ParsedInlineStyle {
                        color: tag_style.color.or(parent.color),
                        font_weight: tag_style.font_weight.or(parent.font_weight),
                        font_style: tag_style.font_style.or(parent.font_style),
                        background_color: tag_style.background_color.or(parent.background_color),
                    };
                    style_stack.push(merged);
                }
                // Non-container tags: transparent, no stack change.

                if tag_lower.starts_with("br")
                    || tag_lower.starts_with("/div")
                    || tag_lower.starts_with("/p")
                    || tag_lower.starts_with("/pre")
                {
                    push_newline(&mut lines);
                }
            }

            idx += tag_end_rel + 1;
        } else {
            push_html_text(&mut lines, rest, style_stack.last().unwrap());
            break;
        }
    }

    trim_empty_styled_lines(&mut lines);

    if found_style && !lines.is_empty() {
        Some(lines)
    } else {
        None
    }
}

/// Strip `[text](url)` markdown links, keeping only the display text.
///
/// GPUI's `TextView::markdown` renders links with hit areas that extend past
/// `overflow_hidden` boundaries — clicking on the gap *between* cards still
/// navigates to the URL because the link's hit-test area leaks out.
/// Stripping the URL at the source prevents this entirely.
///
/// Images (`![alt](url)`) are intentionally left untouched — they are not
/// interactive links and must keep their URL for the image to render.
///
/// Works at the **character** level to preserve multi-byte UTF-8 sequences
/// (CJK, emoji, etc.). Byte-level iteration would split these into garbled
/// Latin-1 fragments.
pub fn strip_markdown_links(md: &str) -> String {
    let chars: Vec<char> = md.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(md.len());
    let mut i = 0;

    while i < len {
        if chars[i] == '[' {
            let is_image = i > 0 && chars[i - 1] == '!';

            // Find the matching ']'
            let mut j = i + 1;
            let mut depth = 1u32;
            while j < len && depth > 0 {
                match chars[j] {
                    '[' => depth += 1,
                    ']' => depth -= 1,
                    '\n' => break,
                    _ => {}
                }
                j += 1;
            }

            if !is_image && depth == 0 && j < len && chars[j] == '(' {
                // Build the URL string to check if it looks like a real URL
                let url_str: String = chars[j + 1..].iter().collect();
                if looks_like_url(&url_str) {
                    // --- Strip the link: keep [text], drop (url) ---
                    for &ch in &chars[(i + 1)..(j - 1)] {
                        result.push(ch);
                    }
                    // Skip past ](url)
                    i = j + 1;
                    depth = 1;
                    while i < len && depth > 0 {
                        match chars[i] {
                            '(' => depth += 1,
                            ')' => depth -= 1,
                            '\n' => break,
                            _ => {}
                        }
                        i += 1;
                    }
                    continue;
                }
            }

            // Image, false positive, or unmatched — output [ as-is
            result.push('[');
            i += 1;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

/// Strip `<a href="...">text</a>` HTML links, keeping only the inner text.
///
/// Same rationale as `strip_markdown_links` — GPUI's `TextView::html` renders
/// clickable links whose hit areas leak past `overflow_hidden`.
///
/// `<a>` tags that carry inline `style` attributes (e.g. `color`) are
/// converted to `<span>` tags so the styling is preserved.
///
/// Works at the **character** level to preserve multi-byte UTF-8 sequences.
pub fn strip_html_links(html: &str) -> String {
    // ── Pass 1: convert styled <a> tags to <span> so their inline styles ──
    // ── survive the link-stripping pass below.                           ──
    let html = convert_styled_anchors(html);

    let chars: Vec<char> = html.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(html.len());
    let mut i = 0;

    while i < len {
        if chars[i] == '<' && tag_is_a(&chars, i) {
            // Skip the opening <a ...> tag
            while i < len && chars[i] != '>' {
                i += 1;
            }
            if i < len {
                i += 1; // skip >
            }
            // Copy inner text until matching </a>
            let mut depth = 1u32;
            while i < len && depth > 0 {
                if chars[i] == '<' {
                    if tag_is_a(&chars, i) {
                        // Nested <a> — skip opening tag
                        while i < len && chars[i] != '>' {
                            i += 1;
                        }
                        if i < len {
                            i += 1;
                        }
                        depth += 1;
                        continue;
                    }
                    if tag_is_close_a(&chars, i) {
                        // </a> — skip closing tag
                        while i < len && chars[i] != '>' {
                            i += 1;
                        }
                        if i < len {
                            i += 1;
                        }
                        depth -= 1;
                        continue;
                    }
                }
                result.push(chars[i]);
                i += 1;
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

/// Convert `<a ... style="...">` / `</a>` pairs to `<span ...>` / `</span>`
/// when the `<a>` tag has an inline style attribute. Only the style is kept;
/// other attributes (`href`, `target`, etc.) are dropped.
fn convert_styled_anchors(html: &str) -> String {
    if !html.contains("<a ") && !html.contains("<A ") {
        return html.to_string();
    }

    let chars: Vec<char> = html.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(html.len());
    let mut style_stack: Vec<bool> = Vec::new(); // true = this <a> had style
    let mut i = 0;

    while i < len {
        if chars[i] == '<' && tag_is_a(&chars, i) {
            let tag_start = i;
            while i < len && chars[i] != '>' {
                i += 1;
            }
            let tag_end = if i < len { i + 1 } else { i };

            let tag_str: String = chars[tag_start..tag_end].iter().collect();
            if let Some(style) = extract_style_attr(&tag_str) {
                style_stack.push(true);
                out.push_str(&format!("<span style=\"{}\">", style));
            } else {
                style_stack.push(false);
                out.push_str(&tag_str); // keep as-is, will be stripped later
            }
            i = tag_end;
        } else if chars[i] == '<' && tag_is_close_a(&chars, i) {
            while i < len && chars[i] != '>' {
                i += 1;
            }
            if i < len {
                i += 1;
            }
            let had_style = style_stack.pop().unwrap_or(false);
            out.push_str(if had_style { "</span>" } else { "</a>" });
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }

    out
}

/// Extract the value of the `style` attribute from an opening HTML tag.
/// Returns `None` when the tag has no style attribute.
fn extract_style_attr(tag: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let style_pos = lower.find("style=")?;
    let after_style = &tag[style_pos + "style=".len()..];
    let quote = after_style.chars().next()?;
    let value_start = quote.len_utf8();
    let rest = &after_style[value_start..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

/// Check if chars starting at `i` form `<a ` or `<a>` (case-insensitive).
fn tag_is_a(chars: &[char], i: usize) -> bool {
    chars.get(i..).is_some_and(|rest| {
        rest.len() >= 3
            && rest[0] == '<'
            && rest[1].eq_ignore_ascii_case(&'a')
            && (rest[2] == ' ' || rest[2] == '>')
    })
}

/// Check if chars starting at `i` form `</a` (case-insensitive).
fn tag_is_close_a(chars: &[char], i: usize) -> bool {
    chars.get(i..).is_some_and(|rest| {
        rest.len() >= 3 && rest[0] == '<' && rest[1] == '/' && rest[2].eq_ignore_ascii_case(&'a')
    })
}

/// Normalize a clipboard HTML payload for rendering.
///
/// Removes the CF_HTML header / fragment markers so the parser only sees
/// the actual markup.
pub fn normalize_clipboard_html_for_render(html: &str) -> String {
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

/// Render parsed styled-HTML lines as a column of colored rows.
///
/// `fallback` is the default text color used for spans without an explicit
/// color.
pub fn render_styled_html_lines(
    lines: Vec<Vec<StyledHtmlSpan>>,
    fallback: Rgba,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(1.))
        .children(lines.into_iter().map(|line| {
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .children(line.into_iter().map(|span| {
                    let mut d = div()
                        .text_size(px(12.))
                        .font_family("Consolas")
                        .text_color(span.color.unwrap_or(fallback))
                        .font_weight(span.font_weight.unwrap_or_default());
                    if span.font_style == Some(FontStyle::Italic) {
                        d = d.italic();
                    }
                    if let Some(bg) = span.background_color {
                        d = d.text_bg(bg);
                    }
                    d.child(span.text)
                }))
        }))
}

// ── Private helpers ──────────────────────────────────────────────────────

pub fn focus_styled_html_lines(
    lines: Vec<Vec<StyledHtmlSpan>>,
    terms: &[String],
) -> Vec<Vec<StyledHtmlSpan>> {
    if terms.is_empty() {
        return lines;
    }

    let Some((hit_idx, _)) = lines.iter().enumerate().find_map(|(idx, line)| {
        let text: String = line.iter().map(|span| span.text.as_str()).collect();
        search_highlight::first_match_range(&text, terms).map(|range| (idx, range))
    }) else {
        return lines.into_iter().take(6).collect();
    };

    let hit_line = lines[hit_idx].clone();
    vec![focus_long_styled_line(
        hit_line,
        terms,
        STYLED_SEARCH_PREVIEW_CHARS,
    )]
}

pub fn highlight_styled_html_lines(
    lines: Vec<Vec<StyledHtmlSpan>>,
    terms: &[String],
    highlight_bg: Rgba,
    highlight_text: Rgba,
) -> Vec<Vec<StyledHtmlSpan>> {
    if terms.is_empty() {
        return lines;
    }

    lines
        .into_iter()
        .map(|line| {
            let line_text: String = line.iter().map(|span| span.text.as_str()).collect();
            let ranges = highlight_ranges(&line_text, terms);
            split_styled_line_by_ranges(line, &ranges, highlight_bg, highlight_text)
        })
        .collect()
}

pub fn has_highlighted_span(lines: &[Vec<StyledHtmlSpan>], highlight_bg: Rgba) -> bool {
    lines.iter().flatten().any(|span| {
        span.background_color
            .is_some_and(|background| background == highlight_bg)
    })
}

fn highlight_ranges(text: &str, terms: &[String]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut offset = 0usize;
    for segment in search_highlight::highlight_segments(text, terms) {
        let end = offset + segment.text.len();
        if segment.highlighted {
            ranges.push((offset, end));
        }
        offset = end;
    }
    ranges
}

fn focus_long_styled_line(
    line: Vec<StyledHtmlSpan>,
    terms: &[String],
    max_chars: usize,
) -> Vec<StyledHtmlSpan> {
    let line_text: String = line.iter().map(|span| span.text.as_str()).collect();
    let (start_byte, end_byte) =
        search_highlight::focused_window_byte_range(&line_text, terms, max_chars);
    crop_styled_line(line, start_byte, end_byte)
}

fn crop_styled_line(
    line: Vec<StyledHtmlSpan>,
    start_byte: usize,
    end_byte: usize,
) -> Vec<StyledHtmlSpan> {
    if start_byte == 0 && line.iter().map(|span| span.text.len()).sum::<usize>() <= end_byte {
        return line;
    }

    let mut out = Vec::new();
    if start_byte > 0 {
        let mut ellipsis = line.first().cloned().unwrap_or_else(|| StyledHtmlSpan {
            text: String::new(),
            color: None,
            font_weight: None,
            font_style: None,
            background_color: None,
        });
        ellipsis.text = "...".to_string();
        out.push(ellipsis);
    }

    let mut global_start = 0usize;
    let mut total_len = 0usize;
    for span in line {
        let global_end = global_start + span.text.len();
        total_len = global_end;
        let local_start = start_byte.max(global_start).min(global_end) - global_start;
        let local_end = end_byte.max(global_start).min(global_end) - global_start;

        if local_start < local_end {
            out.push(StyledHtmlSpan {
                text: span.text[local_start..local_end].to_string(),
                color: span.color,
                font_weight: span.font_weight,
                font_style: span.font_style,
                background_color: span.background_color,
            });
        }

        global_start = global_end;
    }

    if end_byte < total_len {
        let mut ellipsis = out.last().cloned().unwrap_or_else(|| StyledHtmlSpan {
            text: String::new(),
            color: None,
            font_weight: None,
            font_style: None,
            background_color: None,
        });
        ellipsis.text = "...".to_string();
        out.push(ellipsis);
    }

    out
}

fn split_styled_line_by_ranges(
    line: Vec<StyledHtmlSpan>,
    ranges: &[(usize, usize)],
    highlight_bg: Rgba,
    highlight_text: Rgba,
) -> Vec<StyledHtmlSpan> {
    if ranges.is_empty() {
        return line;
    }

    let mut out = Vec::new();
    let mut global_start = 0usize;
    for span in line {
        let global_end = global_start + span.text.len();
        let mut boundaries = vec![0usize, span.text.len()];

        for &(range_start, range_end) in ranges {
            let start = range_start.max(global_start).min(global_end) - global_start;
            let end = range_end.max(global_start).min(global_end) - global_start;
            if start < end {
                boundaries.push(start);
                boundaries.push(end);
            }
        }
        boundaries.sort_unstable();
        boundaries.dedup();

        for window in boundaries.windows(2) {
            let start = window[0];
            let end = window[1];
            if start == end {
                continue;
            }
            let highlighted = ranges.iter().any(|&(range_start, range_end)| {
                global_start + start >= range_start && global_start + end <= range_end
            });
            out.push(StyledHtmlSpan {
                text: span.text[start..end].to_string(),
                color: if highlighted {
                    Some(highlight_text)
                } else {
                    span.color
                },
                font_weight: span.font_weight,
                font_style: span.font_style,
                background_color: if highlighted {
                    Some(highlight_bg)
                } else {
                    span.background_color
                },
            });
        }

        global_start = global_end;
    }

    out
}

fn push_html_text(lines: &mut Vec<Vec<StyledHtmlSpan>>, text: &str, style: &ParsedInlineStyle) {
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
                    color: style.color,
                    font_weight: style.font_weight,
                    font_style: style.font_style,
                    background_color: style.background_color,
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

/// Parsed inline CSS properties from an HTML tag's style attribute,
/// plus classic HTML presentational attributes (e.g. `<font color="...">`).
#[derive(Clone, Default)]
struct ParsedInlineStyle {
    color: Option<Rgba>,
    font_weight: Option<FontWeight>,
    font_style: Option<FontStyle>,
    background_color: Option<Rgba>,
}

impl ParsedInlineStyle {
    fn has_any_style(&self) -> bool {
        self.color.is_some()
            || self.font_weight.is_some()
            || self.font_style.is_some()
            || self.background_color.is_some()
    }
}

fn parse_inline_style(tag_name: &str, tag: &str) -> ParsedInlineStyle {
    let mut color = None;
    let mut font_weight = match tag_name {
        "b" | "strong" => Some(FontWeight::BOLD),
        _ => None,
    };
    let mut font_style = match tag_name {
        "i" | "em" => Some(FontStyle::Italic),
        _ => None,
    };
    let mut background_color = None;

    // ── CSS style="..." attribute ──
    if let Some(style_body) = extract_style_body(tag) {
        for decl in style_body.split(';') {
            let mut parts = decl.splitn(2, ':');
            let key = match parts.next() {
                Some(k) => k.trim().to_ascii_lowercase(),
                None => continue,
            };
            let value = match parts.next() {
                Some(v) => v.trim(),
                None => continue,
            };
            match key.as_str() {
                "color" => color = parse_css_color(value),
                "font-weight" => font_weight = parse_css_font_weight(value),
                "font-style" => font_style = parse_css_font_style(value),
                "background-color" => background_color = parse_css_color(value),
                _ => {}
            }
        }
    }

    // ── Classic HTML color="..." attribute (e.g. <font color="red">) ──
    if color.is_none() {
        color = parse_html_color_attr(tag);
    }

    ParsedInlineStyle {
        color,
        font_weight,
        font_style,
        background_color,
    }
}

/// Extract the body of a `style="..."` attribute from an HTML tag.
fn extract_style_body(tag: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let style_pos = lower.find("style=")?;
    let after_style = &tag[style_pos + "style=".len()..];
    let quote = after_style.chars().next()?;
    let value_start = quote.len_utf8();
    let rest = &after_style[value_start..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

/// Parse a classic HTML `color="..."` attribute value (named colors and hex).
fn parse_html_color_attr(tag: &str) -> Option<Rgba> {
    let lower = tag.to_ascii_lowercase();
    let color_pos = lower.find("color=")?;
    let after_color = &tag[color_pos + "color=".len()..];
    let quote = after_color.chars().next()?;
    // Only parse quoted attribute values
    let value_start = quote.len_utf8();
    let rest = &after_color[value_start..];
    let end = rest.find(quote)?;
    let value = rest[..end].trim();
    // Try hex first, then named colors
    parse_css_color(value).or_else(|| parse_named_html_color(value))
}

/// Map common HTML named colors to their RGB values.
fn parse_named_html_color(name: &str) -> Option<Rgba> {
    match name.to_ascii_lowercase().as_str() {
        "red" => Some(rgb(0xFF0000)),
        "green" => Some(rgb(0x008000)),
        "blue" => Some(rgb(0x0000FF)),
        "yellow" => Some(rgb(0xFFFF00)),
        "orange" => Some(rgb(0xFFA500)),
        "purple" => Some(rgb(0x800080)),
        "pink" => Some(rgb(0xFFC0CB)),
        "brown" => Some(rgb(0xA52A2A)),
        "black" => Some(rgb(0x000000)),
        "white" => Some(rgb(0xFFFFFF)),
        "gray" | "grey" => Some(rgb(0x808080)),
        "silver" => Some(rgb(0xC0C0C0)),
        "maroon" => Some(rgb(0x800000)),
        "navy" => Some(rgb(0x000080)),
        "teal" => Some(rgb(0x008080)),
        "aqua" | "cyan" => Some(rgb(0x00FFFF)),
        "lime" => Some(rgb(0x00FF00)),
        "fuchsia" | "magenta" => Some(rgb(0xFF00FF)),
        "olive" => Some(rgb(0x808000)),
        _ => None,
    }
}

fn parse_css_font_weight(value: &str) -> Option<FontWeight> {
    match value.trim().to_ascii_lowercase().as_str() {
        "bold" | "700" => Some(FontWeight::BOLD),
        "normal" | "400" => Some(FontWeight::NORMAL),
        "100" => Some(FontWeight::THIN),
        "200" => Some(FontWeight::EXTRA_LIGHT),
        "300" => Some(FontWeight::LIGHT),
        "500" => Some(FontWeight::MEDIUM),
        "600" => Some(FontWeight::SEMIBOLD),
        "800" => Some(FontWeight::EXTRA_BOLD),
        "900" => Some(FontWeight::BLACK),
        _ => None,
    }
}

fn parse_css_font_style(value: &str) -> Option<FontStyle> {
    match value.trim().to_ascii_lowercase().as_str() {
        "italic" => Some(FontStyle::Italic),
        "oblique" => Some(FontStyle::Oblique),
        "normal" => Some(FontStyle::Normal),
        _ => None,
    }
}

fn parse_css_color(value: &str) -> Option<Rgba> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() == 6 {
            return u32::from_str_radix(hex, 16).ok().map(rgb);
        }
    }

    let Some(rgb_values) = value
        .strip_prefix("rgb(")
        .and_then(|s| s.strip_suffix(')'))
        .or_else(|| {
            value
                .strip_prefix("rgba(")
                .and_then(|s| s.strip_suffix(')'))
        })
    else {
        return parse_named_html_color(value);
    };
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

fn parse_cf_html_offset(header: &str, key: &str) -> Option<usize> {
    header.lines().find_map(|line| {
        line.strip_prefix(key)
            .and_then(|value| value.trim().parse::<usize>().ok())
    })
}

/// Heuristic to distinguish real URLs from code patterns like `array[i](fn)`.
///
/// Only treats `(...)` after `[...]` as a markdown link URL when the content
/// starts with a known scheme, avoiding false positives on code syntax.
fn looks_like_url(after_paren: &str) -> bool {
    let s = after_paren.trim_start();
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("ftp://")
        || s.starts_with("mailto:")
        || s.starts_with("tel:")
        || s.starts_with("//")  // protocol-relative
        || s.starts_with('#')   // anchor
        || s.starts_with('/')   // absolute path
        || s.contains("://") // any other protocol
}

#[cfg(test)]
mod tests {
    use super::{
        focus_styled_html_lines, highlight_styled_html_lines, parse_styled_html_lines,
        strip_html_links,
    };
    use gpui::{rgb, FontStyle, FontWeight};

    #[test]
    fn parses_semantic_weight_and_style_without_color() {
        let lines = parse_styled_html_lines("<strong>Bold</strong> <em>Italic</em>").unwrap();

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0][0].text, "Bold");
        assert_eq!(lines[0][0].font_weight, Some(FontWeight::BOLD));
        assert_eq!(lines[0][1].text, " ");
        assert_eq!(lines[0][2].text, "Italic");
        assert_eq!(lines[0][2].font_style, Some(FontStyle::Italic));
    }

    #[test]
    fn parses_background_only_styles() {
        let lines =
            parse_styled_html_lines(r#"<span style="background-color: yellow">Marked</span>"#)
                .unwrap();

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0][0].text, "Marked");
        assert_eq!(lines[0][0].background_color, Some(rgb(0xFFFF00)));
    }

    #[test]
    fn closing_block_tags_split_lines() {
        let lines = parse_styled_html_lines(
            r#"<div><span style="color:#ff0000">One</span></div><div><span style="color:#00ff00">Two</span></div>"#,
        )
        .unwrap();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0][0].text, "One");
        assert_eq!(lines[1][0].text, "Two");
    }

    #[test]
    fn styled_links_keep_style_when_links_are_stripped() {
        let stripped =
            strip_html_links(r#"<a href="https://example.com" style="color: red">Link</a>"#);
        let lines = parse_styled_html_lines(&stripped).unwrap();

        assert_eq!(stripped, r#"<span style="color: red">Link</span>"#);
        assert_eq!(lines[0][0].text, "Link");
        assert_eq!(lines[0][0].color, Some(rgb(0xFF0000)));
    }

    #[test]
    fn styled_html_highlights_substring_inside_span() {
        let terms = vec!["api".to_string()];
        let lines = parse_styled_html_lines(
            r#"<div><span style="color:#bbbebf"> rapid=</span><span style="color:#569cd6">false</span></div>"#,
        )
        .unwrap();
        let lines = focus_styled_html_lines(lines, &terms);
        let lines = highlight_styled_html_lines(lines, &terms, rgb(0x7ECBA3), rgb(0xFFFFFF));
        let highlighted: Vec<String> = lines[0]
            .iter()
            .filter(|span| span.background_color == Some(rgb(0x7ECBA3)))
            .map(|span| span.text.clone())
            .collect();

        assert_eq!(highlighted, vec!["api"]);
    }

    #[test]
    fn styled_html_only_changes_matched_fragment() {
        let terms = vec!["api".to_string()];
        let lines =
            parse_styled_html_lines(r#"<span style="color:#bbbebf"> rapid=false</span>"#).unwrap();
        let lines = highlight_styled_html_lines(lines, &terms, rgb(0x7ECBA3), rgb(0xFFFFFF));
        let spans = &lines[0];

        assert_eq!(
            spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<Vec<_>>(),
            vec![" r", "api", "d=false"]
        );
        assert_eq!(spans[0].color, Some(rgb(0xBBBEBF)));
        assert_eq!(spans[0].background_color, None);
        assert_eq!(spans[1].color, Some(rgb(0xFFFFFF)));
        assert_eq!(spans[1].background_color, Some(rgb(0x7ECBA3)));
        assert_eq!(spans[2].color, Some(rgb(0xBBBEBF)));
        assert_eq!(spans[2].background_color, None);
    }

    #[test]
    fn styled_html_focuses_matching_line_before_highlighting() {
        let terms = vec!["api".to_string()];
        let lines = parse_styled_html_lines(
            r#"<div><span style="color:#bbbebf">first line</span></div><div><span style="color:#bbbebf"> rapid=</span><span style="color:#569cd6">false</span></div><div><span style="color:#bbbebf">last line</span></div>"#,
        )
        .unwrap();
        let lines = focus_styled_html_lines(lines, &terms);
        let lines = highlight_styled_html_lines(lines, &terms, rgb(0x7ECBA3), rgb(0xFFFFFF));
        let visible: Vec<String> = lines
            .iter()
            .map(|line| line.iter().map(|span| span.text.as_str()).collect())
            .collect();
        let highlighted: Vec<String> = lines
            .iter()
            .flatten()
            .filter(|span| span.background_color == Some(rgb(0x7ECBA3)))
            .map(|span| span.text.clone())
            .collect();

        assert!(visible.iter().any(|line| line.contains("rapid=false")));
        assert_eq!(highlighted, vec!["api"]);
    }

    #[test]
    fn styled_html_focuses_inside_long_matching_line() {
        let terms = vec!["api".to_string()];
        let lines = parse_styled_html_lines(
            r#"<div><span style="color:#bbbebf">03:12:05 [INFO] [clipboard] SKIP hash=631733700374619062 owner=43257930 last_owner=43257930 last_hash=6787954947296119038 elapsed=8364ms seq_delta=2 destroyed=false delayed=true same_hash=false owner_window=Visual Studio Code foreground_window=Clippi rapid=</span><span style="color:#569cd6">false</span></div>"#,
        )
        .unwrap();
        let lines = focus_styled_html_lines(lines, &terms);
        let lines = highlight_styled_html_lines(lines, &terms, rgb(0x7ECBA3), rgb(0xFFFFFF));
        let visible: String = lines[0].iter().map(|span| span.text.as_str()).collect();
        let highlighted: Vec<String> = lines[0]
            .iter()
            .filter(|span| span.background_color == Some(rgb(0x7ECBA3)))
            .map(|span| span.text.clone())
            .collect();

        assert!(visible.starts_with("..."));
        assert!(visible.contains("rapid=false"));
        assert_eq!(highlighted, vec!["api"]);
    }

    #[test]
    fn styled_html_focuses_match_near_end_even_when_line_fits_window() {
        let terms = vec!["key".to_string()];
        let lines = parse_styled_html_lines(
            r#"<div><span style="color:#bbbebf">02:29:03 [INFO] [clipboard] PUSH hash=</span><span style="color:#569cd6">14609533420180055385</span><span style="color:#bbbebf"> owner=</span><span style="color:#569cd6">49222182</span><span style="color:#bbbebf"> elapsed=3073ms same_hash=</span><span style="color:#569cd6">false</span><span style="color:#bbbebf"> type=Image text=</span><span style="color:#a5d6ff">"G:\Develop\github\clippi\docs\images\hotkey.png"</span></div>"#,
        )
        .unwrap();
        let lines = focus_styled_html_lines(lines, &terms);
        let lines = highlight_styled_html_lines(lines, &terms, rgb(0x7ECBA3), rgb(0xFFFFFF));
        let visible: String = lines[0].iter().map(|span| span.text.as_str()).collect();
        let highlighted: Vec<String> = lines[0]
            .iter()
            .filter(|span| span.background_color == Some(rgb(0x7ECBA3)))
            .map(|span| span.text.clone())
            .collect();

        assert!(visible.starts_with("..."));
        assert!(visible.contains("hotkey.png"));
        assert_eq!(highlighted, vec!["key"]);
    }

    #[test]
    fn styled_html_highlights_match_across_spans() {
        let terms = vec!["api".to_string()];
        let lines = parse_styled_html_lines(
            r#"<div><span style="color:#bbbebf">a</span><span style="color:#569cd6">pi</span></div>"#,
        )
        .unwrap();
        let lines = highlight_styled_html_lines(lines, &terms, rgb(0x7ECBA3), rgb(0xFFFFFF));
        let highlighted: Vec<String> = lines[0]
            .iter()
            .filter(|span| span.background_color == Some(rgb(0x7ECBA3)))
            .map(|span| span.text.clone())
            .collect();

        assert_eq!(highlighted, vec!["a", "pi"]);
        assert!(lines[0]
            .iter()
            .filter(|span| span.background_color == Some(rgb(0x7ECBA3)))
            .all(|span| span.color == Some(rgb(0xFFFFFF))));
    }
}
