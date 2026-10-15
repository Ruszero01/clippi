//! Unicode-derived offline symbols; category IDs match update-symbol-data.py.
use super::emoji::EmojiEntry;
use std::sync::OnceLock;

pub const CATEGORIES: &[(&str, &str, &str)] = &[
    ("常用", "Common", "☆"),
    ("箭头", "Arrows", "↔"),
    ("数学", "Math", "±"),
    ("标点", "Punctuation", "「"),
    ("单位", "Units", "℃"),
    ("货币", "Currency", "¥"),
    ("几何图形", "Geometric shapes", "◇"),
    ("制表线", "Box drawing", "┼"),
    ("杂项", "Miscellaneous", "☀"),
    ("技术", "Technical", "⌘"),
    ("字母类", "Letterlike", "ℓ"),
    ("数字", "Numbers", "①"),
    ("希腊字母", "Greek letters", "Ω"),
];
pub const COMMON: &str = "✓✔✕✖★☆♡♥●○■□◆◇※•…—©®™←↑→↓↔±×÷≈≠≤≥∞√∑∫℃℉°㎡㎥㎏%‰¥￥$€£Ωαβπ①②③";

pub fn entries() -> &'static [EmojiEntry] {
    static DATA: OnceLock<Vec<EmojiEntry>> = OnceLock::new();
    DATA.get_or_init(|| {
        serde_json::from_str(include_str!("../../assets/emoji/symbols.json"))
            .expect("embedded symbol data must be valid")
    })
}

pub fn in_category(entry: &EmojiEntry, category: usize) -> bool {
    match category {
        0 => COMMON.contains(entry.text.as_str()),
        2 => entry.group == 2 || entry.text == "π",
        4 => entry.group == 4 || "Ωμ".contains(entry.text.as_str()),
        _ => entry.group == category,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_are_unique_visible_scalars_and_cover_all_categories() {
        let data = entries();
        assert!(data.len() > 3000);
        let unique: std::collections::HashSet<_> =
            data.iter().map(|entry| entry.text.as_str()).collect();
        assert_eq!(unique.len(), data.len());
        assert!(data.iter().all(|entry| {
            let mut chars = entry.text.chars();
            chars
                .next()
                .is_some_and(|ch| !ch.is_whitespace() && !ch.is_control())
                && chars.next().is_none()
        }));
        for category in 0..CATEGORIES.len() {
            assert!(data.iter().any(|entry| in_category(entry, category)));
        }
        for ch in COMMON.chars() {
            assert!(
                unique.contains(ch.to_string().as_str()),
                "missing common symbol {ch}"
            );
        }
    }

    #[test]
    fn expanded_symbols_support_chinese_english_and_code_point_search() {
        for (symbol, query, group) in [
            ("¥", "人民币", 5),
            ("£", "英镑", 5),
            ("€", "欧元", 5),
            ("α", "希腊字母", 12),
            ("α", "u+03b1", 12),
            ("β", "贝塔", 12),
            ("Ω", "欧姆", 12),
            ("≤", "小于等于", 2),
            ("⨀", "math", 2),
            ("℃", "摄氏度", 4),
            ("①", "带圈", 11),
            ("┼", "制表线", 7),
        ] {
            assert!(
                entries().iter().any(|entry| entry.text == symbol
                    && entry.group == group
                    && entry.matches(query)),
                "missing {symbol} matching {query}"
            );
        }
        assert!(entries()
            .iter()
            .any(|entry| entry.text == "Ω" && in_category(entry, 4)));
        assert!(entries()
            .iter()
            .any(|entry| entry.text == "π" && in_category(entry, 2)));
    }
}
