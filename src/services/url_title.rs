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
    if rd.page_title.is_some() {
        return false; // already has a title, no update needed
    }
    rd.page_title = Some(title);
    let json = rd.to_json();
    db.update_rich_data(existing.id, &json).is_ok()
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
    let mut body = Vec::with_capacity(65536);
    let mut reader = response.into_reader();
    std::io::copy(&mut reader.by_ref().take(65536), &mut body).ok()?;

    let html = String::from_utf8_lossy(&body);
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

/// Extract the content of the first `<title>...</title>` tag (case-insensitive).
fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let start_tag = "<title";
    let end_tag = "</title>";

    let start = lower.find(start_tag)?;
    let after_open = &lower[start + start_tag.len()..];

    // Skip past any attributes: <title lang="en"> → find '>'
    let content_start = after_open.find('>')? + 1;
    let content = &html[start + start_tag.len() + content_start..];

    let content_lower = &lower[start + start_tag.len() + content_start..];
    let end = content_lower.find(end_tag)?;

    let raw = content[..end].trim().to_string();

    // Decode common HTML entities.
    Some(decode_html_entities(&raw))
}

/// Decode the most common HTML character entities.
fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        // Numeric entities (decimal)
        .replace("&#32;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

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
