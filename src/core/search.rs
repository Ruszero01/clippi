//! Shared keyword search primitives — the single source of truth for
//! tokenization, text matching and match-range computation.
//!
//! Used by both the clipboard history search (`AppState`) and the transfer
//! station name search, and by the highlight renderer (`ui::search_highlight`)
//! so filtering and highlighting can never drift apart. Pure logic, no GPUI.
//!
//! # Cost model (issue #127)
//!
//! Pinyin matching makes a short Latin term (the 简拼 initials users type)
//! match almost every Chinese character, so it must never pay for spans the
//! caller does not need:
//!
//! * Boolean questions ("does this text match?") run the substring check and a
//!   plain `str::contains` over the encoded pinyin forms — they never map
//!   matches back to document offsets.
//! * Offset questions (`first_match_range`, `match_ranges`,
//!   `highlight_segments`) map encoded offsets back with a binary search over
//!   the per-character table, and encode a text at most once per call.
//!
//! Both checks are skipped entirely when pinyin cannot possibly apply: pinyin
//! forms are plain ASCII, so a non-ASCII term can never match them, and an
//! ASCII-only text has no characters to encode. Without that guard a Chinese
//! term paid for a full pinyin encode it could never use.

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
    let term = term.trim().to_lowercase();
    if term.is_empty() {
        return false;
    }
    if find_next_direct_match(text, std::slice::from_ref(&term), 0).is_some() {
        return true;
    }
    pinyin_index_if_reachable(text, std::slice::from_ref(&term))
        .is_some_and(|index| index.matches(&term))
}

/// Whether `text` matches every keyword term (AND semantics).
/// The text's pinyin forms are encoded once and shared by all terms.
pub fn text_matches_all_terms(text: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let lower_terms = lowercase_terms(terms);
    if lower_terms.len() != terms.len() {
        // An all-whitespace term normalizes to nothing and matches nothing.
        return false;
    }
    let index = pinyin_index_if_reachable(text, &lower_terms);
    lower_terms.iter().all(|term| {
        find_next_direct_match(text, std::slice::from_ref(term), 0).is_some()
            || index.as_ref().is_some_and(|index| index.matches(term))
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSegment {
    pub text: String,
    pub highlighted: bool,
}

/// Whether any keyword term matches anywhere in `text`.
pub fn contains_match(text: &str, terms: &[String]) -> bool {
    let lower_terms = lowercase_terms(terms);
    if lower_terms.is_empty() {
        return false;
    }
    if find_next_direct_match(text, &lower_terms, 0).is_some() {
        return true;
    }
    pinyin_index_if_reachable(text, &lower_terms)
        .is_some_and(|index| lower_terms.iter().any(|term| index.matches(term)))
}

/// Byte range of the first keyword match in `text`, if any.
pub fn first_match_range(text: &str, terms: &[String]) -> Option<(usize, usize)> {
    let lower_terms = lowercase_terms(terms);
    if lower_terms.is_empty() || text.is_empty() {
        return None;
    }
    let mut best = find_next_direct_match(text, &lower_terms, 0);
    if let Some(index) = pinyin_index_if_reachable(text, &lower_terms) {
        for term in &lower_terms {
            if let Some(span) = index.first_span(term) {
                best = choose_better_match(best, span);
            }
        }
    }
    best
}

/// All non-overlapping match ranges (byte offsets) in `text`, covering every
/// keyword term — the exact ranges `highlight_segments` renders. Used by
/// callers that need to re-split matches across display boundaries (e.g. the
/// file-name stem/extension split) while keeping one set of match semantics.
pub fn match_ranges(text: &str, terms: &[String]) -> Vec<(usize, usize)> {
    if text.is_empty() || terms.is_empty() {
        return Vec::new();
    }
    let lower_terms = lowercase_terms(terms);
    if lower_terms.is_empty() {
        return Vec::new();
    }

    let mut cursor = MatchCursor::new(text, &lower_terms);
    let mut ranges = Vec::new();
    let mut pos = 0usize;
    while pos < text.len() {
        let Some((start, end)) = cursor.next_match(pos) else {
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
    let lower_terms = lowercase_terms(terms);
    if lower_terms.is_empty() {
        return vec![HighlightSegment {
            text: text.to_string(),
            highlighted: false,
        }];
    }

    let mut cursor = MatchCursor::new(text, &lower_terms);
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < text.len() {
        let Some((start, end)) = cursor.next_match(pos) else {
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

/// Whether pinyin matching is worth attempting at all.
///
/// Pinyin forms are lowercased ASCII, so a term carrying a non-ASCII character
/// can only ever match the text itself, and an ASCII-only text has nothing to
/// encode. Skipping the encode in both cases is pure win — the Chinese half of
/// issue #127 used to encode every candidate text for a term that could never
/// hit it.
fn pinyin_reachable(text: &str, lower_terms: &[String]) -> bool {
    !text.is_ascii() && lower_terms.iter().any(|term| term.is_ascii())
}

fn pinyin_index_if_reachable(text: &str, lower_terms: &[String]) -> Option<PinyinIndex> {
    if pinyin_reachable(text, lower_terms) {
        Some(PinyinIndex::encode(text))
    } else {
        None
    }
}

/// Walks the matches of one text in document order, computing the pinyin spans
/// once for the whole walk instead of once per match.
struct MatchCursor<'a> {
    text: &'a str,
    lower_terms: &'a [String],
    /// Every pinyin span for every term, sorted by start offset.
    pinyin_spans: Vec<(usize, usize)>,
    /// Next unconsumed pinyin span. Spans are only ever compared against
    /// offsets that grow, so consumption is monotonic.
    pinyin_cursor: usize,
    /// Set once a direct search found nothing: no later position can hold a
    /// direct match either, so the scan is never repeated. Without this, a
    /// text whose only hits are pinyin (a Chinese entry searched by 简拼) was
    /// re-scanned to its end for every single hit.
    direct_exhausted: bool,
}

impl<'a> MatchCursor<'a> {
    fn new(text: &'a str, lower_terms: &'a [String]) -> Self {
        let mut pinyin_spans = match pinyin_index_if_reachable(text, lower_terms) {
            Some(index) => pinyin_match_spans(&index, lower_terms),
            None => Vec::new(),
        };
        pinyin_spans.sort_unstable();
        Self {
            text,
            lower_terms,
            pinyin_spans,
            pinyin_cursor: 0,
            direct_exhausted: false,
        }
    }

    /// The first match at or after `from`. Same rule as the single-shot
    /// searches: earliest start, and the longest span when starts tie.
    fn next_match(&mut self, from: usize) -> Option<(usize, usize)> {
        let mut best = if self.direct_exhausted {
            None
        } else {
            let direct = find_next_direct_match(self.text, self.lower_terms, from);
            if direct.is_none() {
                self.direct_exhausted = true;
            }
            direct
        };
        // A span starting before `from` can never be the next match again.
        self.pinyin_cursor +=
            self.pinyin_spans[self.pinyin_cursor..].partition_point(|(start, _)| *start < from);
        // A text can hold several spans at the same start (full pinyin and
        // initials, or overlapping terms); the longest one wins, so consume
        // every tie instead of only the first.
        while let Some(&(start, end)) = self.pinyin_spans.get(self.pinyin_cursor) {
            if best.is_some_and(|(best_start, _)| start > best_start) {
                break;
            }
            best = choose_better_match(best, (start, end));
            self.pinyin_cursor += 1;
        }
        best
    }
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

    /// Whether any pinyin span (full or initials) contains `term`. Boolean
    /// only — no offset mapping, which is what makes the hot filtering path
    /// cheap.
    fn matches(&self, term: &str) -> bool {
        self.full.contains(term) || self.initials.contains(term)
    }

    /// The earliest pinyin span matching `term`, mapped back to the original
    /// text. Only the first occurrence of each form is located, so preview
    /// cropping never enumerates matches it will not use.
    fn first_span(&self, term: &str) -> Option<(usize, usize)> {
        let mut best = first_encoded_span(&self.full, &self.full_chars, term);
        if let Some(span) = first_encoded_span(&self.initials, &self.initial_chars, term) {
            best = choose_better_match(best, span);
        }
        best
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

/// Map the first occurrence of `term` in `encoded` back to the original text.
fn first_encoded_span(encoded: &str, chars: &[PinyinChar], term: &str) -> Option<(usize, usize)> {
    if encoded.is_empty() || term.is_empty() {
        return None;
    }
    let match_start = encoded.find(term)?;
    let match_end = match_start + term.len();
    span_for_encoded_range(chars, match_start, match_end)
}

/// Every occurrence of `term` in `encoded`, mapped back to the original text.
///
/// `chars` is sorted by both offsets, so mapping a match back is a binary
/// search, not a scan: a short term (the 简拼 case) matches thousands of times
/// in a large clipboard entry, and a scan per match made that quadratic.
fn encoded_match_spans(encoded: &str, chars: &[PinyinChar], term: &str) -> Vec<(usize, usize)> {
    if encoded.is_empty() || term.is_empty() {
        return Vec::new();
    }

    let mut spans = Vec::new();
    let mut search_from = 0usize;
    while let Some(rel_start) = encoded[search_from..].find(term) {
        let match_start = search_from + rel_start;
        let match_end = match_start + term.len();

        if let Some(span) = span_for_encoded_range(chars, match_start, match_end) {
            spans.push(span);
        }

        search_from = match_start + 1;
        if search_from >= encoded.len() {
            break;
        }
    }
    spans
}

/// Byte range in the original text covered by `[match_start, match_end)` in the
/// encoded pinyin string.
fn span_for_encoded_range(
    chars: &[PinyinChar],
    match_start: usize,
    match_end: usize,
) -> Option<(usize, usize)> {
    let first = chars.partition_point(|entry| entry.py_end <= match_start);
    let last = chars.partition_point(|entry| entry.py_start < match_end);
    (first < last).then(|| (chars[first].byte_start, chars[last - 1].byte_end))
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

    #[test]
    fn highlight_segments_walk_consecutive_pinyin_matches() {
        // Back-to-back initials hits must not be merged or dropped.
        let terms = vec!["ab".to_string()];
        let segments = highlight_segments("阿波 测试", &terms);
        let highlighted: Vec<String> = segments
            .iter()
            .filter(|segment| segment.highlighted)
            .map(|segment| segment.text.clone())
            .collect();

        assert_eq!(highlighted, vec!["阿波"]);
    }

    /// Regression scenario for issue #127: a single Latin letter matches the
    /// pinyin initials of nearly every Chinese character, so this input makes
    /// the matcher walk a long span list. Pinyin offset mapping used to be a
    /// scan per match — quadratic in the text length — which froze the panel on
    /// clipboard entries of a few thousand characters.
    #[test]
    fn chinese_text_walks_every_initial_match() {
        let filler = "工作计划报告总结数据分析结果团队成员沟通协调会议记录项目进度安排\
                      测试用例覆盖范围版本发布计划问题反馈跟踪文档整理归档工作交接说明";
        let text: String = filler.repeat(40); // ~4 000 chars of Chinese
        let terms = vec!["a".to_string()];

        assert!(super::contains_match(&text, &terms));

        let ranges = match_ranges(&text, &terms);
        assert!(!ranges.is_empty());
        // The focused-preview probe and the highlight walk must agree on the
        // first hit.
        assert_eq!(super::first_match_range(&text, &terms), Some(ranges[0]));
        // Ranges never overlap and always advance, so the walk terminates even
        // when one letter hits thousands of positions.
        assert!(ranges.windows(2).all(|pair| pair[0].1 <= pair[1].0));
        assert!(ranges.iter().all(|(start, end)| start < end));
    }
}
