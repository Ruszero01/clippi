//! Local link recognition without network access.
use regex::Regex;
use std::{collections::HashSet, sync::LazyLock};
static LINKS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(?:https?://|ftp://|www\.)[^\s<>"'`\x00-\x1f，。！？；：、【】「」『』（）]+"#,
    )
    .unwrap()
});
pub fn has_candidate(text: &str) -> bool {
    static PREFIX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(?:https?://|ftp://|www\.)").unwrap());
    PREFIX.is_match(text)
}
pub fn extract(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    LINKS
        .find_iter(text)
        .filter_map(|m| {
            let mut s = m.as_str().trim_end_matches(['.', ',', '!', '?', ';', ':']);
            let pairs = [('(', ')'), ('[', ']'), ('{', '}')];
            let mut excess = pairs.map(|(open, close)| {
                s.matches(close)
                    .count()
                    .saturating_sub(s.matches(open).count())
            });
            loop {
                let (open, close) = match s.chars().last() {
                    Some(')') => ('(', ')'),
                    Some(']') => ('[', ']'),
                    Some('}') => ('{', '}'),
                    _ => break,
                };
                let index = pairs
                    .iter()
                    .position(|pair| *pair == (open, close))
                    .unwrap();
                if excess[index] == 0 {
                    break;
                }
                excess[index] -= 1;
                s = s[..s.len() - 1].trim_end_matches(['.', ',', '!', '?', ';', ':']);
            }
            let normalized = if s.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("www.")) {
                format!("https://{s}")
            } else {
                s.to_owned()
            };
            let url = url::Url::parse(&normalized).ok()?;
            if url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
                return None;
            }
            seen.insert(normalized.clone()).then_some(normalized)
        })
        .collect()
}
pub fn extract_content(text: &str, html: Option<&str>) -> Vec<String> {
    let mut links = extract(text);
    let mut seen: HashSet<String> = links.iter().cloned().collect();
    if let Some(html) = html {
        let html = crate::core::html_text::normalize_clipboard_html(html);
        let fragment = scraper::Html::parse_fragment(&html);
        static ANCHORS: LazyLock<scraper::Selector> =
            LazyLock::new(|| scraper::Selector::parse("a[href], area[href]").unwrap());
        for el in fragment.select(&ANCHORS) {
            if let Some(href) = el.value().attr("href") {
                for link in extract(href) {
                    if seen.insert(link.clone()) {
                        links.push(link);
                    }
                }
            }
        }
        for link in extract(&crate::core::html_text::visible_text(&html)) {
            if seen.insert(link.clone()) {
                links.push(link);
            }
        }
    }
    links
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn share_links() {
        assert_eq!(extract("分享https://v.douyin.com/abc/，看看！ [视频](https://b23.tv/xyz)。 https://v.douyin.com/abc/"), vec!["https://v.douyin.com/abc/", "https://b23.tv/xyz"]);
        assert_eq!(
            extract("(https://example.com/Foo_(bar)). www.example.com/a?q=1&b=2"),
            vec![
                "https://example.com/Foo_(bar)",
                "https://www.example.com/a?q=1&b=2"
            ]
        );
    }
    #[test]
    fn hidden_html_links() {
        assert_eq!(
            extract_content(
                "标签",
                Some(r#"<a href="https://example.com/?a=1&amp;b=2">标签</a>"#)
            ),
            vec!["https://example.com/?a=1&b=2"]
        );
    }
    #[test]
    fn no_links() {
        assert!(extract("普通文本 https:// http://?x mail@example.com").is_empty());
        assert!(!has_candidate(&"普通文本 ".repeat(100000)));
    }
}
