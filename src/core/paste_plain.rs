//! Pure mapping logic for the "paste plain text" global hotkey (spec §4).
//!
//! This module is deliberately free of clipboard/OS dependencies: the four
//! mapping rules operate on a `ClipboardContentClass` produced elsewhere by a
//! read-only clipboard probe, so the rules themselves are directly
//! unit-testable on every platform.

/// Classification of the current system clipboard content (spec §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardContentClass {
    /// Rule 1: text-based content (plain text, HTML, RTF, and text-typed
    /// links/colors/paths). Holds the plain-text representation.
    Text(String),
    /// Rule 2: file references present (Windows `CF_HDROP` / macOS
    /// NSFilenames), including a single image file reference ("带路径图片").
    /// Holds one full path per entry.
    Files(Vec<String>),
    /// Rule 3: bitmap only — an image format exists but there is no file
    /// reference format and no text (e.g. a screenshot copy).
    BitmapOnly,
    /// Rule 4: nothing recognizable (empty clipboard, unreadable text, or
    /// read failure).
    Empty,
}

/// Outcome of applying the mapping rules (spec §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PastePlainDecision {
    /// Paste the given plain text: write it back to the clipboard (with the
    /// anti-self-record flag), then simulate a paste.
    Paste(String),
    /// No operation: leave the clipboard untouched and do not simulate a
    /// paste (rules 3 and 4).
    NoOp,
}

/// Apply the four mapping rules from spec §4 to a classified clipboard.
///
/// - `Text` (non-empty) → paste the plain text.
/// - `Files` (non-empty) → paste the paths, one full path per line.
/// - `BitmapOnly` → no-op (rule 3, AC-5).
/// - `Empty`, or `Text`/`Files` that carry nothing usable → no-op (rule 4).
///
/// Whitespace-only text is treated as empty: "复制空文本（无操作）" (spec §8.3).
/// Otherwise the content is preserved verbatim — this function never rewrites
/// user content, it only decides whether to act.
pub fn map_to_plain_paste(class: ClipboardContentClass) -> PastePlainDecision {
    match class {
        ClipboardContentClass::Text(text) if !text.trim().is_empty() => {
            PastePlainDecision::Paste(text)
        }
        ClipboardContentClass::Files(paths) => {
            let text = paths.join("\n");
            if text.trim().is_empty() {
                // A file payload that yields no usable path → rule 4 no-op.
                PastePlainDecision::NoOp
            } else {
                PastePlainDecision::Paste(text)
            }
        }
        // Rules 3 and 4 (and unusable text payloads) end silently.
        _ => PastePlainDecision::NoOp,
    }
}

#[cfg(test)]
mod tests {
    use super::{map_to_plain_paste, ClipboardContentClass, PastePlainDecision};

    #[test]
    fn plain_text_is_pasted_verbatim() {
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Text("hello world".into())),
            PastePlainDecision::Paste("hello world".into())
        );
        // Link / color / path text is treated as plain text (rule 1).
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Text(
                "https://example.com/page".into()
            )),
            PastePlainDecision::Paste("https://example.com/page".into())
        );
    }

    #[test]
    fn empty_or_whitespace_text_is_noop() {
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Text(String::new())),
            PastePlainDecision::NoOp
        );
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Text("   ".into())),
            PastePlainDecision::NoOp
        );
    }

    #[test]
    fn single_file_reference_pastes_its_path() {
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Files(vec![
                r"C:\Users\me\report.pdf".into()
            ])),
            PastePlainDecision::Paste(r"C:\Users\me\report.pdf".into())
        );
    }

    #[test]
    fn multiple_files_join_with_one_path_per_line() {
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Files(vec![
                r"C:\a\one.txt".into(),
                r"C:\b\two.txt".into(),
            ])),
            PastePlainDecision::Paste(
                r"C:\a\one.txt
C:\b\two.txt"
                    .into()
            )
        );
    }

    #[test]
    fn bitmap_without_file_reference_is_noop() {
        // Rule 3: screenshot bitmap → no-op, clipboard unchanged (AC-5).
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::BitmapOnly),
            PastePlainDecision::NoOp
        );
    }

    #[test]
    fn nothing_recognizable_is_noop() {
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Empty),
            PastePlainDecision::NoOp
        );
        // Files payload that yields no usable path is also a no-op.
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Files(vec![String::new()])),
            PastePlainDecision::NoOp
        );
    }

    // ────────────────────────────────────────────────────────────────────────
    // Justice (刑部) justice-tests additions — spec §4 rules + notes + §8.3
    // edge cases, all platform-independent (constructed classified inputs).
    // ────────────────────────────────────────────────────────────────────────

    #[test]
    fn text_with_surrounding_whitespace_is_pasted_verbatim() {
        // Rule 1: only the emptiness guard trims; the pasted content is never
        // rewritten — surrounding whitespace survives verbatim.
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Text("  hello world  ".into())),
            PastePlainDecision::Paste("  hello world  ".into())
        );
    }

    #[test]
    fn multiline_text_is_pasted_verbatim_without_interpretation() {
        // Rule 1: the mapping pastes the plain-text representation it was
        // given; it never parses markup or reformats multi-line payloads.
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Text("line1\nline2\t".into())),
            PastePlainDecision::Paste("line1\nline2\t".into())
        );
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Text("<b>bold</b>".into())),
            PastePlainDecision::Paste("<b>bold</b>".into())
        );
    }

    #[test]
    fn bitmap_plus_text_coexistence_is_pasted_as_text() {
        // Spec §4.1 note + AC-11: when the read-only probe sees a bitmap
        // together with HTML/RTF/plain text (WPS/Excel/OneNote), it classifies
        // the clipboard as `Text` — the mapping must paste, never no-op. This
        // pins the mapping-side boundary: `Text` (the coexistence class)
        // pastes, while `BitmapOnly` (bitmap with no text and no file
        // reference) is the only image case that yields a no-op (rule 3).
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Text("cell value".into())),
            PastePlainDecision::Paste("cell value".into())
        );
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::BitmapOnly),
            PastePlainDecision::NoOp
        );
    }

    #[test]
    fn image_file_reference_pastes_full_path() {
        // Rule 2 note: a single image file reference ("带路径图片") pastes its
        // path text, exactly like any other file reference.
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Files(vec![
                r"C:\Users\me\Pictures\photo.png".into()
            ])),
            PastePlainDecision::Paste(r"C:\Users\me\Pictures\photo.png".into())
        );
    }

    #[test]
    fn folder_reference_pastes_its_path_verbatim() {
        // Rule 2: a folder reference is a path like any other; the trailing
        // separator is preserved verbatim (never rewritten).
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Files(vec![r"D:\Documents\".into()])),
            PastePlainDecision::Paste(r"D:\Documents\".into())
        );
    }

    #[test]
    fn unc_path_reference_pastes_verbatim() {
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Files(vec![
                r"\\server\share\file.txt".into()
            ])),
            PastePlainDecision::Paste(r"\\server\share\file.txt".into())
        );
    }

    #[test]
    fn multiple_files_mix_images_and_documents_one_path_per_line() {
        // Rule 2: multiple files join with exactly one full path per line —
        // no trailing newline, no separator other than `\n` (spec §8.3
        // 边界补测: 复制多个文件).
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Files(vec![
                r"C:\a\doc.pdf".into(),
                r"C:\b\photo.png".into(),
            ])),
            PastePlainDecision::Paste("C:\\a\\doc.pdf\nC:\\b\\photo.png".into())
        );
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Files(vec![
                r"\\nas\share\one.txt".into(),
                r"D:\two.txt".into(),
                r"C:\three.txt".into(),
            ])),
            PastePlainDecision::Paste("\\\\nas\\share\\one.txt\nD:\\two.txt\nC:\\three.txt".into())
        );
    }

    #[test]
    fn files_with_whitespace_only_entries_are_noop() {
        // §8.3 edge: a file payload that carries nothing usable is rule-4
        // no-op. The read-only probe filters empty/whitespace paths before
        // classifying, so this guards the mapping's own defensive emptiness
        // check on the joined payload.
        assert_eq!(
            map_to_plain_paste(ClipboardContentClass::Files(vec!["   ".into()])),
            PastePlainDecision::NoOp
        );
    }
}
