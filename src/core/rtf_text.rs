//! --- RTF payload text extraction and generation ---
//!
//! Clipboard RTF (`CF_RTF` on Windows, `public.rtf` on macOS) stores non-ASCII
//! text as escapes: `\'hh` byte runs in the document's ANSI code page
//! (`\ansicpgN`), plus `\uN` Unicode escapes followed by a `\ucN`-counted
//! fallback character. Windows RichEdit sources (Event Viewer, WordPad, most
//! Win32 rich-edit controls) use the byte form with the *system* code page —
//! cp936 for Chinese Windows — so mapping each byte to a scalar renders 「错误」
//! as 「´íÎó」 (issue #114). The decoder below therefore resolves the document
//! code page and keeps multi-byte runs together: `\ansicpgN` when declared,
//! otherwise the font's `\fcharsetN`, otherwise the system ANSI code page —
//! several Windows applications (Event Viewer on some builds) emit local-code-
//! page bytes with no `\ansicpg` at all.
//!
//! [`plain_text_to_rtf`] is the round trip used when an edited entry stays an
//! RTF entry: the original formatting cannot survive a plain-text edit, but the
//! stored payload stays a valid RTF document.
//!
//! Code-page resolution lives in `platform::util::decode_ansi_bytes`: Windows
//! uses MultiByteToWideChar and macOS uses CoreFoundation's code-page mapping.

use crate::platform::util::decode_ansi_bytes;

/// Code page [`plain_text_to_rtf`] declares. The generated body is ASCII-only
/// (`\uN` escapes carry everything else), so the value only has to be a valid
/// code page; 1252 is RTF's own default.
const RTF_GENERATED_CODEPAGE: u16 = 1252;

/// Destination groups that never contribute visible body text (font and colour
/// tables, style sheets, document metadata, embedded objects). Skipping them
/// keeps declarations such as a font name (`\'cb\'ce\'cc\'e5` = 宋体) out of the
/// text.
const RTF_SKIPPED_DESTINATIONS: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "pict",
    "object",
    "themedata",
    "colorschememapping",
    "latentstyles",
    "listtable",
    "listoverridetable",
    "rsidtbl",
    "datastore",
    "xmlnstbl",
    "header",
    "headerl",
    "headerr",
    "footer",
    "footerl",
    "footerr",
    "footnote",
    "annotation",
];

/// Convert an RTF payload to plain text.
///
/// Handles the constructs clipboard payloads actually contain: `\ansicpgN`
/// (code page of the `\'hh` byte runs), `\ucN` + `\uN` Unicode escapes with
/// their fallback characters (including surrogate pairs), `\'hh` byte escapes
/// decoded through the document code page, `\par`/`\line`/`\tab`, escaped
/// literals, and destination groups (`{\*\...}`, `{\fonttbl ...}`, …).
pub fn rtf_to_plain_text(rtf: &str) -> String {
    let mut out = String::new();
    let mut chars = rtf.chars().peekable();
    let mut group_depth_to_skip: Option<usize> = None;
    let mut depth = 0usize;
    let mut codepage = initial_codepage(rtf);
    let mut unicode_fallback_skip = 1i32;
    let mut skip_pending = 0i32;
    let mut group_settings = Vec::new();
    // Consecutive `\'hh` escapes form one byte run and must be decoded
    // together, because a multi-byte character spans several escapes.
    let mut ansi_run: Vec<u8> = Vec::new();
    let mut high_surrogate: Option<u16> = None;

    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                flush_ansi_run(&mut out, &mut ansi_run, codepage);
                group_settings.push((codepage, unicode_fallback_skip));
                skip_pending = 0;
                depth += 1;
                if group_depth_to_skip.is_none() && opens_skipped_group(&chars) {
                    group_depth_to_skip = Some(depth);
                }
            }
            '}' => {
                flush_ansi_run(&mut out, &mut ansi_run, codepage);
                if group_depth_to_skip == Some(depth) {
                    group_depth_to_skip = None;
                }
                depth = depth.saturating_sub(1);
                if let Some((page, fallback)) = group_settings.pop() {
                    codepage = page;
                    unicode_fallback_skip = fallback;
                }
                skip_pending = 0;
            }
            '\\' if group_depth_to_skip.is_some() => {
                // Consume escaped syntax so literal braces cannot change depth.
                match chars.peek() {
                    Some('\\' | '{' | '}') => {
                        chars.next();
                    }
                    Some('\'') => {
                        chars.next();
                        chars.next();
                        chars.next();
                    }
                    _ => {
                        read_rtf_control_word(&mut chars);
                    }
                }
            }
            '\\' if group_depth_to_skip.is_none() => match chars.peek().copied() {
                Some('\\' | '{' | '}') => {
                    flush_ansi_run(&mut out, &mut ansi_run, codepage);
                    if let Some(escaped) = chars.next() {
                        if skip_pending > 0 {
                            skip_pending -= 1;
                        } else {
                            high_surrogate = None;
                            out.push(escaped);
                        }
                    }
                }
                Some('\'') => {
                    let _ = chars.next();
                    let hi = chars.next();
                    let lo = chars.next();
                    if let (Some(high), Some(low)) = (
                        hi.and_then(|digit| digit.to_digit(16)),
                        lo.and_then(|digit| digit.to_digit(16)),
                    ) {
                        // A `\uN` fallback may also be written as a byte escape.
                        if skip_pending > 0 {
                            skip_pending -= 1;
                        } else {
                            ansi_run.push((high * 16 + low) as u8);
                        }
                    }
                }
                _ => {
                    let (word, parameter) = read_rtf_control_word(&mut chars);
                    match word.as_str() {
                        "ansicpg" => {
                            flush_ansi_run(&mut out, &mut ansi_run, codepage);
                            if let Some(page) = parameter.filter(|page| *page > 0) {
                                codepage = page.min(u16::MAX as i32) as u16;
                            }
                        }
                        "uc" => {
                            unicode_fallback_skip = parameter.unwrap_or(1).max(0);
                        }
                        "u" => {
                            flush_ansi_run(&mut out, &mut ansi_run, codepage);
                            if let Some(value) = parameter {
                                // A negative parameter is the code unit minus 65536.
                                let value = if value < 0 { value + 0x1_0000 } else { value };
                                push_rtf_code_unit(&mut out, &mut high_surrogate, value as u32);
                            }
                            skip_pending = unicode_fallback_skip;
                        }
                        "par" | "line" => {
                            flush_ansi_run(&mut out, &mut ansi_run, codepage);
                            high_surrogate = None;
                            out.push('\n');
                        }
                        "tab" => {
                            flush_ansi_run(&mut out, &mut ansi_run, codepage);
                            high_surrogate = None;
                            out.push('\t');
                        }
                        // Every other control word (formatting, font selection,
                        // table geometry, …) has no visible text of its own.
                        _ => flush_ansi_run(&mut out, &mut ansi_run, codepage),
                    }
                }
            },
            // Raw line breaks and tabs in the RTF source are formatting aids
            // (RichEdit writes `\par\r\n`); only the control words produce text.
            '\r' | '\n' | '\t' if group_depth_to_skip.is_none() => {
                flush_ansi_run(&mut out, &mut ansi_run, codepage);
            }
            _ if group_depth_to_skip.is_none() => {
                flush_ansi_run(&mut out, &mut ansi_run, codepage);
                if skip_pending > 0 {
                    skip_pending -= 1;
                } else {
                    high_surrogate = None;
                    out.push(ch);
                }
            }
            _ => {}
        }
    }

    flush_ansi_run(&mut out, &mut ansi_run, codepage);

    // Payloads copied from a control end with stray bytes (`}\r\n\0`); NUL and
    // other C0 controls are not text and would otherwise render as glyphs.
    out.chars()
        .filter(|ch| !is_payload_control(*ch))
        .collect::<String>()
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Control characters a clipboard RTF payload can carry but never renders.
/// Tab and newline are real text and stay.
fn is_payload_control(ch: char) -> bool {
    (ch <= '\u{1f}' && ch != '\n' && ch != '\t') || ch == '\u{7f}'
}

/// Build a minimal RTF document from plain text.
///
/// Used when an RTF entry's text is edited: the original formatting is gone
/// either way, but the entry keeps a valid RTF payload (so "copy as rich text"
/// and other RTF consumers still work). Non-ASCII characters are written as
/// `\uN` escapes with their `?` fallback, which every RTF reader understands
/// and which [`rtf_to_plain_text`] round-trips exactly.
pub fn plain_text_to_rtf(text: &str) -> String {
    let mut body = String::with_capacity(text.len() + 32);
    for ch in text.chars() {
        match ch {
            '\\' | '{' | '}' => {
                body.push('\\');
                body.push(ch);
            }
            '\n' => body.push_str("\\par\n"),
            '\r' => {}
            '\t' => body.push_str("\\tab "),
            ch if (ch as u32) < 0x80 => body.push(ch),
            ch => {
                let mut units = [0u16; 2];
                for unit in ch.encode_utf16(&mut units) {
                    // RTF parameters are signed 16-bit, so surrogates and other
                    // units above 0x7FFF are written as a negative value.
                    let signed = if *unit > 0x7FFF {
                        i32::from(*unit) - 0x1_0000
                    } else {
                        i32::from(*unit)
                    };
                    body.push_str("\\u");
                    body.push_str(&signed.to_string());
                    body.push('?');
                }
            }
        }
    }
    format!(
        "{{\\rtf1\\ansi\\ansicpg{0}\\deff0\\uc1{{\\fonttbl{{\\f0\\fnil\\fcharset0 Segoe UI;}}}}\
         \n\\viewkind4\\pard\\f0\\fs20 {body}}}",
        RTF_GENERATED_CODEPAGE
    )
}

/// Code page of the `\'hh` byte runs before the payload declares one.
///
/// Resolution order mirrors what Windows RTF readers do: an explicit
/// `\ansicpgN` wins, then the first `\fcharsetN` font declaration, and finally
/// the system ANSI code page (`0` asks `decode_ansi_bytes` for it). RTF's own
/// 1252 default would mangle the local-code-page payloads several Windows
/// applications write *without* declaring a code page — Event Viewer on some
/// builds emits `\fcharset134` with cp936 bytes and no `\ansicpg` (issue #114).
fn initial_codepage(rtf: &str) -> u16 {
    if let Some(page) = first_control_parameter(rtf, "ansicpg").filter(|page| *page > 0) {
        return page.min(u16::MAX as i32) as u16;
    }
    if let Some(page) = first_control_parameter(rtf, "fcharset").and_then(codepage_for_charset) {
        return page;
    }
    0
}

/// Windows code page implied by an RTF `\fcharsetN` font declaration.
///
/// Only the character sets that actually change the byte interpretation are
/// mapped; `0`/`1` mean "ANSI"/"default" and are deliberately treated as no
/// hint, so the system code page applies.
fn codepage_for_charset(charset: i32) -> Option<u16> {
    match charset {
        128 => Some(932),  // Shift-JIS
        129 => Some(949),  // Korean
        134 => Some(936),  // GB2312
        136 => Some(950),  // Big5
        161 => Some(1253), // Greek
        162 => Some(1254), // Turkish
        163 => Some(1258), // Vietnamese
        177 => Some(1255), // Hebrew
        178 => Some(1256), // Arabic
        186 => Some(1257), // Baltic
        204 => Some(1251), // Cyrillic
        222 => Some(874),  // Thai
        238 => Some(1250), // Central European
        _ => None,
    }
}

/// First numeric parameter of `\wordN` in the payload, matched on the whole
/// control word so `\fcharset` never matches a longer word.
fn first_control_parameter(rtf: &str, word: &str) -> Option<i32> {
    for (index, _) in rtf.match_indices('\\') {
        let rest = &rtf[index + 1..];
        // Both scans are ASCII-only, so byte offsets stay char boundaries.
        let bytes = rest.as_bytes();
        let letters = bytes
            .iter()
            .take_while(|byte| byte.is_ascii_alphabetic())
            .count();
        if letters != word.len() || !rest[..letters].eq_ignore_ascii_case(word) {
            continue;
        }
        let digits = bytes[letters..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit() || **byte == b'-')
            .count();
        if let Ok(value) = rest[letters..letters + digits].parse::<i32>() {
            return Some(value);
        }
    }
    None
}

/// Decode the accumulated `\'hh` byte run with the document's code page.
fn flush_ansi_run(out: &mut String, ansi_run: &mut Vec<u8>, codepage: u16) {
    if ansi_run.is_empty() {
        return;
    }
    out.push_str(&decode_ansi_bytes(codepage, ansi_run));
    ansi_run.clear();
}

/// Push a `\uN` code unit, pairing surrogate halves into one scalar.
fn push_rtf_code_unit(out: &mut String, high_surrogate: &mut Option<u16>, value: u32) {
    if (0xD800..=0xDBFF).contains(&value) {
        *high_surrogate = Some(value as u16);
        return;
    }
    if (0xDC00..=0xDFFF).contains(&value) {
        if let Some(high) = high_surrogate.take() {
            let combined = 0x1_0000 + ((u32::from(high) - 0xD800) << 10) + (value - 0xDC00);
            if let Some(character) = char::from_u32(combined) {
                out.push(character);
            }
        }
        return;
    }
    *high_surrogate = None;
    if let Some(character) = char::from_u32(value) {
        out.push(character);
    }
}

/// Whether the group that just opened is a destination whose content is never
/// rendered: an ignorable `{\*\...}` destination or a table/metadata group.
fn opens_skipped_group(chars: &std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    let mut probe = chars.clone();
    if probe.next() != Some('\\') {
        return false;
    }
    let mut word = String::new();
    while let Some(next) = probe.peek().copied() {
        if next.is_ascii_alphabetic() {
            word.push(next);
            let _ = probe.next();
        } else {
            break;
        }
    }
    if word.is_empty() {
        // `\*` marks an ignorable destination.
        return probe.peek() == Some(&'*');
    }
    RTF_SKIPPED_DESTINATIONS.contains(&word.as_str())
}

/// Read an RTF control word (`\word`, `\word123`, `\word-123`) together with its
/// optional numeric parameter, consuming the single delimiter space if present.
fn read_rtf_control_word(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> (String, Option<i32>) {
    let mut word = String::new();
    while let Some(next) = chars.peek().copied() {
        if next.is_ascii_alphabetic() {
            word.push(next);
            let _ = chars.next();
        } else {
            break;
        }
    }

    let negative = if chars.peek() == Some(&'-') {
        let _ = chars.next();
        true
    } else {
        false
    };
    let mut digits = String::new();
    while let Some(next) = chars.peek().copied() {
        if next.is_ascii_digit() {
            digits.push(next);
            let _ = chars.next();
        } else {
            break;
        }
    }
    if matches!(chars.peek(), Some(' ')) {
        let _ = chars.next();
    }

    let parameter = digits
        .parse::<i32>()
        .ok()
        .map(|value| if negative { -value } else { value });
    (word, parameter)
}

#[cfg(test)]
mod tests {
    use super::{initial_codepage, plain_text_to_rtf, rtf_to_plain_text};

    /// Verbatim RichEdit clipboard RTF on a Chinese Windows system: non-ASCII
    /// text is escaped as `\'hh` bytes in the document code page (cp936), and
    /// the font table declares the font name the same way.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    const RICH_EDIT_GBK_RTF: &str = r"{\rtf1\ansi\ansicpg936\deff0\deflang1033\deflangfe2052{\fonttbl{\f0\fnil\fcharset134 \'cb\'ce\'cc\'e5;}}
\viewkind4\uc1\pard\lang2052\f0\fs18\'b4\'ed\'ce\'f3\'d3\'a6\'d3\'c3\'b3\'cc\'d0\'f2\'c3\'fb\'b3\'c6: fluxdown-agent.exe\'a3\'ac\'b0\'e6\'b1\'be: 0.0.0.0\par
\'b4\'ed\'ce\'f3\'c4\'a3\'bf\'e9\'c3\'fb\'b3\'c6: fluxdown-agent.exe}";

    /// Verbatim payload of the follow-up report: the same cp936 bytes with **no
    /// `\ansicpg`** — only the font table's `\fcharset134` hints at the code
    /// page — plus the `}\r\n\0` tail a control hands over.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    const NO_ANSICPG_CHARSET134_RTF: &str = r"{\rtf1\ansi\deff0\nouicompat{\fonttbl{\f0\fnil\fcharset134 \'ce\'a2\'c8\'ed\'d1\'c5\'ba\'da;}}
\uc1 
\pard\f0\fs18\lang2052 Background Intelligent Transfer Service \'b7\'fe\'ce\'f1\'b5\'c4\'c6\'f4\'b6\'af\'c0\'e0\'d0\'cd\'b4\'d3 \'d7\'d4\'b6\'af\'c6\'f4\'b6\'af \'b8\'fc\'b8\'c4\'ce\'aa \'b0\'b4\'d0\'e8\'c6\'f4\'b6\'af\'a1\'a3}";

    /// Issue #114: the card showed `´íÎóÓ¦ÓÃ³ÌÐòÃû³Æ` because the byte run was
    /// mapped to Latin-1 scalars instead of being decoded as cp936.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn gbk_byte_run_decodes_through_the_document_code_page() {
        assert_eq!(
            rtf_to_plain_text(RICH_EDIT_GBK_RTF),
            "错误应用程序名称: fluxdown-agent.exe，版本: 0.0.0.0\n错误模块名称: fluxdown-agent.exe"
        );
    }

    /// A payload that declares no code page still decodes: the font charset is
    /// the only hint, and the font name in the table must stay out of the text.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn missing_ansicpg_decodes_through_the_font_charset() {
        assert_eq!(
            rtf_to_plain_text(NO_ANSICPG_CHARSET134_RTF),
            "Background Intelligent Transfer Service 服务的启动类型从 自动启动 更改为 按需启动。"
        );
    }

    /// The code page is resolved in that order, and an unknown one asks for the
    /// system code page (`0`) rather than silently assuming 1252.
    #[test]
    fn code_page_resolution_prefers_ansicpg_then_font_charset() {
        assert_eq!(initial_codepage(r"{\rtf1\ansi}"), 0);
        assert_eq!(initial_codepage(r"{\rtf1\ansi\ansicpg936}"), 936);
        assert_eq!(initial_codepage(r"{\rtf1\ansi\ansicpg950}"), 950);
        assert_eq!(
            initial_codepage(r"{\rtf1\ansi{\fonttbl{\f0\fnil\fcharset134 x;}}}"),
            936
        );
        assert_eq!(
            initial_codepage(r"{\rtf1\ansi{\fonttbl{\f0\fnil\fcharset129 x;}}}"),
            949
        );
        // `\fcharset0` means "ANSI": no hint, the system code page applies.
        assert_eq!(
            initial_codepage(r"{\rtf1\ansi{\fonttbl{\f0\fnil\fcharset0 x;}}}"),
            0
        );
        // An explicit `\ansicpg` outranks the font table.
        assert_eq!(
            initial_codepage(r"{\rtf1\ansi\ansicpg950{\fonttbl{\f0\fcharset134 x;}}}"),
            950
        );
    }

    /// NUL and other C0 controls at the end of a clipboard payload never reach
    /// the card.
    #[test]
    fn payload_control_characters_are_dropped() {
        assert_eq!(rtf_to_plain_text("{\\rtf1\\ansi Body}\r\n\u{0}"), "Body");
        assert_eq!(rtf_to_plain_text("{\\rtf1\\ansi A\u{7} B}"), "A B");
    }

    /// `\uN` escapes are followed by a `\ucN`-counted fallback (usually `?`),
    /// and a supplementary character arrives as two surrogate escapes.
    #[test]
    fn unicode_escapes_drop_their_fallback_characters() {
        assert_eq!(
            rtf_to_plain_text(r"{\rtf1\ansi\uc1\u38169?\u35823?}"),
            "错误"
        );
        assert_eq!(rtf_to_plain_text(r"{\rtf1\ansi\uc1\u55357?\u56832?}"), "😀");
    }

    /// Negative `\u` parameters are the code unit minus 65536.
    #[test]
    fn negative_unicode_parameters_decode_to_surrogate_pairs() {
        assert_eq!(
            rtf_to_plain_text(r"{\rtf1\ansi\uc1\u-10179?\u-8704?}"),
            "😀"
        );
    }

    /// Destination groups carry declarations, not body text.
    #[test]
    fn destination_groups_are_skipped() {
        assert_eq!(
            rtf_to_plain_text(
                r"{\rtf1\ansi{\fonttbl{\f0\fnil\fcharset0 Arial;}}{\*\generator Riched20 10.0;}Body}"
            ),
            "Body"
        );
    }

    #[test]
    fn skipped_destinations_keep_escaped_braces_out_of_group_depth() {
        for escaped in [r"\{", r"\}", r"\\{nested}"] {
            let rtf = format!(r"{{\rtf1\ansi {{\*\generator hidden {escaped} text}}VISIBLE}}");
            assert_eq!(rtf_to_plain_text(&rtf), "VISIBLE");
        }
    }

    #[test]
    fn unicode_fallback_count_is_restored_after_nested_groups() {
        assert_eq!(
            rtf_to_plain_text(r"{\rtf1\ansi\uc1 {\uc0\u65}\u66?C}"),
            "ABC"
        );
        assert_eq!(
            rtf_to_plain_text(r"{\rtf1\ansi\uc0 {\uc2\u65??}\u66C}"),
            "ABC"
        );
        assert_eq!(rtf_to_plain_text(r"{\rtf1\uc1\u65{B}C}"), "ABC");
    }

    /// Control words that carry text, escaped literals, and raw CRLF handling.
    #[test]
    fn structure_control_words_become_text() {
        assert_eq!(
            rtf_to_plain_text("{\\rtf1\\ansi A\\tab B\\par\r\nC\\line D\\{x\\}}"),
            "A\tB\nC\nD{x}"
        );
    }

    /// Round trip: an edited entry keeps a payload our own decoder reads back
    /// byte for byte, including newlines, tabs, braces, and emoji.
    #[test]
    fn plain_text_round_trips_through_the_generated_payload() {
        for text in [
            "plain ASCII text",
            "错误应用程序名称: fluxdown-agent.exe，版本: 0.0.0.0",
            "第一行\n第二行\t带制表符",
            "braces {and} back\\slashes",
            "emoji 😀 and CJK 漢字",
        ] {
            let rtf = plain_text_to_rtf(text);
            assert!(rtf.starts_with("{\\rtf1"), "generated RTF is malformed");
            assert_eq!(
                rtf_to_plain_text(&rtf),
                *text,
                "round trip failed for {text}"
            );
        }
    }

    /// The copied payload must not leak the generated font table.
    #[test]
    fn generated_payload_does_not_expose_its_declarations() {
        let rtf = plain_text_to_rtf("正文");
        assert!(!rtf_to_plain_text(&rtf).contains("Segoe"));
    }
}
