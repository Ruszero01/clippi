//! Shared rich-text preview helpers for clipboard cards and the edit panel.
//!
//! Extracted from `clipboard_card.rs` so the edit panel can reuse HTML color-tag
//! parsing and rendering.

use gpui::*;

/// A single styled text span with optional color.
#[derive(Clone)]
pub struct StyledHtmlSpan {
    pub text: String,
    pub color: Option<Rgba>,
}

/// Parse an HTML string and extract colored `<span style="color:...">` lines.
///
/// Returns `None` when the HTML contains no color styles, so the caller can
/// fall back to `TextView::html()`.
pub fn parse_styled_html_lines(html: &str) -> Option<Vec<Vec<StyledHtmlSpan>>> {
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
                    div()
                        .text_size(px(12.))
                        .font_family("Consolas")
                        .text_color(span.color.unwrap_or(fallback))
                        .child(span.text)
                }))
        }))
}

// ── Private helpers ──────────────────────────────────────────────────────

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
        Some(rgb(
            ((channels[0] as u32) << 16) | ((channels[1] as u32) << 8) | channels[2] as u32,
        ))
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
