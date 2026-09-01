//! Plain visible-text extraction for clipboard HTML.

/// Preserve the complete HTML document from a Windows CF_HTML payload.
///
/// Unlike [`normalize_clipboard_html`], this keeps `<head>` and `<style>` so
/// class-based formatting (notably WPS spreadsheet cell styles) survives a
/// clipboard history round trip.
pub fn preserve_clipboard_html_document(html: &str) -> String {
    let html = html.trim_end_matches('\0');
    if !html.trim_start().starts_with("Version:") {
        return html.to_string();
    }

    let markup_start = html.find('<').unwrap_or(html.len());
    let header = &html[..markup_start];
    let (start, end) = match (
        parse_cf_html_offset(header, "StartHTML:"),
        parse_cf_html_offset(header, "EndHTML:"),
    ) {
        (Some(start), Some(end))
            if start >= markup_start
                && valid_html_range(html, start, end)
                && html[start..].starts_with('<') =>
        {
            (start, end)
        }
        _ => (markup_start, html.len()),
    };
    let document = &html[start..end];
    // Keep numeric fragment boundaries when stripping the transport header.
    // Otherwise a later encode/preview would expand a selection to the body.
    if let (Some(fragment_start), Some(fragment_end)) = (
        parse_cf_html_offset(header, "StartFragment:"),
        parse_cf_html_offset(header, "EndFragment:"),
    ) {
        if fragment_start >= start
            && fragment_end <= end
            && valid_html_range(html, fragment_start, fragment_end)
        {
            return format!(
                "{}<!--StartFragment-->{}<!--EndFragment-->{}",
                strip_fragment_markers(&html[start..fragment_start]),
                strip_fragment_markers(&html[fragment_start..fragment_end]),
                strip_fragment_markers(&html[fragment_end..end]),
            );
        }
    }
    document.to_string()
}

fn valid_byte_range(text: &str, start: usize, end: usize) -> bool {
    start <= end && end <= text.len() && text.is_char_boundary(start) && text.is_char_boundary(end)
}

// CF_HTML offsets can be in bounds yet split a tag or comment. Inserting
// fragment markers there corrupts the document and exposes markup as text.
fn valid_html_range(html: &str, start: usize, end: usize) -> bool {
    if !valid_byte_range(html, start, end) {
        return false;
    }
    let bytes = html.as_bytes();
    let mut cursor = 0;
    while let Some(relative) = html[cursor..].find('<') {
        let open = cursor + relative;
        if open >= end {
            break;
        }
        let Some(next) = bytes.get(open + 1) else {
            break;
        };
        if !next.is_ascii_alphabetic() && !matches!(next, b'/' | b'!' | b'?') {
            cursor = open + 1;
            continue;
        }
        let close = if html[open..].starts_with("<!--") {
            html[open + 4..].find("-->").map(|i| open + 4 + i + 3)
        } else {
            let mut quote = None;
            let mut close = None;
            for (i, &byte) in bytes.iter().enumerate().skip(open + 1) {
                match (quote, byte) {
                    (Some(expected), actual) if actual == expected => quote = None,
                    (None, b'\'' | b'"') => quote = Some(byte),
                    (None, b'>') => {
                        close = Some(i + 1);
                        break;
                    }
                    _ => {}
                }
            }
            close
        };
        let Some(close) = close else {
            return false;
        };
        if (open < start && start < close) || (open < end && end < close) {
            return false;
        }
        cursor = close;
    }
    true
}

/// Encode HTML as CF_HTML, preserving the selected fragment and its context.
#[cfg(any(target_os = "windows", test))]
pub fn encode_cf_html(html: &str) -> String {
    const START_MARKER: &str = "<!--StartFragment-->";
    const END_MARKER: &str = "<!--EndFragment-->";

    let clean = preserve_clipboard_html_document(html);
    let mut document = if find_html_start(&clean).is_some() {
        clean
    } else {
        format!("<html><body>{clean}</body></html>")
    };

    if fragment_marker_range(&document).is_none() {
        document = strip_fragment_markers(&document);
        let body_range = find_case_insensitive(&document, "<body").and_then(|body_start| {
            let start = body_start + document[body_start..].find('>')? + 1;
            let end = start + find_case_insensitive(&document[start..], "</body")?;
            Some((start, end))
        });
        if let Some((start, end)) = body_range {
            document.insert_str(end, END_MARKER);
            document.insert_str(start, START_MARKER);
        } else {
            document = format!("<html><body>{START_MARKER}{document}{END_MARKER}</body></html>");
        }
    }

    let header_template = concat!(
        "Version:1.0\r\n",
        "StartHTML:0000000000\r\n",
        "EndHTML:0000000000\r\n",
        "StartFragment:0000000000\r\n",
        "EndFragment:0000000000\r\n"
    );
    let start_html = header_template.len();
    let end_html = start_html + document.len();
    let (start, end) = fragment_marker_range(&document).expect("CF_HTML fragment must exist");
    let start_fragment = start_html + start;
    let end_fragment = start_html + end;
    let header = format!(
        "Version:1.0\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n"
    );
    debug_assert_eq!(header.len(), header_template.len());
    format!("{header}{document}")
}

/// NSPasteboard takes HTML, not CF_HTML. Keep embedded styles but exclude
/// unselected body context when pasting a history item synced from Windows.
#[cfg(any(target_os = "macos", test))]
pub fn clipboard_html_for_macos(html: &str) -> String {
    let document = preserve_clipboard_html_document(html);
    if fragment_marker_range(&document).is_none() {
        return strip_fragment_markers(&document);
    }
    let head = find_case_insensitive(&document, "<head")
        .and_then(|start| {
            let end = start + find_case_insensitive(&document[start..], "</head>")? + 7;
            document.get(start..end)
        })
        .unwrap_or("");
    format!(
        "<html>{head}<body>{}</body></html>",
        normalize_clipboard_html(&document)
    )
}

/// Normalize a clipboard HTML payload down to its visible fragment.
///
/// Clipboard HTML arrives in several shapes:
///
/// 1. CF_HTML with a numeric header (`StartFragment:` / `EndFragment:` byte
///    offsets) — used by browsers and most apps.
/// 2. A full HTML document with `<!--StartFragment-->` / `<!--EndFragment-->`
///    comments but no numeric header — the shape Microsoft Word ships.
/// 3. A plain fragment with no wrapping at all.
///
/// Word's full documents carry `<head>`, `<style>`, Office XML and `mso-*`
/// metadata that must never leak into previews or search. This function
/// extracts the fragment using the most reliable signal available:
///
/// 1. Valid CF_HTML byte offsets (never slicing through a UTF-8 codepoint).
/// 2. Both fragment comments, in order.
/// 3. The `<body>` element (excluding `<head>`).
/// 4. The original document with the CF_HTML header and markers stripped.
pub fn normalize_clipboard_html(html: &str) -> String {
    let header_end = find_html_start(html);
    let has_cf_header = header_end.is_some_and(|pos| {
        html[..pos]
            .lines()
            .any(|line| line.trim_start().starts_with("Version:"))
    });

    // 1. Byte offsets from a CF_HTML numeric header.
    if let Some(pos) = header_end {
        if has_cf_header {
            let header = &html[..pos];
            if let (Some(start), Some(end)) = (
                parse_cf_html_offset(header, "StartFragment:"),
                parse_cf_html_offset(header, "EndFragment:"),
            ) {
                // Offsets are byte positions; never slice through a UTF-8
                // codepoint. Invalid ranges degrade to marker/body extraction.
                if start >= pos && valid_html_range(html, start, end) {
                    return strip_fragment_markers(&html[start..end]).trim().to_string();
                }
            }
        }
    }

    // 2. `<!--StartFragment-->` / `<!--EndFragment-->` comments.
    if let Some((start, end)) = fragment_marker_range(html) {
        return strip_fragment_markers(&html[start..end]).trim().to_string();
    }

    // 3. `<body>...</body>` — keep the body, exclude `<head>`.
    if let Some(body) = extract_body_inner(html) {
        return strip_fragment_markers(&body).trim().to_string();
    }

    // 4. Fallback: strip the CF_HTML header when present, otherwise keep
    //    the input as-is (plain fragments and plain text pass through).
    match header_end {
        Some(pos) if has_cf_header => strip_fragment_markers(&html[pos..]).trim().to_string(),
        _ => strip_fragment_markers(html).trim().to_string(),
    }
}

/// Locate the start of the document markup (`<html` or `<!DOCTYPE`),
/// tolerating casing differences.
fn find_html_start(html: &str) -> Option<usize> {
    find_case_insensitive(html, "<html").or_else(|| find_case_insensitive(html, "<!DOCTYPE"))
}

/// Case-insensitive byte-oriented `str::find`. Both needles used here are
/// ASCII, so byte scanning cannot split a UTF-8 codepoint.
fn find_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let needle = needle.to_ascii_lowercase();
    let needle = needle.as_bytes();
    let hay = haystack.as_bytes();
    if hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| {
        hay[i..i + needle.len()]
            .iter()
            .zip(needle)
            .all(|(a, b)| a.to_ascii_lowercase() == *b)
    })
}

fn parse_cf_html_offset(header: &str, key: &str) -> Option<usize> {
    header.lines().find_map(|line| {
        line.strip_prefix(key)
            .and_then(|value| value.trim().parse::<usize>().ok())
    })
}

/// Locate a `<!--StartFragment-->`-style comment, tolerating whitespace
/// inside the comment and casing differences. Returns the byte range of the
/// whole comment.
fn find_fragment_comment(html: &str, name: &str) -> Option<(usize, usize)> {
    let mut search_from = 0usize;
    while let Some(rel) = html[search_from..].find("<!--") {
        let comment_start = search_from + rel;
        let comment_rest = &html[comment_start + "<!--".len()..];
        let rel_end = comment_rest.find("-->")?;
        if comment_rest[..rel_end].trim().eq_ignore_ascii_case(name) {
            return Some((
                comment_start,
                comment_start + "<!--".len() + rel_end + "-->".len(),
            ));
        }
        search_from = comment_start + "<!--".len();
    }
    None
}

/// Byte range of the content between the fragment comments, only when both
/// markers exist and are in order. An empty fragment is valid and yields an
/// empty range.
fn fragment_marker_range(html: &str) -> Option<(usize, usize)> {
    let (start_open, start_end) = find_fragment_comment(html, "StartFragment")?;
    let (end_start, end_end) = find_fragment_comment(&html[start_end..], "EndFragment")?;
    let end_start = start_end + end_start;
    let end_end = start_end + end_end;
    // Also reject malformed markers persisted by older capture code. Their
    // removal in the body fallback restores tags split by marker insertion.
    let unmarked = format!(
        "{}{}{}",
        &html[..start_open],
        &html[start_end..end_start],
        &html[end_end..],
    );
    valid_html_range(&unmarked, start_open, start_open + end_start - start_end)
        .then_some((start_end, end_start))
}

/// Content inside `<body>...</body>`, when both tags exist.
fn extract_body_inner(html: &str) -> Option<String> {
    let body_tag_start = find_case_insensitive(html, "<body")?;
    let after_open = html[body_tag_start..].find('>')? + body_tag_start + 1;
    let close_rel = find_case_insensitive(&html[after_open..], "</body")?;
    let close_start = after_open + close_rel;
    Some(html[after_open..close_start].to_string())
}

/// Remove any fragment markers left in the text.
fn strip_fragment_markers(html: &str) -> String {
    let mut out = html.to_string();
    loop {
        let span = find_fragment_comment(&out, "StartFragment")
            .or_else(|| find_fragment_comment(&out, "EndFragment"));
        let Some((start, end)) = span else {
            break;
        };
        out.replace_range(start..end, "");
    }
    out
}

pub fn visible_text(html: &str) -> String {
    let html = normalize_clipboard_html(html);
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();
    let mut last_was_space = false;

    while let Some(ch) = chars.next() {
        if ch == '<' {
            let mut tag = String::new();
            for next in chars.by_ref() {
                if next == '>' {
                    break;
                }
                tag.push(next);
            }
            let tag = tag.trim().to_ascii_lowercase();
            if is_block_break_tag(&tag) {
                out.push('\n');
                last_was_space = true;
            }
            continue;
        }

        if ch.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }

    decode_html_entities(&out).trim().to_string()
}

/// Extract a tab/newline-delimited plain-text representation from an
/// Excel-compatible clipboard table (Microsoft Excel and WPS).
///
/// Spreadsheet applications quote `CF_UNICODETEXT` fields that contain an
/// in-cell newline. The HTML flavor retains the actual cell structure, so it
/// is a safer source for "paste as plain text" than stripping quotes from an
/// otherwise arbitrary text payload.
pub fn spreadsheet_plain_text(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let is_excel_html = lower.contains("urn:schemas-microsoft-com:office:excel")
        || lower.contains("content=\"microsoft excel\"")
        || lower.contains("content='microsoft excel'");
    if !is_excel_html {
        return None;
    }

    let fragment = normalize_clipboard_html(html);
    let fragment_lower = fragment.to_ascii_lowercase();
    if !fragment_lower.contains("<table") || !fragment_lower.contains("<td") {
        return None;
    }

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell: Option<String> = None;
    let mut cursor = 0;

    while cursor < fragment.len() {
        let Some(relative_open) = fragment[cursor..].find('<') else {
            if let Some(value) = cell.as_mut() {
                value.push_str(&decode_html_entities(&fragment[cursor..]));
            }
            break;
        };
        let open = cursor + relative_open;
        if let Some(value) = cell.as_mut() {
            value.push_str(&decode_html_entities(&fragment[cursor..open]));
        }

        let Some(relative_close) = fragment[open..].find('>') else {
            break;
        };
        let close = open + relative_close;
        let raw_tag = fragment[open + 1..close].trim();
        if !raw_tag.starts_with('!') && !raw_tag.starts_with('?') {
            let closing = raw_tag.starts_with('/');
            let name = raw_tag
                .trim_start_matches('/')
                .split_ascii_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches('/')
                .to_ascii_lowercase();
            match (closing, name.as_str()) {
                (false, "td" | "th") => cell = Some(String::new()),
                (false, "br") => {
                    if let Some(value) = cell.as_mut() {
                        value.push('\n');
                    }
                }
                (true, "td" | "th") => {
                    if let Some(value) = cell.take() {
                        row.push(normalize_spreadsheet_cell(&value));
                    }
                }
                (true, "tr") if !row.is_empty() => rows.push(std::mem::take(&mut row)),
                _ => {}
            }
        }
        cursor = close + 1;
    }

    if let Some(value) = cell.take() {
        row.push(normalize_spreadsheet_cell(&value));
    }
    if !row.is_empty() {
        rows.push(row);
    }
    if rows.is_empty() {
        return None;
    }

    Some(
        rows.into_iter()
            .map(|cells| cells.join("\t"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn normalize_spreadsheet_cell(value: &str) -> String {
    value
        .split('\n')
        .map(|line| line.trim_matches(|ch: char| ch.is_whitespace()).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn has_visible_match(html: &str, predicate: impl Fn(&str) -> bool) -> bool {
    let text = visible_text(html);
    !text.is_empty() && predicate(&text)
}

fn is_block_break_tag(tag: &str) -> bool {
    tag.starts_with("br")
        || tag.starts_with("/p")
        || tag.starts_with("/div")
        || tag.starts_with("/li")
        || tag.starts_with("/tr")
        || tag.starts_with("/pre")
}

fn decode_html_entities(text: &str) -> String {
    let named = text
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    decode_numeric_html_entities(&named)
}

fn decode_numeric_html_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("&#") {
        out.push_str(&rest[..start]);
        let entity = &rest[start + 2..];
        let Some(end) = entity.find(';') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let digits = &entity[..end];
        let parsed = digits
            .strip_prefix('x')
            .or_else(|| digits.strip_prefix('X'))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .or_else(|| digits.parse::<u32>().ok())
            .and_then(char::from_u32);
        if let Some(ch) = parsed {
            out.push(ch);
        } else {
            out.push_str(&rest[start..start + 2 + end + 1]);
        }
        rest = &entity[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Representative Word clipboard HTML: full document with Office
    /// metadata, no CF_HTML numeric header, fragment marked by comments.
    const WORD_HTML: &str = r#"<html xmlns:o="urn:schemas-microsoft-com:office:office"
xmlns:w="urn:schemas-microsoft-com:office:word"
xmlns:m="http://schemas.microsoft.com/office/2004/12/omml">
<head>
<meta http-equiv=Content-Type content="text/html; charset=utf-8">
<meta name=ProgId content=Word.Document>
<style>
@font-face { font-family:"Cambria Math"; }
p.MsoNormal, li.MsoNormal, div.MsoNormal { font-family:"Times New Roman"; }
</style>
<xml><w:WordDocument><w:LatentStyles/></w:WordDocument></xml>
</head>
<body lang=ZH-CN style='tab-interval:21.0pt;text-justify-trim:punctuation'>
<!--StartFragment-->
<p class=MsoNormal>与保险公司协调处理索赔并解决问题。</p>
<p class=MsoNormal><span style='mso-spacerun:yes'>在日常运营和客户服务中经过培训和受监督的药学技术人员。</span></p>
<!--EndFragment-->
</body>
</html>"#;

    /// Real sample pasted by the reporter in issue #64 (GitHub comment,
    /// with the mail/comment `>` escaping undone). It has no CF_HTML
    /// numeric header, conditional-comment Office XML, `LatentStyles`,
    /// a `style` block, and a single right-aligned paragraph with inline
    /// `mso-*` styles and an empty `<o:p>`.
    const REAL_WORD_HTML: &str = r#"<html xmlns:o="urn:schemas-microsoft-com:office:office"
xmlns:w="urn:schemas-microsoft-com:office:word"
xmlns:m="http://schemas.microsoft.com/office/2004/12/omml"
xmlns="http://www.w3.org/TR/REC-html40">


<head>
<meta http-equiv=Content-Type content="text/html; charset=utf-8">
<meta name=ProgId content=Word.Document>
<meta name=Generator content="Microsoft Word 15">
<meta name=Originator content="Microsoft Word 15">
<link rel=File-List
href="file:///C:/Users/ADMINI~1/AppData/Local/Temp/msohtmlclip1/01/clip_filelist.xml">
<!--[if gte mso 9]><xml>
 <o:OfficeDocumentSettings>
  <o:RelyOnVML/>
  <o:AllowPNG/>
 </o:OfficeDocumentSettings>
</xml><![endif]-->
<link rel=themeData
href="file:///C:/Users/ADMINI~1/AppData/Local/Temp/msohtmlclip1/01/clip_themedata.thmx">
<link rel=colorSchemeMapping
href="file:///C:/Users/ADMINI~1/AppData/Local/Temp/msohtmlclip1/01/clip_colorschememapping.xml">
<!--[if gte mso 9]><xml>
 <w:WordDocument>
  <w:View>Normal</w:View>
  <w:Zoom>0</w:Zoom>
  <w:DrawingGridVerticalSpacing>7.8 磅</w:DrawingGridVerticalSpacing>
  <w:LidThemeAsian>ZH-CN</w:LidThemeAsian>
  <m:mathPr>
   <m:mathFont m:val="Cambria Math"/>
   <m:brkBinSub m:val="&#45;-"/>
  </m:mathPr>
 </w:WordDocument>
</xml><![endif]--><!--[if gte mso 9]><xml>
 <w:LatentStyles DefLockedState="false" DefUnhideWhenUsed="false"
  DefSemiHidden="false" DefQFormat="false" DefPriority="99"
  LatentStyleCount="371">
  <w:LsdException Locked="false" Priority="0" QFormat="true" Name="Normal"/>
  <w:LsdException Locked="false" Priority="9" QFormat="true" Name="heading 1"/>
  <w:LsdException Locked="false" Priority="9" SemiHidden="true"
   UnhideWhenUsed="true" QFormat="true" Name="heading 2"/>
  <w:LsdException Locked="false" Priority="39" Name="Table Grid"/>
  <w:LsdException Locked="false" Priority="46" Name="List Table 1 Light"/>
 </w:LatentStyles>
</xml><![endif]-->
<style>
<!--
 /* Font Definitions */
@font-face
   {font-family:宋体;
   panose-1:2 1 6 0 3 1 1 1 1 1;
   mso-font-alt:SimSun;
   mso-font-charset:134;
   mso-generic-font-family:auto;
   mso-font-pitch:variable;
   mso-font-signature:515 680460288 22 0 262145 0;}
@font-face
   {font-family:"Cambria Math";
   panose-1:2 4 5 3 5 4 6 3 2 4;
   mso-font-charset:1;
   mso-generic-font-family:roman;
   mso-font-pitch:variable;
   mso-font-signature:0 0 0 0 0 0;}
 /* Style Definitions */
 p.MsoNormal, li.MsoNormal, div.MsoNormal
   {mso-style-unhide:no;
   mso-style-qformat:yes;
   margin:0cm;
   margin-bottom:.0001pt;
   mso-pagination:widow-orphan;
   font-size:12.0pt;
   font-family:宋体;
   mso-bidi-font-family:宋体;}
.MsoChpDefault
   {mso-style-type:export-only;
   mso-default-props:yes;
   font-size:10.0pt;
   font-family:"Calibri",sans-serif;
   mso-bidi-font-family:"Times New Roman";
   mso-bidi-theme-font:minor-bidi;
   mso-font-kerning:0pt;}
 /* Page Definitions */
@page WordSection1
   {size:612.0pt 792.0pt;
   margin:72.0pt 90.0pt 72.0pt 90.0pt;
   mso-header-margin:36.0pt;
   mso-footer-margin:36.0pt;
   mso-paper-source:0;}
div.WordSection1
   {page:WordSection1;}
-->
</style>
<!--[if gte mso 10]>
<style>
 /* Style Definitions */
 table.MsoNormalTable
   {mso-style-name:普通表格;
   mso-tstyle-rowband-size:0;
   mso-style-priority:99;
   mso-padding-alt:0cm 5.4pt 0cm 5.4pt;
   mso-para-margin:0cm;
   mso-pagination:widow-orphan;
   font-size:10.0pt;
   font-family:"Calibri",sans-serif;
   mso-ascii-theme-font:minor-latin;
   mso-bidi-font-family:"Times New Roman";
   mso-bidi-theme-font:minor-bidi;}
</style>
<![endif]-->
</head>


<body lang=ZH-CN style='tab-interval:21.0pt;text-justify-trim:punctuation'>
<!--StartFragment-->


<p class=MsoNormal align=right style='margin-top:6.0pt;margin-right:0cm;
margin-bottom:6.0pt;margin-left:0cm;mso-para-margin-top:.5gd;mso-para-margin-right:
0cm;mso-para-margin-bottom:.5gd;mso-para-margin-left:0cm;text-align:right;
mso-pagination:none'><b style='mso-bidi-font-weight:normal'><span
style='font-size:11.0pt;mso-bidi-font-size:14.0pt;mso-bidi-font-family:"Times New Roman";
mso-font-kerning:1.0pt'>豫欣恩预评字<span lang=EN-US>[2026]053</span>号<span
lang=EN-US><o:p></o:p></span></span></b></p>


<!--EndFragment-->
</body>


</html>"#;

    #[test]
    fn real_word_sample_from_issue_64_extracts_visible_text() {
        let out = normalize_clipboard_html(REAL_WORD_HTML);

        // The reporter's actual paragraph survives.
        assert!(out.contains("豫欣恩预评字"));
        assert!(out.contains("[2026]053"));
        assert!(out.contains("号"));
        // Head / style / Office XML metadata never leaks into the fragment.
        assert!(!out.contains("LatentStyles"));
        assert!(!out.contains("WordDocument"));
        assert!(!out.contains("Cambria Math"));
        assert!(!out.contains("@font-face"));
        assert!(!out.contains("<head"));
        assert!(!out.contains("<style"));
        assert!(!out.contains("<!--"));
        // The paragraph's inline mso-* styles stay on the tags.
        assert!(out.contains("mso-para-margin-top:.5gd"));
        assert!(out.contains("<o:p></o:p>"));
    }

    #[test]
    fn real_word_sample_visible_text_shows_only_the_paragraph() {
        let text = visible_text(REAL_WORD_HTML);

        assert_eq!(text, "豫欣恩预评字[2026]053号");
    }

    #[test]
    fn word_html_without_numeric_header_extracts_fragment() {
        let out = normalize_clipboard_html(WORD_HTML);

        assert!(out.contains("与保险公司协调处理索赔并解决问题。"));
        assert!(out.contains("在日常运营和客户服务中经过培训和受监督的药学技术人员。"));
        assert!(!out.contains("Cambria Math"));
        assert!(!out.contains("Times New Roman"));
        assert!(!out.contains("LatentStyles"));
        assert!(!out.contains("<head"));
        assert!(!out.contains("<style"));
    }

    #[test]
    fn word_fragment_keeps_sdt_and_o_p_wrappers() {
        let html = r#"<html><body><!--StartFragment--><p><w:Sdt><span lang=ZH-CN>带超链接的文字</span></w:Sdt><o:p></o:p></p><!--EndFragment--></body></html>"#;

        let out = normalize_clipboard_html(html);

        assert!(out.contains("带超链接的文字"));
        assert!(!out.contains("<!--StartFragment"));
        assert!(!out.contains("<!--EndFragment"));
    }

    #[test]
    fn cf_html_byte_offsets_are_preferred() {
        let markup =
            r#"<html><body><!--StartFragment--><p>Hello 世界</p><!--EndFragment--></body></html>"#;
        let frag_start =
            markup.find("<!--StartFragment-->").unwrap() + "<!--StartFragment-->".len();
        let frag_end = markup.find("<!--EndFragment-->").unwrap();
        let header_len =
            "Version:1.0\r\nStartHTML:0\r\nEndHTML:0\r\nStartFragment:0\r\nEndFragment:0\r\n".len();
        let header = format!(
            "Version:1.0\r\nStartHTML:0\r\nEndHTML:{}\r\nStartFragment:{}\r\nEndFragment:{}\r\n",
            header_len + markup.len(),
            header_len + frag_start,
            header_len + frag_end,
        );
        let full = format!("{header}{markup}");

        let out = normalize_clipboard_html(&full);

        assert_eq!(out, "<p>Hello 世界</p>");
    }

    #[test]
    fn cf_html_offset_inside_utf8_codepoint_falls_back_to_markers() {
        let markup =
            r#"<html><body><!--StartFragment--><p>Hello 世界</p><!--EndFragment--></body></html>"#;
        let frag_start =
            markup.find("<!--StartFragment-->").unwrap() + "<!--StartFragment-->".len();
        let frag_end = markup.find("<!--EndFragment-->").unwrap();
        let header_len =
            "Version:1.0\r\nStartHTML:0\r\nEndHTML:0\r\nStartFragment:0\r\nEndFragment:0\r\n".len();
        // StartFragment points one byte into the first CJK character.
        let bad_start = header_len + frag_start + "<p>Hello ".len() + 1;
        let header = format!(
            "Version:1.0\r\nStartHTML:0\r\nEndHTML:{}\r\nStartFragment:{}\r\nEndFragment:{}\r\n",
            header_len + markup.len(),
            bad_start,
            header_len + frag_end,
        );
        let full = format!("{header}{markup}");

        let out = normalize_clipboard_html(&full);

        assert_eq!(out, "<p>Hello 世界</p>");
    }

    #[test]
    fn cf_html_offset_out_of_range_falls_back_to_markers() {
        let markup =
            r#"<html><body><!--StartFragment--><p>Hello</p><!--EndFragment--></body></html>"#;
        let frag_start =
            markup.find("<!--StartFragment-->").unwrap() + "<!--StartFragment-->".len();
        let header = format!(
            "Version:1.0\r\nStartHTML:0\r\nEndHTML:999999\r\nStartFragment:{}\r\nEndFragment:999999\r\n",
            frag_start,
        );
        let full = format!("{header}{markup}");

        let out = normalize_clipboard_html(&full);

        assert_eq!(out, "<p>Hello</p>");
    }

    #[test]
    fn no_markers_extracts_body_inner() {
        let html =
            r#"<html><head><style>p{color:red}</style></head><body><p>Body text</p></body></html>"#;

        let out = normalize_clipboard_html(html);

        assert_eq!(out, "<p>Body text</p>");
    }

    #[test]
    fn plain_fragment_passes_through() {
        let html = r#"<p>Hello <b>world</b></p>"#;

        let out = normalize_clipboard_html(html);

        assert_eq!(out, html);
    }

    #[test]
    fn missing_end_marker_falls_back_to_body() {
        let html = r#"<html><body><!--StartFragment--><p>Text</p></body></html>"#;

        let out = normalize_clipboard_html(html);

        assert!(out.contains("<p>Text</p>"));
        assert!(!out.contains("<!--StartFragment"));
    }

    #[test]
    fn out_of_order_markers_fall_back_to_body() {
        let html = r#"<html><body><!--EndFragment--><p>Text</p><!--StartFragment--></body></html>"#;

        let out = normalize_clipboard_html(html);

        assert!(out.contains("<p>Text</p>"));
        assert!(!out.contains("StartFragment"));
        assert!(!out.contains("EndFragment"));
    }

    #[test]
    fn empty_fragment_yields_empty_string() {
        let html = r#"<html><body><!--StartFragment--><!--EndFragment--></body></html>"#;

        let out = normalize_clipboard_html(html);

        assert_eq!(out, "");
    }

    #[test]
    fn casing_variations_are_tolerated() {
        let html = r#"<HTML><BODY><!--STARTFRAGMENT--><p>Text</p><!--endfragment--></BODY></HTML>"#;

        let out = normalize_clipboard_html(html);

        assert_eq!(out, "<p>Text</p>");
    }

    #[test]
    fn visible_text_uses_fragment_not_metadata() {
        let text = visible_text(WORD_HTML);

        assert!(text.contains("与保险公司协调处理索赔并解决问题。"));
        assert!(!text.contains("Times New Roman"));
        assert!(!text.contains("Cambria"));
        assert!(!text.contains("LatentStyles"));
    }

    #[test]
    fn visible_text_keeps_link_text() {
        let text = visible_text(r#"<p><a href="https://example.com">Link text</a></p>"#);

        assert_eq!(text, "Link text");
    }

    #[test]
    fn wps_class_style_survives_cf_html_round_trip() {
        let wps = r#"<html><head><style>.et2 { color: #ff6600; }</style></head><body><table><tr><td class=et2>测试文本</td></tr></table></body></html>"#;

        let payload = encode_cf_html(wps);
        let preserved = preserve_clipboard_html_document(&payload);

        assert_eq!(super::strip_fragment_markers(&preserved), wps);
        assert!(preserved.contains(".et2 { color: #ff6600; }"));
        assert_eq!(
            normalize_clipboard_html(&payload),
            "<table><tr><td class=et2>测试文本</td></tr></table>"
        );
    }

    #[test]
    fn numeric_fragment_offsets_must_not_split_table_tags() {
        let document = "<html><head><style>.et2{color:red}</style></head><body><!--StartFragment--><table border=0 cellpadding=0 cellspacing=0><tr><td class=et2>测试文本</td></tr></table><!--EndFragment--></body></html>";
        let header = "Version:1.0\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
        let table_start = document.find("<table").unwrap();
        let table_end = document.find("</table>").unwrap() + "</table>".len();
        for (start, end) in [(table_start + 1, table_end), (table_start, table_end - 1)] {
            let payload = format!(
                "Version:1.0\r\nStartHTML:{:010}\r\nEndHTML:{:010}\r\nStartFragment:{:010}\r\nEndFragment:{:010}\r\n{document}",
                header.len(), header.len() + document.len(), header.len() + start, header.len() + end,
            );
            let expected = &document[table_start..table_end];
            assert_eq!(normalize_clipboard_html(&payload), expected);
            let stored = preserve_clipboard_html_document(&payload);
            assert_eq!(stored, document);
            assert_eq!(visible_text(&stored), "测试文本");
            assert_eq!(normalize_clipboard_html(&encode_cf_html(&stored)), expected);
        }
    }

    #[test]
    fn malformed_stored_markers_do_not_expose_table_markup() {
        let table = "<table border=0 cellpadding=0 cellspacing=0><tr><td class=et2>测试文本</td></tr></table>";
        for (start, end) in [(1, table.len()), (0, table.len() - 1)] {
            // Reproduce the exact marker insertion used by the released code.
            let stored = format!(
                "<html><head><style>.et2{{color:red}}</style></head><body>{}<!--StartFragment-->{}<!--EndFragment-->{}</body></html>",
                &table[..start], &table[start..end], &table[end..],
            );
            assert_eq!(normalize_clipboard_html(&stored), table);
            assert_eq!(visible_text(&stored), "测试文本");
            let encoded = encode_cf_html(&stored);
            assert_eq!(normalize_clipboard_html(&encoded), table);
            assert!(encoded.contains("<style>.et2{color:red}</style>"));
            let mac_html = super::clipboard_html_for_macos(&stored);
            assert!(mac_html.contains(table));
            assert_eq!(visible_text(&mac_html), "测试文本");
        }
    }

    #[test]
    fn html_boundaries_distinguish_attributes_comments_and_text_selections() {
        let html = "<body><span title='a > b'>测试文本</span><!-- comment > here --></body>";
        let attr = html.find("b'").unwrap();
        let comment = html.find("here").unwrap();
        let text = html.find("测试文本").unwrap();
        assert!(!super::valid_html_range(html, attr, html.len()));
        assert!(!super::valid_html_range(html, 0, comment));
        assert!(super::valid_html_range(html, text, text + "测试".len()));
    }

    #[test]
    fn cf_html_fragment_offsets_are_byte_accurate_for_non_ascii_text() {
        let payload = encode_cf_html("<span style='color:#ff6600'>测试文本</span>");

        assert_eq!(
            normalize_clipboard_html(&payload),
            "<span style='color:#ff6600'>测试文本</span>"
        );
    }

    #[test]
    fn selected_fragment_survives_storage_and_cross_platform_paste() {
        let document = "<html><head><style>.x{color:red}</style></head><body>未选中<!--StartFragment--><b class=x>选中</b><!--EndFragment-->也未选中</body></html>";
        let payload = encode_cf_html(document);
        let stored = preserve_clipboard_html_document(&payload);
        assert_eq!(stored, document);
        assert_eq!(normalize_clipboard_html(&stored), "<b class=x>选中</b>");
        assert_eq!(
            normalize_clipboard_html(&encode_cf_html(&stored)),
            "<b class=x>选中</b>"
        );
        let mac_html = super::clipboard_html_for_macos(&stored);
        assert!(!mac_html.contains("未选中"));
        assert!(mac_html.contains("<style>.x{color:red}</style>"));
        assert_eq!(
            normalize_clipboard_html(&encode_cf_html(&mac_html)),
            "<b class=x>选中</b>"
        );
    }

    #[test]
    fn numeric_fragment_offsets_survive_without_comments_or_document_context() {
        let fragment = "<b>选中</b>";
        let header = format!("Version:1.0\r\nStartHTML:-1\r\nEndHTML:-1\r\nStartFragment:{:010}\r\nEndFragment:{:010}\r\n", 0, 0);
        let start = header.len();
        let payload = format!("Version:1.0\r\nStartHTML:-1\r\nEndHTML:-1\r\nStartFragment:{start:010}\r\nEndFragment:{:010}\r\n{fragment}", start + fragment.len());
        let stored = preserve_clipboard_html_document(&payload);
        assert!(!stored.contains("Version:"));
        assert_eq!(normalize_clipboard_html(&stored), fragment);
        assert_eq!(normalize_clipboard_html(&encode_cf_html(&stored)), fragment);
    }

    #[test]
    fn invalid_fragment_offsets_fall_back_without_slicing_utf8() {
        let document = "<html><body><!--StartFragment-->中文<!--EndFragment--></body></html>";
        let payload = encode_cf_html(document);
        let start = payload.find("中文").unwrap() + 1;
        let previous = payload
            .lines()
            .find(|line| line.starts_with("StartFragment:"))
            .unwrap();
        let payload = payload.replacen(previous, &format!("StartFragment:{start:010}"), 1);
        assert_eq!(
            normalize_clipboard_html(&preserve_clipboard_html_document(&payload)),
            "中文"
        );
    }

    #[test]
    fn excel_multiline_cell_plain_text_has_no_transport_quotes() {
        let html = r#"<html xmlns:x="urn:schemas-microsoft-com:office:excel">
<head><meta name=Generator content="Microsoft Excel"></head><body>
<!--StartFragment--><table>&#x20;<tr><td x:str>表格测试纯文本<br>表格测试纯文本</td></tr></table><!--EndFragment-->
</body></html>"#;

        assert_eq!(
            spreadsheet_plain_text(html).as_deref(),
            Some("表格测试纯文本\n表格测试纯文本")
        );
    }

    #[test]
    fn excel_table_preserves_rows_columns_and_decodes_numeric_entities() {
        let html = r#"<html xmlns:x='urn:schemas-microsoft-com:office:excel'><body>
<!--StartFragment--><table><tr><td>A&#x20;B</td><td>C</td></tr>
<tr><td>D<br>E</td><td>&#34;F&#34;</td></tr></table><!--EndFragment-->
</body></html>"#;

        assert_eq!(
            spreadsheet_plain_text(html).as_deref(),
            Some("A B\tC\nD\nE\t\"F\"")
        );
    }

    #[test]
    fn ordinary_html_is_not_treated_as_a_spreadsheet() {
        let html = "<html><body><table><tr><td>\"用户文本\"</td></tr></table></body></html>";
        assert_eq!(spreadsheet_plain_text(html), None);
    }
}
