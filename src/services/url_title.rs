//! Page title fetching for URL clipboard items.
//! Non-critical: all failures are silent and the UI falls back to the
//! domain+path display.

use std::io::Read;
use std::time::Duration;

/// Fetch the page title from a URL and store it in the item's rich_data.
///
/// Called from a background thread. Returns `true` when the title was
/// successfully fetched and stored — the caller should signal the UI
/// to refresh.
pub fn fetch_and_store_title(url: &str, content_hash: u64, db_path: &str) -> bool {
    let title = match fetch_page_title(url) {
        Some(t) => t,
        None => return false,
    };

    // ── Store in DB ──────────────────────────────────────────────
    let resolved = crate::core::paths::resolve_db_path(db_path);
    let db = match crate::core::db::Database::open(&resolved.to_string_lossy()) {
        Ok(db) => db,
        Err(_) => return false,
    };

    let existing = match db.get_by_hash(content_hash) {
        Ok(Some(item)) => item,
        _ => return false,
    };

    let mut rd = crate::core::types::RichData::from_json(&existing.rich_data);
    if !title_needs_refresh(rd.page_title.as_deref()) {
        return false; // already has a title, no update needed
    }
    rd.page_title = Some(title);
    let json = rd.to_json();
    db.update_rich_data(existing.id, &json).is_ok()
}

/// Retry titles damaged by the former UTF-8-only decoder when a link is revisited.
pub(super) fn title_needs_refresh(title: Option<&str>) -> bool {
    title.is_none_or(|title| title.trim().is_empty() || title.contains('\u{fffd}'))
}

/// HTTP GET `url`, read the first 64 KB of HTML, and extract the `<title>`.
fn fetch_page_title(url: &str) -> Option<String> {
    let url = normalize_http_url(url)?;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .build();

    let response = agent
        .get(&url)
        .set(
            "User-Agent",
            &format!("Clippi/{}", env!("CARGO_PKG_VERSION")),
        )
        .set("Accept", "text/html")
        .call()
        .ok()?;

    // Read at most 64 KB — enough to cover the <head> of any page.
    let content_type = response
        .header("Content-Type")
        .unwrap_or_default()
        .to_string();
    let mut body = Vec::with_capacity(65536);
    let mut reader = response.into_reader();
    std::io::copy(&mut reader.by_ref().take(65536), &mut body).ok()?;

    let html = decode_html(&body, &content_type);
    extract_title(&html)
}

fn normalize_http_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url.to_string())
    } else {
        Some(format!("https://{url}"))
    }
}

/// Resolve BOM, HTTP charset, then HTML meta charset before falling back to UTF-8.
fn decode_html<'a>(body: &'a [u8], content_type: &str) -> std::borrow::Cow<'a, str> {
    use encoding_rs::{Encoding, UTF_8};

    let encoding = Encoding::for_bom(body)
        .map(|(encoding, _)| encoding)
        .or_else(|| charset_from_content_type(content_type))
        .or_else(|| {
            // HTML encoding declarations use ASCII, even on legacy encoded pages.
            let prefix = String::from_utf8_lossy(&body[..body.len().min(1024)]);
            let document = scraper::Html::parse_document(&prefix);
            let selector = scraper::Selector::parse("meta").expect("valid meta selector");
            let encoding = document.select(&selector).find_map(|meta| {
                let value = meta.value();
                value
                    .attr("charset")
                    .and_then(|label| Encoding::for_label(label.as_bytes()))
                    .or_else(|| {
                        value
                            .attr("http-equiv")
                            .filter(|name| name.eq_ignore_ascii_case("content-type"))?;
                        charset_from_content_type(value.attr("content")?)
                    })
            });
            encoding
        })
        .unwrap_or(UTF_8);
    encoding.decode(body).0
}

fn charset_from_content_type(value: &str) -> Option<&'static encoding_rs::Encoding> {
    value.split(';').skip(1).find_map(|parameter| {
        let (name, label) = parameter.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("charset") {
            return None;
        }
        encoding_rs::Encoding::for_label(label.trim().trim_matches(['\"', '\'']).as_bytes())
    })
}

/// Parse HTML rather than indexing a lowercased Unicode copy of the document.
fn extract_title(html: &str) -> Option<String> {
    let document = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("title").expect("valid title selector");
    let title = document
        .select(&selector)
        .next()?
        .text()
        .collect::<String>();
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damaged_cached_titles_can_be_refetched() {
        assert!(title_needs_refresh(None));
        assert!(title_needs_refresh(Some("52pojie - �")));
        assert!(title_needs_refresh(Some(" ")));
        assert!(!title_needs_refresh(Some("吾爱破解")));
    }

    #[test]
    fn legacy_chinese_title_uses_http_charset() {
        let html = "<title>52pojie - 吾爱破解 - 52pojie.cn</title>";
        let (bytes, _, errors) = encoding_rs::GBK.encode(html);
        assert!(!errors);
        assert_eq!(
            extract_title(&decode_html(&bytes, "text/html; charset=gbk")),
            Some("52pojie - 吾爱破解 - 52pojie.cn".to_string())
        );
    }

    #[test]
    fn meta_charset_and_http_equiv_are_supported() {
        for meta in [
            "<meta charset=gb2312>",
            "<META HTTP-EQUIV='Content-Type' CONTENT='text/html; charset=GBK'>",
        ] {
            let html = format!("{meta}<title>吾爱破解</title>");
            let (bytes, _, _) = encoding_rs::GBK.encode(&html);
            assert_eq!(
                extract_title(&decode_html(&bytes, "text/html")),
                Some("吾爱破解".into())
            );
        }
    }

    #[test]
    fn encoding_precedence_and_utf8_fallback() {
        let html = "<meta charset=gbk><title>中文</title>";
        assert_eq!(
            decode_html(html.as_bytes(), "text/html; charset=\"UTF-8\""),
            html
        );
        let bom = [b"\xef\xbb\xbf".as_slice(), html.as_bytes()].concat();
        assert_eq!(decode_html(&bom, "text/html; charset=gbk"), html);
        assert_eq!(decode_html("中文".as_bytes(), "text/html"), "中文");
    }

    #[test]
    fn unicode_before_title_and_numeric_entities() {
        assert_eq!(
            extract_title("<!-- İ --><title>&#21566;&#x7231;破解</title>"),
            Some("吾爱破解".into())
        );
    }

    #[test]
    fn test_extract_title_simple() {
        let html = "<html><head><title>Hello World</title></head></html>";
        assert_eq!(extract_title(html), Some("Hello World".to_string()));
    }

    #[test]
    fn test_extract_title_case_insensitive() {
        let html = "<HTML><HEAD><TITLE>Case Test</TITLE></HEAD></HTML>";
        assert_eq!(extract_title(html), Some("Case Test".to_string()));
    }

    #[test]
    fn test_extract_title_with_attributes() {
        let html = r#"<title lang="en" data-rh="true">GitHub</title>"#;
        assert_eq!(extract_title(html), Some("GitHub".to_string()));
    }

    #[test]
    fn test_extract_title_multiline() {
        let html = "<head>\n<title>\n  Multi\n  Line\n</title>\n</head>";
        assert_eq!(extract_title(html), Some("Multi\n  Line".to_string()));
    }

    #[test]
    fn test_extract_title_entities() {
        let html = "<title>Rock &amp; Roll</title>";
        assert_eq!(extract_title(html), Some("Rock & Roll".to_string()));
    }

    #[test]
    fn test_extract_title_none() {
        assert_eq!(extract_title("<html><head></head></html>"), None);
        assert_eq!(extract_title(""), None);
        assert_eq!(extract_title("no title here"), None);
    }

    #[test]
    fn test_normalize_http_url_keeps_existing_scheme() {
        assert_eq!(
            normalize_http_url("https://example.com/path"),
            Some("https://example.com/path".to_string())
        );
        assert_eq!(
            normalize_http_url("http://example.com/path"),
            Some("http://example.com/path".to_string())
        );
    }

    #[test]
    fn test_normalize_http_url_adds_https_for_protocol_less_url() {
        assert_eq!(
            normalize_http_url("example.com/path"),
            Some("https://example.com/path".to_string())
        );
    }

    #[test]
    fn test_normalize_http_url_rejects_empty_url() {
        assert_eq!(normalize_http_url("  "), None);
    }
}
