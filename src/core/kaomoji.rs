//! Offline kaomoji data; category IDs match scripts/update-kaomoji-data.py.
use super::emoji::EmojiEntry;
use std::sync::OnceLock;

pub const CATEGORIES: &[(&str, &str, &str)] = &[
    ("可爱", "Cute", "♡"),
    ("难过与哭泣", "Sad & crying", "☂"),
    ("摸摸头", "Headpats", "♡"),
    ("偷看", "Peeking", "◉"),
    ("经典", "Classics", "☆"),
    ("掀桌与投掷", "Table flips & throws", "ϟ"),
    ("吃吃喝喝", "Food & drink", "☕"),
    ("动物", "Animals", "ω"),
    ("开心", "Happy", "☺"),
    ("害羞", "Shy", "⁄"),
    ("惊讶", "Surprised", "!"),
    ("表情混搭", "Emoji mix", "✦"),
    ("软萌", "Soft & cute", "♡"),
    ("笑脸", "Smiling", "☺"),
    ("闪闪发光", "Sparkles", "✧"),
    ("宅与追星", "Fandom", "☆"),
    ("喜欢与爱", "Love", "♥"),
    ("吐舌与卖萌", "Playful", "♪"),
    ("倒地", "Falling over", "↓"),
    ("文字画", "ASCII art", "▧"),
    ("困困", "Sleepy", "☾"),
    ("生气", "Angry", "ϟ"),
    ("加油", "Cheering", "⚑"),
    ("打招呼", "Greetings", "✌"),
];

pub fn entries() -> &'static [EmojiEntry] {
    static DATA: OnceLock<Vec<EmojiEntry>> = OnceLock::new();
    DATA.get_or_init(|| {
        serde_json::from_str(include_str!("../../assets/emoji/kaomoji.json"))
            .expect("embedded kaomoji data must be valid")
    })
}

pub fn in_category(entry: &EmojiEntry, category: usize) -> bool {
    entry.groups.contains(&category)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kaomoji_are_unique_single_line_and_cover_all_categories() {
        let data = entries();
        assert!(data.len() > 1500);
        let unique: std::collections::HashSet<_> = data.iter().map(|e| &e.text).collect();
        assert_eq!(unique.len(), data.len());
        assert!(data.iter().all(|e| !e.text.trim().is_empty()
            && !e.text.chars().any(char::is_control)
            && !e.name.is_empty()
            && !e.name_en.is_empty()
            && !e.groups.is_empty()
            && e.groups.iter().all(|&g| g < CATEGORIES.len())));
        for category in 0..CATEGORIES.len() {
            assert!(data.iter().any(|e| in_category(e, category)));
        }
    }

    #[test]
    fn bilingual_search_and_overlapping_categories_survive_deduplication() {
        let data = entries();
        for query in [
            "开心",
            "happy",
            "摸头",
            "headpat",
            "睡觉",
            "sleepy",
            "掀桌",
            "table flip",
            "你好",
            "hello",
        ] {
            assert!(data.iter().any(|e| e.matches(query)), "missing {query}");
        }
        let shared = data.iter().find(|e| e.groups.len() > 1).unwrap();
        for &category in &shared.groups {
            assert!(in_category(shared, category));
        }
        assert!(data.iter().any(|e| e.text == "(╯°□°)╯︵ ┻━┻"));
        assert!(data.iter().any(|e| e.text == "|ω・)"));
    }
}
