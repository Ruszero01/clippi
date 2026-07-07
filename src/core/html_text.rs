//! Plain visible-text extraction for clipboard HTML.

pub fn visible_text(html: &str) -> String {
    let html = normalize_cf_html_fragment(html);
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

fn normalize_cf_html_fragment(html: &str) -> &str {
    let Some(header_end) = html.find("<html").or_else(|| html.find("<!DOCTYPE")) else {
        return html;
    };
    let header = &html[..header_end];
    if !header.lines().any(|line| line.starts_with("Version:")) {
        return html;
    }
    &html[header_end..]
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
    use super::visible_text;

    #[test]
    fn visible_text_ignores_style_attributes() {
        let text =
            visible_text(r#"<div data-key="api"><span style="color:red">Visible</span></div>"#);

        assert_eq!(text, "Visible");
    }

    #[test]
    fn visible_text_keeps_visible_substrings() {
        let text = visible_text(r#"<div><span> rapid=</span><span>false</span></div>"#);

        assert!(text.contains("rapid=false"));
        assert!(text.to_lowercase().contains("api"));
    }
}
