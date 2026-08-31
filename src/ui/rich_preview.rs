//! Shared rich-text preview helpers for clipboard cards and the edit panel.
//!
//! Extracted from `clipboard_card.rs` so the edit panel can reuse HTML color-tag
//! parsing and rendering.

use std::collections::HashMap;

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

/// Tags whose content must never become visible text. Word documents ship
/// `<head>`/`<style>` blocks and Office XML whose text (font names, style
/// rules) would otherwise leak into previews.
fn is_non_visible_tag(tag: &str) -> bool {
    matches!(tag, "head" | "style" | "script" | "title" | "xml")
}

/// Map from a CSS selector (`.class` or bare tag name) to the bounded
/// four-property style parsed for it. Only selectors that yield at least one
/// whitelisted declaration are stored.
pub type EmbeddedStyleMap = HashMap<String, ParsedInlineStyle>;

/// Extract and parse the embedded `<style>` blocks of a raw clipboard HTML
/// payload (Word/WPS style sheets) into a bounded CSS subset (04-spec §3):
///
/// - Case-insensitive `<style>...</style>` extraction with `<!-- -->` and
///   `/* ... */` comment stripping.
/// - Rules split on `{...}`; `@`-rules (`@media`, `@font-face`, …) are
///   skipped together with their whole (nested-brace-aware) block.
/// - Only `.class` and bare tag-name selectors are accepted; combinators,
///   pseudo-selectors, attributes, IDs and compound selectors are ignored.
/// - Only `color`, `font-weight`, `font-style`, `background-color` are
///   parsed; `!important` is stripped without any priority semantics.
/// - Later rules override earlier ones per property for the same selector.
/// - Truncated input (unclosed `<style>` / `{`) never panics and parses
///   everything up to the last complete rule.
///
/// Never fetches external resources (`<link rel=stylesheet>` is ignored).
pub fn parse_embedded_stylesheet(raw_html: &str) -> EmbeddedStyleMap {
    let mut map: EmbeddedStyleMap = HashMap::new();
    let lower = raw_html.to_ascii_lowercase();
    let mut search_from = 0usize;
    while let Some(rel) = lower[search_from..].find("<style") {
        let style_open_start = search_from + rel;
        let Some(after_open_rel) = lower[style_open_start..].find('>') else {
            break;
        };
        let content_start = style_open_start + after_open_rel + 1;
        let Some(close_rel) = lower[content_start..].find("</style") else {
            break;
        };
        let content_end = content_start + close_rel;
        parse_style_content(&raw_html[content_start..content_end], &mut map);
        search_from = content_end;
    }
    map
}

/// Parse a raw clipboard HTML payload (including `<head>`/`<style>` blocks)
/// into styled preview lines, honouring the bounded embedded-stylesheet
/// subset on top of the legacy inline-style parsing.
///
/// Pipeline: `parse_embedded_stylesheet` → normalize the visible fragment →
/// stack-based parse. Style merge order at an opening tag is:
/// `b/strong/i/em` semantic defaults → tag-selector rule → class-selector
/// rules (in attribute order, later wins) → inline `style` attribute, with
/// inline > class > tag > inherited stack. Non-container elements that hit a
/// rule also push/pop stack frames so their text children inherit the style.
/// `</tr>` breaks lines (`</td>` stays inline).
///
/// Returns `None` when no recognisable style applies, so callers fall back
/// to `TextView::html`/plain text.
pub fn parse_styled_html_lines_full(raw_html: &str) -> Option<Vec<Vec<StyledHtmlSpan>>> {
    let style_map = parse_embedded_stylesheet(raw_html);
    let html = normalize_clipboard_html_for_render(raw_html);

    let mut lines: Vec<Vec<StyledHtmlSpan>> = vec![Vec::new()];
    let mut style_stack = vec![(String::new(), ParsedInlineStyle::default())];
    let mut found_style = false;
    let mut idx = 0usize;
    // Nesting depth of non-visible containers (`head`, `style`, `script`,
    // `title`, `xml`). While > 0, no text, styles, or newlines are emitted.
    let mut skip_depth = 0usize;

    while idx < html.len() {
        let rest = &html[idx..];
        if let Some(tag_start_rel) = rest.find('<') {
            let text = &rest[..tag_start_rel];
            if skip_depth == 0 {
                push_html_text(&mut lines, text, &style_stack.last().unwrap().1);
            }
            idx += tag_start_rel;

            if html[idx..].starts_with("<!--") {
                let content_start = idx + "<!--".len();
                let Some(end) = html[content_start..].find("-->") else {
                    break;
                };
                idx = content_start + end + "-->".len();
                continue;
            }
            let Some(tag_end_rel) = html_tag_end(&html[idx..]) else {
                break;
            };
            let tag = &html[idx + 1..idx + tag_end_rel];
            let tag_lower = tag.trim().to_ascii_lowercase();

            let is_closing = tag_lower.starts_with('/');
            let tag_name = tag_lower
                .trim_start_matches('/')
                .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
                .next()
                .unwrap_or("");

            if is_non_visible_tag(tag_name) {
                if is_closing {
                    skip_depth = skip_depth.saturating_sub(1);
                } else {
                    skip_depth += 1;
                }
                idx += tag_end_rel + 1;
                continue;
            }

            if skip_depth > 0 {
                idx += tag_end_rel + 1;
                continue;
            }

            if is_closing {
                // Closing tag — pop the frame pushed by its opening tag.
                if let Some(frame) = style_stack.iter().rposition(|(name, _)| name == tag_name) {
                    style_stack.truncate(frame.max(1));
                }
                if matches!(tag_name, "div" | "p" | "pre" | "tr" | "li") {
                    push_newline(&mut lines);
                }
            } else {
                // Opening tag — merge semantic defaults, then tag/class
                // stylesheet rules (inline > class > tag > inherited stack),
                // then the inline style attribute. Every opening tag pushes a
                // frame so closing tags can always pop it back; transparent
                // frames are identical to their parent and change nothing.
                let semantic = semantic_tag_defaults(tag_name);
                if semantic.has_any_style() {
                    found_style = true;
                }
                let mut merged = merge_rule_style(&style_stack.last().unwrap().1, &semantic);
                if let Some(rule) = style_map.get(tag_name) {
                    merged = merge_rule_style(&merged, rule);
                    found_style = true;
                }
                for cls in extract_classes(tag) {
                    if let Some(rule) = style_map.get(&format!(".{}", cls)) {
                        merged = merge_rule_style(&merged, rule);
                        found_style = true;
                    }
                }
                let inline = parse_inline_style_attr(tag);
                if inline.has_any_style() {
                    found_style = true;
                }
                merged = merge_rule_style(&merged, &inline);
                if !tag_lower.ends_with('/')
                    && !matches!(
                        tag_name,
                        "area"
                            | "base"
                            | "br"
                            | "col"
                            | "embed"
                            | "hr"
                            | "img"
                            | "input"
                            | "link"
                            | "meta"
                            | "param"
                            | "source"
                            | "track"
                            | "wbr"
                    )
                {
                    style_stack.push((tag_name.to_string(), merged));
                }

                if tag_name == "br" {
                    push_newline(&mut lines);
                }
            }

            idx += tag_end_rel + 1;
        } else {
            if skip_depth == 0 {
                push_html_text(&mut lines, rest, &style_stack.last().unwrap().1);
            }
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
/// Delegates to the shared extraction in `core::html_text` so capture and
/// preview always agree on what the visible fragment is (CF_HTML offsets,
/// `<!--StartFragment-->` comments, or `<body>` content).
pub fn normalize_clipboard_html_for_render(html: &str) -> String {
    crate::core::html_text::normalize_clipboard_html(html)
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
        crate::core::search::first_match_range(&text, terms).map(|range| (idx, range))
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
    for segment in crate::core::search::highlight_segments(text, terms) {
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
pub struct ParsedInlineStyle {
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

/// Read a complete HTML attribute, accepting whitespace around `=` and both
/// quoted and unquoted values. Never match `data-class` or text inside quotes.
fn html_attribute<'a>(tag: &'a str, wanted: &str) -> Option<&'a str> {
    let mut rest = tag.trim_start();
    let tag_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    rest = &rest[tag_end..];
    while !rest.is_empty() {
        rest = rest.trim_start();
        let name_end = rest
            .find(|c: char| c.is_whitespace() || c == '=' || c == '/')
            .unwrap_or(rest.len());
        if name_end == 0 {
            break;
        }
        let name = &rest[..name_end];
        rest = rest[name_end..].trim_start();
        if !rest.starts_with('=') {
            continue;
        }
        rest = rest[1..].trim_start();
        let (value, tail) =
            if let Some(quote) = rest.chars().next().filter(|c| matches!(c, '\'' | '"')) {
                let body = &rest[1..];
                let end = body.find(quote)?;
                (&body[..end], &body[end + 1..])
            } else {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                (&rest[..end], &rest[end..])
            };
        if name.eq_ignore_ascii_case(wanted) {
            return Some(value);
        }
        rest = tail;
    }
    None
}

fn html_tag_end(tag: &str) -> Option<usize> {
    let mut quote = None;
    for (index, ch) in tag.char_indices() {
        match (quote, ch) {
            (Some(open), ch) if open == ch => quote = None,
            (None, '\'' | '"') => quote = Some(ch),
            (None, '>') => return Some(index),
            _ => {}
        }
    }
    None
}

fn parse_html_color_attr(tag: &str) -> Option<Rgba> {
    let value = html_attribute(tag, "color")?.trim();
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

/// Default styles applied for semantic tags (`b`/`strong` → bold,
/// `i`/`em` → italic). Lowest precedence in the merge order.
fn semantic_tag_defaults(tag_name: &str) -> ParsedInlineStyle {
    ParsedInlineStyle {
        color: None,
        font_weight: match tag_name {
            "b" | "strong" => Some(FontWeight::BOLD),
            _ => None,
        },
        font_style: match tag_name {
            "i" | "em" => Some(FontStyle::Italic),
            _ => None,
        },
        background_color: None,
    }
}

/// Parse only the inline `style="..."` attribute (plus the classic HTML
/// `color="..."` attribute) of a tag — no semantic tag defaults.
fn parse_inline_style_attr(tag: &str) -> ParsedInlineStyle {
    let mut style = html_attribute(tag, "style")
        .map(parse_style_declarations)
        .unwrap_or_default();
    if style.color.is_none() {
        style.color = parse_html_color_attr(tag);
    }
    style
}

/// Merge `over` onto `base`: properties set by `over` replace `base`'s,
/// unset properties inherit from `base`.
fn merge_rule_style(base: &ParsedInlineStyle, over: &ParsedInlineStyle) -> ParsedInlineStyle {
    ParsedInlineStyle {
        color: over.color.or(base.color),
        font_weight: over.font_weight.or(base.font_weight),
        font_style: over.font_style.or(base.font_style),
        background_color: over.background_color.or(base.background_color),
    }
}

/// Insert or merge one parsed rule into the map. Later rules override
/// earlier ones per property (04-spec §3.6).
fn merge_rule_into(map: &mut EmbeddedStyleMap, key: String, style: ParsedInlineStyle) {
    let existing = map.entry(key).or_default();
    if style.color.is_some() {
        existing.color = style.color;
    }
    if style.font_weight.is_some() {
        existing.font_weight = style.font_weight;
    }
    if style.font_style.is_some() {
        existing.font_style = style.font_style;
    }
    if style.background_color.is_some() {
        existing.background_color = style.background_color;
    }
}

/// Strip `/* ... */` comments entirely and remove the `<!--`/`-->` markers
/// Word/WPS use to *wrap* a stylesheet (the CSS between the markers is kept,
/// per 04-spec §3.1). Unclosed markers drop the rest of the input
/// (truncation safe).
fn strip_style_comments(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let bytes = content.as_bytes();
    let mut idx = 0usize;
    let mut in_comment_wrap = false;
    while idx < bytes.len() {
        if bytes[idx..].starts_with(b"/*") {
            idx = bytes[idx + 2..]
                .windows(2)
                .position(|w| w == b"*/".as_slice())
                .map(|rel| idx + 2 + rel + 2)
                .unwrap_or(bytes.len());
            continue;
        }
        if bytes[idx..].starts_with(b"<!--") {
            in_comment_wrap = true;
            idx += "<!--".len();
            continue;
        }
        if in_comment_wrap && bytes[idx..].starts_with(b"-->") {
            in_comment_wrap = false;
            idx += "-->".len();
            continue;
        }
        let ch_len = content[idx..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&content[idx..idx + ch_len]);
        idx += ch_len;
    }
    out
}

/// Split a style block into `selector { declarations }` rules. `@`-rules are
/// skipped together with their whole (nested-brace-aware) block; truncated
/// input stops parsing at the last complete rule.
fn parse_style_content(content: &str, map: &mut EmbeddedStyleMap) {
    let cleaned = strip_style_comments(content);
    let mut idx = 0usize;
    while idx < cleaned.len() {
        let rest = &cleaned[idx..];
        let Some(open_rel) = rest.find('{') else {
            break;
        };
        let selector = &rest[..open_rel];
        let open_abs = idx + open_rel;
        let Some(close_rel) = matching_brace(&cleaned[open_abs..]) else {
            break;
        };
        let close_abs = open_abs + close_rel;
        let body = &cleaned[open_abs + 1..close_abs];

        let selector = selector.trim();
        if !selector.starts_with('@') {
            for item in selector.split(',') {
                let item = item.trim();
                if let Some((key, style)) = parse_rule_selector(item, body) {
                    merge_rule_into(map, key, style);
                }
            }
        }
        idx = close_abs + 1;
    }
}

/// Position of the `}` closing the brace block that starts at `text[0]`,
/// counting nested braces. `None` when the block is truncated.
fn matching_brace(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, ch) in text.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

/// Validate one comma-separated selector item and parse its declarations.
/// Only plain `.class` or bare tag-name selectors are accepted; combinators,
/// pseudo-selectors, attributes, IDs and compound selectors reject the whole
/// item (04-spec §3.3). Rules without whitelisted declarations are dropped.
fn parse_rule_selector(selector: &str, body: &str) -> Option<(String, ParsedInlineStyle)> {
    let selector = selector.trim();
    if selector.is_empty() || selector.starts_with('@') {
        return None;
    }
    let is_class = selector.starts_with('.');
    let name = if is_class { &selector[1..] } else { selector };
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let style = parse_style_declarations(body);
    if !style.has_any_style() {
        return None;
    }
    let key = if is_class {
        format!(".{name}")
    } else {
        name.to_ascii_lowercase()
    };
    Some((key, style))
}

/// Parse the whitelisted declarations (`color`, `font-weight`, `font-style`,
/// `background-color`) from a rule body, stripping `!important`.
fn parse_style_declarations(body: &str) -> ParsedInlineStyle {
    let mut style = ParsedInlineStyle::default();
    for decl in body.split(';') {
        let mut parts = decl.splitn(2, ':');
        let Some(key) = parts.next() else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let Some(value) = parts.next() else {
            continue;
        };
        let value = strip_important(value);
        match key.as_str() {
            "color" => style.color = parse_css_color(value),
            "font-weight" => style.font_weight = parse_css_font_weight(value),
            "font-style" => style.font_style = parse_css_font_style(value),
            "background-color" => style.background_color = parse_css_color(value),
            _ => {}
        }
    }
    style
}

/// Remove a trailing `!important` marker so the value parses as a normal
/// declaration (no priority semantics, 04-spec §3.5).
fn strip_important(value: &str) -> &str {
    let value = value.trim();
    match value.to_ascii_lowercase().find("!important") {
        Some(pos) => value[..pos].trim(),
        None => value,
    }
}

/// Extract the class names of an opening tag in attribute order, supporting
/// both quoted (`class="a b"`) and unquoted (`class=et2`) forms.
fn extract_classes(tag: &str) -> Vec<&str> {
    html_attribute(tag, "class")
        .unwrap_or("")
        .split_whitespace()
        .collect()
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
        focus_styled_html_lines, highlight_styled_html_lines, normalize_clipboard_html_for_render,
        parse_embedded_stylesheet, parse_styled_html_lines_full, strip_html_links,
    };
    use gpui::{rgb, FontStyle, FontWeight};

    #[test]
    fn styled_html_skips_head_style_and_office_xml() {
        let html = r#"<html><head><style>p { color: red }</style><xml><w:LatentStyles>Times New Roman</w:LatentStyles></xml></head><body><p><span style="color:#ff0000">Visible</span></p></body></html>"#;
        let lines = parse_styled_html_lines_full(html).unwrap();

        let text: String = lines.iter().flatten().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "Visible");
    }

    #[test]
    fn styled_html_keeps_text_between_two_comments() {
        let html = r#"<!--a--><span style="color:#ff0000">正文</span><!--b-->"#;
        let lines = parse_styled_html_lines_full(html).unwrap();

        let text: String = lines.iter().flatten().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "正文");
    }

    #[test]
    fn styled_html_skips_single_simple_comment() {
        let html = r#"<!-- plain comment --><span style="color:#ff0000">Text</span>"#;
        let lines = parse_styled_html_lines_full(html).unwrap();

        let text: String = lines.iter().flatten().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "Text");
    }

    #[test]
    fn styled_html_skips_conditional_comments() {
        let html = r#"<!--[if gte mso 9]><xml><w:LatentStyles>Cambria Math</w:LatentStyles></xml><![endif]--><p><span style="color:#ff0000">正文</span></p>"#;
        let lines = parse_styled_html_lines_full(html).unwrap();

        let text: String = lines.iter().flatten().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "正文");
    }

    #[test]
    fn styled_html_skips_title_and_script() {
        let html = r#"<html><title>Title Text</title><script>var x = 1;</script><body><span style="color:#ff0000">Real</span></body></html>"#;
        let lines = parse_styled_html_lines_full(html).unwrap();

        let text: String = lines.iter().flatten().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "Real");
    }

    #[test]
    fn render_normalization_extracts_word_fragment_without_header() {
        let html = r#"<html xmlns:w="urn:schemas-microsoft-com:office:word"><head><style>p.MsoNormal{font-family:"Times New Roman"}</style></head><body lang=ZH-CN><!--StartFragment--><p><span style='color:#ff0000'>与保险公司协调处理索赔并解决问题。</span></p><!--EndFragment--></body></html>"#;

        let out = normalize_clipboard_html_for_render(html);

        assert!(out.contains("与保险公司协调处理索赔并解决问题。"));
        assert!(!out.contains("Times New Roman"));
        assert!(!out.contains("<head"));
    }

    #[test]
    fn parses_semantic_weight_and_style_without_color() {
        let lines = parse_styled_html_lines_full("<strong>Bold</strong> <em>Italic</em>").unwrap();

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
            parse_styled_html_lines_full(r#"<span style="background-color: yellow">Marked</span>"#)
                .unwrap();

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0][0].text, "Marked");
        assert_eq!(lines[0][0].background_color, Some(rgb(0xFFFF00)));
    }

    #[test]
    fn closing_block_tags_split_lines() {
        let lines = parse_styled_html_lines_full(
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
        let lines = parse_styled_html_lines_full(&stripped).unwrap();

        assert_eq!(stripped, r#"<span style="color: red">Link</span>"#);
        assert_eq!(lines[0][0].text, "Link");
        assert_eq!(lines[0][0].color, Some(rgb(0xFF0000)));
    }

    #[test]
    fn styled_html_highlights_substring_inside_span() {
        let terms = vec!["api".to_string()];
        let lines = parse_styled_html_lines_full(
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
            parse_styled_html_lines_full(r#"<span style="color:#bbbebf"> rapid=false</span>"#)
                .unwrap();
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
        let lines = parse_styled_html_lines_full(
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
        let lines = parse_styled_html_lines_full(
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
        let lines = parse_styled_html_lines_full(
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
        let lines = parse_styled_html_lines_full(
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

    #[test]
    fn class_selector_styles_cell_text() {
        // WPS sample aligned with `html_text::wps_class_style_survives_cf_html_round_trip`.
        let html = r#"<html><head><style>.et2 { color: #ff6600; }</style></head><body><table><tr><td class=et2>测试文本</td></tr></table></body></html>"#;
        let map = parse_embedded_stylesheet(html);
        assert!(map.contains_key(".et2"));
        let lines = parse_styled_html_lines_full(html).unwrap();

        assert_eq!(lines.len(), 1);
        let text: String = lines.iter().flatten().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "测试文本");
        assert!(lines
            .iter()
            .flatten()
            .any(|span| span.color == Some(rgb(0xFF6600))));
    }

    #[test]
    fn inline_style_overrides_class_rule() {
        let html = r#"<html><head><style>.et2 { color: #ff6600; }</style></head><body><table><tr><td class=et2 style="color:#00ff00">测试文本</td></tr></table></body></html>"#;
        let lines = parse_styled_html_lines_full(html).unwrap();

        let text: String = lines.iter().flatten().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "测试文本");
        let span = lines
            .iter()
            .flatten()
            .find(|s| s.text == "测试文本")
            .unwrap();
        assert_eq!(span.color, Some(rgb(0x00FF00)));
    }

    #[test]
    fn void_elements_and_mismatched_closings_do_not_leak_styles() {
        let lines = parse_styled_html_lines_full(
            "<span style='color:red'>red<br>still red<img src=x><meta></span>plain</bogus><b>bold</b>plain again",
        ).unwrap();
        let spans: Vec<_> = lines.iter().flatten().collect();
        for span in &spans {
            match span.text.as_str() {
                "red" | "still red" => assert_eq!(span.color, Some(rgb(0xff0000))),
                "plain" | "plain again" => {
                    assert_eq!(span.color, None);
                    assert_eq!(span.font_weight, None);
                }
                "bold" => assert_eq!(span.font_weight, Some(FontWeight::BOLD)),
                _ => panic!("unexpected text: {}", span.text),
            }
        }
        assert_eq!(spans.len(), 5);
    }

    #[test]
    fn attributes_use_exact_names_and_accept_spaces_quotes_and_important() {
        let html = "<style>.red{color:red}</style><span data-class='red' title='class=red > ignored'>plain</span><span CLASS = 'red' style = 'color: blue !important'>blue</span><font color=green>green</font>";
        let lines = parse_styled_html_lines_full(html).unwrap();
        assert_eq!(lines[0][0].text, "plain");
        assert_eq!(lines[0][0].color, None);
        assert_eq!(lines[0][1].text, "blue");
        assert_eq!(lines[0][1].color, Some(rgb(0x0000ff)));
        assert_eq!(lines[0][2].color, Some(rgb(0x008000)));
    }

    #[test]
    fn unclosed_comments_do_not_display_hidden_text() {
        let lines =
            parse_styled_html_lines_full("<b>visible</b><!-- unfinished >hidden<b>secret</b>")
                .unwrap();
        assert_eq!(
            lines
                .iter()
                .flatten()
                .map(|span| span.text.as_str())
                .collect::<String>(),
            "visible"
        );
    }

    #[test]
    fn external_stylesheet_and_at_rules_ignored() {
        let html = r#"<html><head><link rel=stylesheet href="http://example.com/x.css"><style>@media screen { .et2 { color: #ff6600; } } @font-face { font-family: "X"; src: url(x.woff); }</style></head><body><table><tr><td class=et2>测试文本</td></tr></table></body></html>"#;
        // `@media` inner `.et2` must NOT apply; the external `<link>` is
        // never parsed or fetched.
        let map = parse_embedded_stylesheet(html);
        assert!(map.is_empty());
        assert!(parse_styled_html_lines_full(html).is_none());
    }

    #[test]
    fn complex_selectors_ignored() {
        let html = r#"<html><head><style>p:hover { color: #ff0000; } #id { color: #00ff00; } .a.b { color: #0000ff; } td[colspan] { color: #ffff00; } div p { color: #ff00ff; }</style></head><body><table><tr><td class=a>测试文本</td></tr></table></body></html>"#;
        // Pseudo-selectors, IDs, compound/attribute/descendant selectors
        // reject the whole rule, so nothing applies.
        assert!(parse_styled_html_lines_full(html).is_none());
    }

    #[test]
    fn comment_wrapped_stylesheet_parses() {
        let html = r#"<html><head><style><!-- .et2 { color:#ff6600 } --></style></head><body><table><tr><td class=et2>测试文本</td></tr></table></body></html>"#;
        let lines = parse_styled_html_lines_full(html).unwrap();

        assert!(lines
            .iter()
            .flatten()
            .any(|span| span.color == Some(rgb(0xFF6600))));
    }

    #[test]
    fn important_stripped() {
        let html = r#"<html><head><style>.et2 { color:#ff6600 !important }</style></head><body><table><tr><td class=et2>测试文本</td></tr></table></body></html>"#;
        let lines = parse_styled_html_lines_full(html).unwrap();

        assert!(lines
            .iter()
            .flatten()
            .any(|span| span.color == Some(rgb(0xFF6600))));
    }

    #[test]
    fn truncated_style_block_degrades_gracefully() {
        // Unclosed `<style>` / `{`: no panic, no garbage text, degrades to None.
        let truncated = r#"<html><head><style>.et2 { color: #ff6600"#;
        assert!(parse_styled_html_lines_full(truncated).is_none());

        // A complete rule before a truncated one still applies.
        let partial = r#"<html><head><style>.et2 { color: #ff6600; } .broken { font-weight: bold</style></head><body><table><tr><td class=et2>测试文本</td></tr></table></body></html>"#;
        let lines = parse_styled_html_lines_full(partial).unwrap();
        let text: String = lines.iter().flatten().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "测试文本");
        assert!(lines
            .iter()
            .flatten()
            .any(|span| span.color == Some(rgb(0xFF6600))));
    }

    #[test]
    fn inline_only_html_keeps_text_and_color() {
        let html = r#"<p><span style='color:#ff0000'>带超链接的文字</span></p>"#;
        let lines = parse_styled_html_lines_full(html).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 1);
        assert_eq!(lines[0][0].text, "带超链接的文字");
        assert_eq!(lines[0][0].color, Some(rgb(0xff0000)));
        assert_eq!(lines[0][0].font_weight, None);
        assert_eq!(lines[0][0].font_style, None);
        assert_eq!(lines[0][0].background_color, None);
    }

    #[test]
    fn tr_tag_breaks_line() {
        let html = r#"<html><head><style>.et2 { color: #ff6600; }</style></head><body><table><tr><td class=et2>第一行</td><td>第二格</td></tr><tr><td class=et2>第三行</td></tr></table></body></html>"#;
        let lines = parse_styled_html_lines_full(html).unwrap();

        let rows: Vec<String> = lines
            .iter()
            .map(|line| line.iter().map(|s| s.text.as_str()).collect())
            .collect();
        assert_eq!(rows, vec!["第一行第二格", "第三行"]);
    }
}
