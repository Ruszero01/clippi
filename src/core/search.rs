//! Shared keyword search primitives — the single source of truth for
//! tokenization, text matching and match-range computation.
//!
//! Used by both the clipboard history search (`AppState`) and the transfer
//! station name search, and by the highlight renderer (`ui::search_highlight`)
//! so filtering and highlighting can never drift apart. Pure logic, no GPUI.

use pinyin::ToPinyin;

/// Split user-entered search text into normalized keyword terms.
///
/// Whitespace separates terms. All returned terms are non-empty and unique,
/// preserving the user's first-seen order.
pub fn split_keyword_terms(keyword: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for term in keyword.split_whitespace() {
        if !terms.iter().any(|existing| existing == term) {
            terms.push(term.to_string());
        }
    }
    terms
}

/// Whether `text` matches a single keyword term: case-insensitive substring,
/// full pinyin or pinyin initials.
pub fn text_matches_term(text: &str, term: &str) -> bool {
    find_next_match(text, &[term], 0).is_some()
}

/// Whether `text` matches every keyword term (AND semantics).
/// The text's pinyin forms are encoded once and shared by all terms.
pub fn text_matches_all_terms(text: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let index = PinyinIndex::encode(text);
    terms.iter().all(|term| {
        let term = term.trim().to_lowercase();
        !term.is_empty()
            && (find_next_direct_match(text, std::slice::from_ref(&term), 0).is_some()
                || index.matches(&term))
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSegment {
    pub text: String,
    pub highlighted: bool,
}

/// Whether any keyword term matches anywhere in `text`.
pub fn contains_match(text: &str, terms: &[String]) -> bool {
    find_next_match(text, terms, 0).is_some()
}

/// Byte range of the first keyword match in `text`, if any.
pub fn first_match_range(text: &str, terms: &[String]) -> Option<(usize, usize)> {
    find_next_match(text, terms, 0)
}

/// All non-overlapping match ranges (byte offsets) in `text`, covering every
/// keyword term — the exact ranges `highlight_segments` renders. Used by
/// callers that need to re-split matches across display boundaries (e.g. the
/// file-name stem/extension split) while keeping one set of match semantics.
pub fn match_ranges(text: &str, terms: &[String]) -> Vec<(usize, usize)> {
    if text.is_empty() || terms.is_empty() {
        return Vec::new();
    }
    let index = PinyinIndex::encode(text);
    let lower_terms = lowercase_terms(terms);
    let mut ranges = Vec::new();
    let mut pos = 0usize;
    while pos < text.len() {
        let Some((start, end)) = find_next_match_with(text, &lower_terms, &index, pos) else {
            break;
        };
        ranges.push((start, end));
        pos = end;
    }
    ranges
}

/// Split `text` into alternating non-highlighted / highlighted segments for
/// every keyword match. Highlight ranges cover the original character spans,
/// including pinyin hits (the matched Chinese characters).
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

    let index = PinyinIndex::encode(text);
    let lower_terms = lowercase_terms(terms);
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < text.len() {
        let Some((start, end)) = find_next_match_with(text, &lower_terms, &index, pos) else {
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

fn lowercase_terms<T: AsRef<str>>(terms: &[T]) -> Vec<String> {
    terms
        .iter()
        .map(|term| term.as_ref().trim().to_lowercase())
        .filter(|term| !term.is_empty())
        .collect()
}

fn find_next_match<T: AsRef<str>>(text: &str, terms: &[T], start: usize) -> Option<(usize, usize)> {
    let lower_terms = lowercase_terms(terms);
    find_next_match_with(text, &lower_terms, &PinyinIndex::encode(text), start)
}

/// Core match with pre-computed lowercase terms and a pre-encoded pinyin
/// index, so a single text is encoded once across repeated lookups.
fn find_next_match_with(
    text: &str,
    lower_terms: &[String],
    index: &PinyinIndex,
    start: usize,
) -> Option<(usize, usize)> {
    let mut best = find_next_direct_match(text, lower_terms, start);
    for (span_start, span_end) in pinyin_match_spans(index, lower_terms) {
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

/// Pre-encoded pinyin forms of one text (full pinyin + initials), so a
/// multi-keyword match encodes the text exactly once.
struct PinyinIndex {
    full: String,
    full_chars: Vec<PinyinChar>,
    initials: String,
    initial_chars: Vec<PinyinChar>,
}

impl PinyinIndex {
    fn encode(text: &str) -> Self {
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

        Self {
            full,
            full_chars,
            initials,
            initial_chars,
        }
    }

    /// Any pinyin span (full or initials) matching `term`.
    fn matches(&self, term: &str) -> bool {
        self.full.contains(term) || self.initials.contains(term)
    }

    /// All pinyin spans (full + initials) matching `term`, mapped back to
    /// byte offsets in the original text.
    fn spans(&self, term: &str) -> Vec<(usize, usize)> {
        let mut spans = encoded_match_spans(&self.full, &self.full_chars, term);
        spans.extend(encoded_match_spans(
            &self.initials,
            &self.initial_chars,
            term,
        ));
        spans
    }
}

fn pinyin_match_spans(index: &PinyinIndex, lower_terms: &[String]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    for term in lower_terms {
        spans.extend(index.spans(term));
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

#[cfg(test)]
mod tests {
    use super::{
        highlight_segments, match_ranges, split_keyword_terms, text_matches_all_terms,
        text_matches_term,
    };

    #[test]
    fn keyword_terms_split_on_whitespace_and_deduplicate() {
        assert_eq!(
            split_keyword_terms("  railway   order railway\tseat  "),
            vec!["railway", "order", "seat"]
        );
    }

    #[test]
    fn text_matches_term_is_case_insensitive() {
        assert!(text_matches_term("Railway-Order.pdf", "RAIL"));
        assert!(!text_matches_term("report.pdf", "rail"));
    }

    #[test]
    fn text_matches_all_terms_requires_every_term() {
        assert!(text_matches_all_terms(
            "Railway-Order.pdf",
            &["rail".to_string(), "order".to_string()]
        ));
        assert!(!text_matches_all_terms(
            "工作计划.docx",
            &["工作".to_string(), "xlsx".to_string()]
        ));
        assert!(text_matches_all_terms("report.pdf", &[]));
    }

    #[test]
    fn text_matches_term_supports_full_pinyin_and_initials() {
        assert!(text_matches_term("工作计划", "gongzuo"));
        assert!(text_matches_term("工作计划", "gzjh"));
        assert!(text_matches_term("工作计划", "工作"));
        assert!(!text_matches_term("工作计划", "zhanshi"));
    }

    #[test]
    fn text_matches_term_ignores_non_matching_plain_terms() {
        assert!(!text_matches_term("报告.pdf", "DESKTOP-A"));
    }

    #[test]
    fn match_ranges_covers_full_name_single_terms_and_multi_word() {
        // A complete file name as one search term matches across the whole name.
        assert_eq!(
            match_ranges("report.pdf", &["report.pdf".to_string()]),
            vec![(0, 10)]
        );
        assert_eq!(
            match_ranges("report.pdf", &["report".to_string()]),
            vec![(0, 6)]
        );
        assert_eq!(
            match_ranges("report.pdf", &["pdf".to_string()]),
            vec![(7, 10)]
        );
        // Multiple terms hitting stem and extension separately.
        assert_eq!(
            match_ranges("report.pdf", &["report".to_string(), "pdf".to_string()]),
            vec![(0, 6), (7, 10)]
        );
        // A term spanning the stem/extension boundary is kept as one range.
        assert_eq!(
            match_ranges("report.pdf", &["report.p".to_string()]),
            vec![(0, 8)]
        );
    }

    #[test]
    fn same_start_prefers_longer_match_over_shorter_candidate() {
        // Two terms starting at the same offset: the longer match wins.
        assert_eq!(
            match_ranges("abcdx", &["abc".to_string(), "abcd".to_string()]),
            vec![(0, 4)]
        );
        // Pinyin: initials cover all four chars (0,12); full pinyin only
        // covers the first two (0,6). Same start -> the longer span wins.
        assert_eq!(
            match_ranges(
                "工作计划.docx",
                &["gongzuo".to_string(), "gzjh".to_string()]
            ),
            vec![(0, 12)]
        );
        // Direct substring vs pinyin hitting the same start: the longer
        // span wins regardless of source (here the initials span is longer).
        assert_eq!(
            match_ranges("工作计划", &["gzjh".to_string(), "工作".to_string()]),
            vec![(0, 12)]
        );
    }

    #[test]
    fn match_ranges_covers_pinyin_hits_on_chinese_names() {
        // 工作计划 = 4 chars × 3 bytes = 12 bytes; .docx = 5 bytes.
        // Pinyin initials cover all four characters.
        assert_eq!(
            match_ranges("工作计划.docx", &["gzjh".to_string()]),
            vec![(0, 12)]
        );
        // Full pinyin "gongzuo" only spells the first two characters (工作).
        assert_eq!(
            match_ranges("工作计划.docx", &["gongzuo".to_string()]),
            vec![(0, 6)]
        );
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
}
