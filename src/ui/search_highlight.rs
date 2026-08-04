//! Shared keyword preview and highlight helpers for clipboard cards.
//!
//! Matching, pinyin and match-range logic lives in `core::search` — this
//! module only handles preview cropping and GPUI element rendering.

use gpui::*;

use crate::core::search::highlight_segments;

const DEFAULT_PREVIEW_CHARS: usize = 300;
const AUXILIARY_LEADING_CONTEXT_CHARS: usize = 3;

pub fn focused_preview(text: &str, terms: &[String]) -> String {
    focused_window(text, terms, DEFAULT_PREVIEW_CHARS)
}

pub fn focused_window(text: &str, terms: &[String], max_chars: usize) -> String {
    if terms.is_empty() {
        return text.chars().take(max_chars).collect();
    }

    let (start, end) = focused_window_byte_range(text, terms, max_chars);
    let mut out = text[start..end].to_string();

    if start > 0 {
        out.insert_str(0, "...");
    }
    if end < text.len() {
        out.push_str("...");
    }

    out
}

pub fn focused_window_with_leading_context(
    text: &str,
    terms: &[String],
    max_chars: usize,
    leading_context_chars: usize,
) -> String {
    if terms.is_empty() {
        return text.chars().take(max_chars).collect();
    }

    let (start, end) = focused_window_byte_range_with_leading_context(
        text,
        terms,
        max_chars,
        leading_context_chars,
    );
    let mut out = text[start..end].to_string();

    if start > 0 {
        out.insert_str(0, "...");
    }
    if end < text.len() {
        out.push_str("...");
    }

    out
}

pub fn focused_window_byte_range(text: &str, terms: &[String], max_chars: usize) -> (usize, usize) {
    if text.is_empty() || max_chars == 0 {
        return (0, 0);
    }

    let total_chars = text.chars().count();
    if let Some((match_start, _)) = crate::core::search::first_match_range(text, terms) {
        let match_char = text[..match_start].chars().count();
        return focused_char_window(text, total_chars, match_char, max_chars, max_chars / 3);
    }

    if total_chars <= max_chars {
        (0, text.len())
    } else {
        (0, char_to_byte(text, max_chars))
    }
}

fn focused_window_byte_range_with_leading_context(
    text: &str,
    terms: &[String],
    max_chars: usize,
    leading_context_chars: usize,
) -> (usize, usize) {
    if text.is_empty() || max_chars == 0 {
        return (0, 0);
    }

    let total_chars = text.chars().count();
    if let Some((match_start, _)) = crate::core::search::first_match_range(text, terms) {
        let match_char = text[..match_start].chars().count();
        return focused_char_window(
            text,
            total_chars,
            match_char,
            max_chars,
            leading_context_chars,
        );
    }

    if total_chars <= max_chars {
        (0, text.len())
    } else {
        (0, char_to_byte(text, max_chars))
    }
}

fn focused_char_window(
    text: &str,
    total_chars: usize,
    match_char: usize,
    max_chars: usize,
    leading_context_chars: usize,
) -> (usize, usize) {
    let window_start = match_char.saturating_sub(leading_context_chars);
    let window_end = (window_start + max_chars).min(total_chars);

    (
        char_to_byte(text, window_start),
        char_to_byte(text, window_end),
    )
}

pub fn render_highlighted_inline(
    text: String,
    terms: &[String],
    text_color: Rgba,
    highlight_bg: Rgba,
    highlight_text: Rgba,
    font_size: f32,
    font_weight: Option<FontWeight>,
) -> AnyElement {
    let text = focused_preview(&text, terms);
    div()
        .flex()
        .flex_row()
        .children(
            highlight_segments(&text, terms)
                .into_iter()
                .map(move |segment| {
                    let mut el = div()
                        .text_size(px(font_size))
                        .text_color(text_color)
                        .child(segment.text);
                    if let Some(weight) = font_weight {
                        el = el.font_weight(weight);
                    }
                    if segment.highlighted {
                        el = el.text_color(highlight_text).text_bg(highlight_bg);
                    }
                    el
                }),
        )
        .into_any_element()
}

pub fn render_highlighted_auxiliary_inline(
    text: String,
    terms: &[String],
    text_color: Rgba,
    highlight_bg: Rgba,
    highlight_text: Rgba,
    font_size: f32,
) -> AnyElement {
    let text = focused_window_with_leading_context(
        &text,
        terms,
        DEFAULT_PREVIEW_CHARS,
        AUXILIARY_LEADING_CONTEXT_CHARS,
    );
    div()
        .flex()
        .flex_row()
        .children(
            highlight_segments(&text, terms)
                .into_iter()
                .map(move |segment| {
                    let mut el = div()
                        .text_size(px(font_size))
                        .text_color(text_color)
                        .child(segment.text);
                    if segment.highlighted {
                        el = el.text_color(highlight_text).text_bg(highlight_bg);
                    }
                    el
                }),
        )
        .into_any_element()
}

pub fn render_highlighted_block(
    text: String,
    terms: &[String],
    text_color: Rgba,
    highlight_bg: Rgba,
    highlight_text: Rgba,
    font_size: f32,
    line_height: f32,
) -> AnyElement {
    let preview = focused_preview(&text, terms);
    div()
        .flex()
        .flex_col()
        .gap(px(1.))
        .children(preview.lines().map(move |line| {
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .line_height(px(line_height))
                .children(
                    highlight_segments(line, terms)
                        .into_iter()
                        .map(move |segment| {
                            let mut el = div()
                                .text_size(px(font_size))
                                .text_color(text_color)
                                .child(segment.text);
                            if segment.highlighted {
                                el = el.text_color(highlight_text).text_bg(highlight_bg);
                            }
                            el
                        }),
                )
        }))
        .into_any_element()
}

fn char_to_byte(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::{focused_window, focused_window_with_leading_context};

    #[test]
    fn focused_preview_uses_a_single_window_around_match() {
        let text = "aaaaaaaaaaaaaaaa bbbbbbbbbbbbbbbb railway order number 123 cccccccccccccccc";
        let terms = vec!["order".to_string()];

        let preview = focused_window(text, &terms, 24);

        assert!(preview.contains("order"));
        assert!(preview.starts_with("..."));
        assert!(preview.ends_with("..."));
        assert!(!preview.contains('\n'));
    }

    #[test]
    fn focused_preview_centers_long_matching_line() {
        let text = "aaaaaaaaaaaaaaaa railway order bbbbbbbbbbbbbbbb";
        let terms = vec!["railway".to_string()];

        let preview = focused_window(text, &terms, 20);

        assert!(preview.contains("railway"));
        assert!(preview.starts_with("..."));
    }

    #[test]
    fn focused_preview_crops_when_match_is_far_right_even_if_text_fits_limit() {
        let text = "prefix that still fits inside the nominal preview window hotkey.png";
        let terms = vec!["key".to_string()];

        let preview = focused_window(text, &terms, 80);

        assert!(preview.contains("hotkey.png"));
        assert!(preview.starts_with("..."));
    }

    #[test]
    fn focused_preview_with_leading_context_keeps_only_short_prefix_before_match() {
        let text = "C:\\Users\\someone\\Documents\\Projects\\clippi\\docs\\images\\hotkey.png";
        let terms = vec!["key".to_string()];

        let preview = focused_window_with_leading_context(text, &terms, 80, 3);

        assert!(preview.starts_with("...hotkey.png"));
    }

    #[test]
    fn focused_preview_uses_a_window_for_pinyin_matches() {
        let text = "first line with a long prefix 工作计划在这里[nd a long suffix";
        let terms = vec!["gongzuo".to_string()];

        let preview = focused_window(text, &terms, 24);

        assert!(preview.contains("工作"));
        assert!(preview.starts_with("..."));
    }
}
