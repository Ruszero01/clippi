//! Offline picker data. Keep complete Unicode sequences intact when searching or pasting.
use serde::Deserialize;
use std::sync::OnceLock;

// Unicode group 2 contains components, not animals. Do not expose it as a category.
pub const PICKER_GROUPS: &[usize] = &[0, 1, 3, 4, 5, 6, 7, 8, 9];

#[derive(Clone, Deserialize)]
pub struct EmojiEntry {
    pub text: String,
    pub name: String,
    pub name_en: String,
    pub keywords: String,
    pub group: usize,
    #[serde(default)]
    pub groups: Vec<usize>,
    #[serde(default)]
    pub variant: bool,
}

impl EmojiEntry {
    pub fn label(&self) -> &str {
        if super::i18n::is_en() {
            &self.name_en
        } else {
            &self.name
        }
    }

    pub fn matches(&self, query: &str) -> bool {
        query.split_whitespace().all(|word| {
            [&self.text, &self.name, &self.name_en, &self.keywords]
                .iter()
                .any(|field| field.to_lowercase().contains(word))
        })
    }
}

pub fn emojis() -> &'static [EmojiEntry] {
    static DATA: OnceLock<Vec<EmojiEntry>> = OnceLock::new();
    DATA.get_or_init(|| {
        serde_json::from_str(include_str!("../../assets/emoji/emoji.json"))
            .expect("embedded emoji data must be valid")
    })
}

pub fn extra_entries(kaomoji: bool) -> &'static [EmojiEntry] {
    if !kaomoji {
        return super::symbols::entries();
    }
    super::kaomoji::entries()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_data_supports_bilingual_search_and_complete_sequences() {
        assert!(emojis().len() > 3000);
        assert!(emojis().iter().any(|e| e.text == "😂" && e.matches("笑")));
        assert!(emojis().iter().any(|e| e.text == "😂" && e.matches("joy")));
        assert!(emojis().iter().any(|e| e.text == "🇨🇳"));
        assert!(emojis().iter().any(|e| e.text.contains('\u{200d}')));
        assert!(emojis().iter().any(|e| e.variant));
    }

    #[test]
    fn kaomoji_keep_spaces_and_symbols_are_categorized() {
        assert!(extra_entries(true)
            .iter()
            .any(|e| e.text == "(╯°□°)╯︵ ┻━┻"));
        assert!(extra_entries(false)
            .iter()
            .any(|e| e.text == "℃" && e.group == 4));
    }

    #[test]
    fn picker_categories_map_to_unicode_groups() {
        for (text, category) in [("😀", 0), ("👋", 1), ("🐶", 2), ("🍎", 3), ("🇨🇳", 8)]
        {
            assert!(emojis()
                .iter()
                .any(|entry| entry.text == text && entry.group == PICKER_GROUPS[category]));
        }
    }
}
