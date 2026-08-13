//! Plain visible-text extraction for clipboard HTML.

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
                if start <= end
                    && end <= html.len()
                    && html.is_char_boundary(start)
                    && html.is_char_boundary(end)
                {
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
    let (_, start_end) = find_fragment_comment(html, "StartFragment")?;
    let (end_start, _) = find_fragment_comment(&html[start_end..], "EndFragment")?;
    Some((start_end, start_end + end_start))
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
    text.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
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
}
