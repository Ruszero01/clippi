//! Literal find/replace for the editor's find bar.
//!
//! Matching is plain literal and case-insensitive: an ASCII query is compared
//! with `eq_ignore_ascii_case`, anything else compares character by character
//! with `to_lowercase`. Both paths return byte ranges into the searched text,
//! so the caller can slice the original string without index mapping.

use std::ops::Range;

/// Cap on the matches collected for one query. A single-letter query in a large
/// entry would otherwise build an unbounded range list; navigation, replace-all
/// and the count all work on the collected matches, and the bar marks the cap
/// with a trailing `+` so a truncated list is never passed off as complete.
pub const MAX_MATCHES: usize = 10_000;

/// The matches of one query, in order of appearance.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MatchSet {
    /// Byte ranges of the first [`MAX_MATCHES`] matches.
    pub ranges: Vec<Range<usize>>,
    /// How many matches the text actually holds, cap included.
    pub total: usize,
}

impl MatchSet {
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

/// All (non-overlapping) matches of `query` in `text`.
///
/// An empty query matches nothing.
pub fn find_matches(text: &str, query: &str) -> MatchSet {
    let mut set = MatchSet::default();
    if query.is_empty() {
        return set;
    }

    if text.is_ascii() && query.is_ascii() {
        let haystack = text.as_bytes();
        let needle = query.as_bytes();
        let mut at = 0;
        while let Some(found) = find_ascii_ignore_case(&haystack[at..], needle) {
            let start = at + found;
            let end = start + needle.len();
            set.total += 1;
            if set.ranges.len() < MAX_MATCHES {
                set.ranges.push(start..end);
            }
            at = end;
        }
        return set;
    }

    let mut at = 0;
    while at < text.len() {
        let Some(ch) = text[at..].chars().next() else {
            break;
        };
        match match_end_at(text, at, query) {
            Some(end) => {
                set.total += 1;
                if set.ranges.len() < MAX_MATCHES {
                    set.ranges.push(at..end);
                }
                // Non-overlapping, like every find/replace tool: continue after
                // the match instead of inside it.
                at = end;
            }
            None => at += ch.len_utf8(),
        }
    }
    set
}

/// Replace one match. Ranges outside `text` are left alone.
pub fn replace_one(text: &str, range: &Range<usize>, replacement: &str) -> String {
    if range.start > range.end || range.end > text.len() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + replacement.len());
    out.push_str(&text[..range.start]);
    out.push_str(replacement);
    out.push_str(&text[range.end..]);
    out
}

/// Replace every match, skipping any range that overlaps an earlier one.
pub fn replace_all(text: &str, ranges: &[Range<usize>], replacement: &str) -> String {
    if ranges.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + ranges.len() * replacement.len());
    let mut at = 0;
    for range in ranges {
        if range.start < at || range.start > range.end || range.end > text.len() {
            continue;
        }
        out.push_str(&text[at..range.start]);
        out.push_str(replacement);
        at = range.end;
    }
    out.push_str(&text[at..]);
    out
}

/// Which match to land on when the current one is gone or the query changed:
/// the first match at or after `offset`, wrapping to the first match overall.
pub fn index_at_or_after(ranges: &[Range<usize>], offset: usize) -> usize {
    ranges
        .iter()
        .position(|range| range.start >= offset)
        .unwrap_or(0)
}

fn find_ascii_ignore_case(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

/// Byte offset just past `query` when it matches `text` at `start`, ignoring
/// case. `None` when it does not match there.
fn match_end_at(text: &str, start: usize, query: &str) -> Option<usize> {
    let rest = &text[start..];
    let mut chars = rest.char_indices();
    let mut matched = None;
    for query_char in query.chars() {
        let (_, text_char) = chars.next()?;
        if !equals_ignore_case(text_char, query_char) {
            return None;
        }
        matched = Some(start + rest.len() - chars.as_str().len());
    }
    matched
}

fn equals_ignore_case(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::{find_matches, index_at_or_after, replace_all, replace_one, MatchSet};

    fn ranges(text: &str, query: &str) -> Vec<(usize, usize)> {
        find_matches(text, query)
            .ranges
            .into_iter()
            .map(|range| (range.start, range.end))
            .collect()
    }

    #[test]
    fn matches_are_literal_and_ignore_case() {
        assert_eq!(
            ranges("one_two_three", "_"),
            vec![(3, 4), (7, 8)],
            "the issue's own example: every underscore"
        );
        assert_eq!(
            ranges("Hello HELLO hello", "hello"),
            vec![(0, 5), (6, 11), (12, 17)]
        );
        assert_eq!(ranges("abc", "ABC"), vec![(0, 3)]);
        // Case-insensitive matching still returns the matched bytes, not the query.
        assert_eq!(ranges("Hello", "he"), vec![(0, 2)]);
    }

    #[test]
    fn matching_is_non_overlapping_and_handles_multibyte_text() {
        assert_eq!(
            ranges("aaa", "aa"),
            vec![(0, 2)],
            "no overlap inside a match"
        );
        assert_eq!(ranges("工作_计划_工作", "_"), vec![(6, 7), (13, 14)]);
        assert_eq!(ranges("Ärger är bra", "ä"), vec![(0, 2), (7, 9)]);
        // A byte that cannot start a match is skipped without splitting a char.
        assert_eq!(ranges("工作", "作"), vec![(3, 6)]);
    }

    #[test]
    fn an_empty_query_matches_nothing() {
        assert_eq!(find_matches("anything", ""), MatchSet::default());
        assert_eq!(find_matches("", "x"), MatchSet::default());
        assert!(find_matches("", "").is_empty());
    }

    #[test]
    fn the_count_reports_matches_past_the_cap() {
        let text = "a".repeat(super::MAX_MATCHES + 25);
        let set = find_matches(&text, "a");
        assert_eq!(set.ranges.len(), super::MAX_MATCHES);
        assert_eq!(set.total, super::MAX_MATCHES + 25);
        assert!(
            set.total > set.ranges.len(),
            "the cap is marked by the caller"
        );
    }

    #[test]
    fn replace_one_and_all_rewrite_only_the_matches() {
        let text = "one_two_three";
        let set = find_matches(text, "_");
        assert_eq!(replace_one(text, &set.ranges[0], " "), "one two_three");
        assert_eq!(replace_all(text, &set.ranges, " "), "one two three");
        // Replacing with the query itself leaves the text alone.
        assert_eq!(replace_all(text, &set.ranges, "_"), text);
        assert_eq!(replace_all(text, &[], "x"), text);
    }

    #[test]
    fn the_landing_match_is_the_first_one_left_after_the_caret() {
        let set = find_matches("a_a_a_a", "a");
        let starts: Vec<usize> = set.ranges.iter().map(|range| range.start).collect();
        assert_eq!(starts, vec![0, 2, 4, 6]);

        assert_eq!(index_at_or_after(&set.ranges, 0), 0);
        assert_eq!(index_at_or_after(&set.ranges, 1), 1, "caret inside a match");
        assert_eq!(index_at_or_after(&set.ranges, 5), 3);
        // Past the last match: wrap around to the first one.
        assert_eq!(index_at_or_after(&set.ranges, 99), 0);
        assert_eq!(index_at_or_after(&[], 0), 0);
    }
}
