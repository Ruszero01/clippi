//! Shared keyword preview and highlight helpers for clipboard cards.

use gpui::*;
use pinyin::ToPinyin;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSegment {
    pub text: String,
    pub highlighted: bool,
}

const DEFAULT_PREVIEW_CHARS: usize = 300;
const AUXILIARY_LEADING_CONTEXT_CHARS: usize = 3;

pub fn contains_match(text: &str, terms: &[String]) -> bool {
    find_next_match(text, terms, 0).is_some()
}

pub fn first_match_range(text: &str, terms: &[String]) -> Option<(usize, usize)> {
    find_next_match(text, terms, 0)
}

pub fn highlight_segments(text: &str, terms: &[String]) -> Vec<HighlightSegment> {
    if text.is_empty() {
        return Vec::new();
    }
    if terms.is_empty() {
        return vec![HighlightSegment {
            text: text.to_string(),
            highlighted: false,
        }];
    }

    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < text.len() {
        let Some((start, end)) = find_next_match(text, terms, pos) else {
            out.push(HighlightSegment {
                text: text[pos..].to_string(),
                highlighted: false,
            });
            break;
        };

        if start > pos {
            out.push(HighlightSegment {
                text: text[pos..start].to_string(),
                highlighted: false,
            });
        }
        out.push(HighlightSegment {
            text: text[start..end].to_string(),
            highlighted: true,
        });
        pos = end;
    }

    out
}

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
    if let Some((match_start, _)) = first_match_range(text, terms) {
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
    if let Some((match_start, _)) = first_match_range(text, terms) {
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

fn find_next_match(text: &str, terms: &[String], start: usize) -> Option<(usize, usize)> {
    let lower_terms: Vec<String> = terms
        .iter()
        .map(|term| term.trim().to_lowercase())
        .filter(|term| !term.is_empty())
        .collect();

    let mut best = find_next_direct_match(text, &lower_terms, start);
    for (span_start, span_end) in pinyin_match_spans(text, &lower_terms) {
        if span_start < start {
            continue;
        }
        best = choose_better_match(best, (span_start, span_end));
    }
    best
}

fn find_next_direct_match(
    text: &str,
    lower_terms: &[String],
    start: usize,
) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for (rel_idx, _) in text[start..].char_indices() {
        let idx = start + rel_idx;
        for term in lower_terms {
            if let Some(end) = term_end_at(text, idx, term) {
                best = choose_better_match(best, (idx, end));
            }
        }
        if best.is_some_and(|(best_start, _)| best_start == idx) {
            break;
        }
    }
    best
}

fn choose_better_match(
    current: Option<(usize, usize)>,
    candidate: (usize, usize),
) -> Option<(usize, usize)> {
    match current {
        Some((best_start, best_end))
            if best_start < candidate.0
                || (best_start == candidate.0 && best_end >= candidate.1) =>
        {
            Some((best_start, best_end))
        }
        _ => Some(candidate),
    }
}

fn term_end_at(text: &str, start: usize, lower_term: &str) -> Option<usize> {
    let mut folded = String::new();
    let mut end = start;

    for ch in text[start..].chars() {
        folded.extend(ch.to_lowercase());
        end += ch.len_utf8();

        if !lower_term.starts_with(&folded) {
            return None;
        }
        if folded == lower_term {
            return Some(end);
        }
    }

    None
}

#[derive(Clone)]
struct PinyinChar {
    byte_start: usize,
    byte_end: usize,
    py_start: usize,
    py_end: usize,
}

fn pinyin_match_spans(text: &str, lower_terms: &[String]) -> Vec<(usize, usize)> {
    let mut full = String::new();
    let mut full_chars = Vec::new();
    let mut initials = String::new();
    let mut initial_chars = Vec::new();

    for (byte_start, ch) in text.char_indices() {
        let Some(py) = ch.to_pinyin() else {
            continue;
        };
        let plain = py.plain().to_lowercase();
        if plain.is_empty() {
            continue;
        }

        let byte_end = byte_start + ch.len_utf8();
        let py_start = full.len();
        full.push_str(&plain);
        let py_end = full.len();
        full_chars.push(PinyinChar {
            byte_start,
            byte_end,
            py_start,
            py_end,
        });

        let initial_start = initials.len();
        if let Some(initial) = plain.chars().next() {
            initials.push(initial);
            initial_chars.push(PinyinChar {
                byte_start,
                byte_end,
                py_start: initial_start,
                py_end: initials.len(),
            });
        }
    }

    let mut spans = Vec::new();
    for term in lower_terms {
        spans.extend(encoded_match_spans(&full, &full_chars, term));
        spans.extend(encoded_match_spans(&initials, &initial_chars, term));
    }
    spans
}

fn encoded_match_spans(encoded: &str, chars: &[PinyinChar], term: &str) -> Vec<(usize, usize)> {
    if encoded.is_empty() || term.is_empty() {
        return Vec::new();
    }

    let mut spans = Vec::new();
    let mut search_from = 0usize;
    while let Some(rel_start) = encoded[search_from..].find(term) {
        let match_start = search_from + rel_start;
        let match_end = match_start + term.len();

        if let (Some(first), Some(last)) = (
            chars.iter().find(|entry| entry.py_end > match_start),
            chars.iter().rev().find(|entry| entry.py_start < match_end),
        ) {
            spans.push((first.byte_start, last.byte_end));
        }

        search_from = match_start + 1;
        if search_from >= encoded.len() {
            break;
        }
    }

    spans
}

fn char_to_byte(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::{focused_window, focused_window_with_leading_context, highlight_segments};

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
    fn highlight_segments_marks_all_terms_case_insensitively() {
        let terms = vec!["railway".to_string(), "ORDER".to_string()];
        let segments = highlight_segments("Railway ticket order", &terms);
        let highlighted: Vec<String> = segments
            .into_iter()
            .filter(|segment| segment.highlighted)
            .map(|segment| segment.text)
            .collect();

        assert_eq!(highlighted, vec!["Railway", "order"]);
    }

    #[test]
    fn highlight_segments_marks_full_pinyin_match() {
        let terms = vec!["gongzuo".to_string()];
        let segments = highlight_segments("工作计划", &terms);
        let highlighted: Vec<String> = segments
            .into_iter()
            .filter(|segment| segment.highlighted)
            .map(|segment| segment.text)
            .collect();

        assert_eq!(highlighted, vec!["工作"]);
    }

    #[test]
    fn highlight_segments_marks_pinyin_initial_match() {
        let terms = vec!["gzjh".to_string()];
        let segments = highlight_segments("工作计划", &terms);
        let highlighted: Vec<String> = segments
            .into_iter()
            .filter(|segment| segment.highlighted)
            .map(|segment| segment.text)
            .collect();

        assert_eq!(highlighted, vec!["工作计划"]);
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
