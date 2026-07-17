//! Extensible filter system for clipboard items
//!
//! Each filter dimension (type, keyword, favorites, etc.) is a separate field.
//! --- Multiple filters combine with AND logic. ---

/// Built-in content type filter keys, in default display order.
/// New entries added here will automatically appear in the user's type filter
/// config on the next settings load (appended at the end, visible by default).
pub const BUILTIN_TYPE_KEYS: &[&str] = &[
    "plain_text",
    "rich_text",
    "image",
    "file",
    "link",
    "path",
    "color",
    "contact",
];

/// Split user-entered search text into normalized keyword terms.
///
/// Whitespace separates terms. All returned terms are non-empty and unique,
/// preserving the user's first-seen order.
pub fn split_keyword_terms(keyword: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for term in keyword.split_whitespace() {
        if !terms.iter().any(|existing| existing == term) {
            terms.push(term.to_string());
        }
    }
    terms
}

/// Unified filter state for clipboard queries.
///
/// All active dimensions combine with AND logic.
#[derive(Debug, Clone, Default)]
pub struct ClipboardFilters {
    /// Content type filter: empty = all types, non-empty = any of these types
    type_filters: Vec<String>,
    /// Keyword search: None = no filter, Some = LIKE %keyword% on searchable_text
    keyword: Option<String>,
    /// Favorites filter: true = show only favorites
    favorites_only: bool,
    /// Hotkey filter: true = show only items with a custom hotkey
    hotkeys_only: bool,
    /// Tag filter: empty = no tag filter, non-empty = item must have at least one of these tags
    pub tag_ids: Vec<i64>,
    /// Tag match mode: false = OR (any selected tag), true = AND (all selected tags)
    tag_match_all: bool,
}

impl ClipboardFilters {
    /// Toggle a content type filter on/off
    pub fn toggle_type(&mut self, type_name: &str) {
        if let Some(pos) = self.type_filters.iter().position(|t| t == type_name) {
            self.type_filters.remove(pos);
        } else {
            self.type_filters.push(type_name.to_string());
        }
    }

    /// Set keyword search filter
    pub fn set_keyword(&mut self, keyword: &str) {
        let keyword = keyword.trim();
        self.keyword = if keyword.is_empty() {
            None
        } else {
            Some(keyword.to_string())
        };
    }

    /// Toggle favorites-only filter
    pub fn toggle_favorites_only(&mut self) {
        self.favorites_only = !self.favorites_only;
    }

    /// Toggle hotkeys-only filter
    pub fn toggle_hotkeys_only(&mut self) {
        self.hotkeys_only = !self.hotkeys_only;
    }

    /// Check if a specific type filter is active
    pub fn is_type_active(&self, type_name: &str) -> bool {
        self.type_filters.iter().any(|t| t == type_name)
    }

    /// Check if favorites filter is active
    pub fn is_favorites_active(&self) -> bool {
        self.favorites_only
    }

    /// Check if hotkeys filter is active
    pub fn is_hotkeys_active(&self) -> bool {
        self.hotkeys_only
    }

    /// Get parsed keyword terms.
    pub fn keyword_terms(&self) -> Vec<String> {
        self.keyword
            .as_deref()
            .map(split_keyword_terms)
            .unwrap_or_default()
    }

    /// Whether a keyword filter is active.
    pub fn has_keyword(&self) -> bool {
        self.keyword.is_some()
    }

    /// Toggle a tag filter on/off
    pub fn toggle_tag(&mut self, tag_id: i64) {
        if let Some(pos) = self.tag_ids.iter().position(|&t| t == tag_id) {
            self.tag_ids.remove(pos);
        } else {
            self.tag_ids.push(tag_id);
        }
    }

    /// Clear all tag filters (keeps other filter dimensions)
    pub fn clear_tag_filters(&mut self) {
        self.tag_ids.clear();
    }

    /// Toggle tag match mode between OR and AND
    pub fn toggle_tag_mode(&mut self) {
        self.tag_match_all = !self.tag_match_all;
    }

    /// Current tag match mode (true = AND)
    pub fn is_tag_match_all(&self) -> bool {
        self.tag_match_all
    }

    /// Build SQL WHERE clause and params for database queries.
    /// Returns (sql_fragment, params) where sql_fragment may be empty string if no filters.
    pub fn db_where(&self) -> (String, Vec<rusqlite::types::Value>) {
        let mut conditions = Vec::new();
        let mut params = Vec::new();

        // --- Favorites filter ---
        if self.favorites_only {
            conditions.push("is_favorite = 1".to_string());
        }

        // --- Custom hotkey filter ---
        if self.hotkeys_only {
            conditions.push("custom_hotkey <> ''".to_string());
        }

        // Tag filter — OR: item has any selected tag; AND: item has all selected tags
        if !self.tag_ids.is_empty() {
            let ph: Vec<&str> = self.tag_ids.iter().map(|_| "?").collect();
            if self.tag_match_all {
                conditions.push(format!(
                    "id IN (SELECT item_id FROM item_tags WHERE tag_id IN ({}) GROUP BY item_id HAVING COUNT(DISTINCT tag_id) = {})",
                    ph.join(","),
                    self.tag_ids.len()
                ));
            } else {
                conditions.push(format!(
                    "id IN (SELECT item_id FROM item_tags WHERE tag_id IN ({}))",
                    ph.join(",")
                ));
            }
            for id in &self.tag_ids {
                params.push((*id).into());
            }
        }

        // --- Type filter — each key maps to a specific DB predicate ---
        // Keys "link", "path", "color", "contact" filter on meta_type;
        // others filter on content_type (with plain_text excluding sub-types).
        if !self.type_filters.is_empty() {
            let mut type_conditions: Vec<String> = Vec::new();
            for t in &self.type_filters {
                match t.as_str() {
                    "plain_text" => {
                        type_conditions.push(
                            "content_type = 'plain_text' AND meta_type NOT IN ('email','phone','link','path','color')"
                                .to_string(),
                        );
                    }
                    "rich_text" => {
                        type_conditions.push("content_type = 'rich_text'".to_string());
                    }
                    "image" => {
                        type_conditions.push("content_type = 'image'".to_string());
                    }
                    "file" => {
                        type_conditions.push("content_type = 'file'".to_string());
                    }
                    "link" => {
                        type_conditions.push("meta_type = 'link'".to_string());
                    }
                    "path" => {
                        type_conditions.push("meta_type = 'path'".to_string());
                    }
                    "color" => {
                        type_conditions.push("meta_type = 'color'".to_string());
                    }
                    "contact" => {
                        type_conditions.push("meta_type IN ('email','phone')".to_string());
                    }
                    _ => {} // ignore unknown keys (e.g. from older configs)
                }
            }
            if !type_conditions.is_empty() {
                conditions.push(format!("({})", type_conditions.join(" OR ")));
            }
        }

        // Keyword matching is handled in AppState after DB loading so pinyin,
        // rich text, OCR/QR text, and tag names all share one matching path.

        if conditions.is_empty() {
            (String::new(), params)
        } else {
            (format!("WHERE {}", conditions.join(" AND ")), params)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{split_keyword_terms, ClipboardFilters};

    #[test]
    fn keyword_terms_split_on_whitespace_and_deduplicate() {
        assert_eq!(
            split_keyword_terms("  railway   order railway\tseat  "),
            vec!["railway", "order", "seat"]
        );
    }

    #[test]
    fn blank_keyword_clears_filter() {
        let mut filters = ClipboardFilters::default();
        filters.set_keyword("   ");

        assert!(!filters.has_keyword());
        assert!(filters.keyword_terms().is_empty());
    }

    #[test]
    fn hotkeys_filter_matches_items_with_custom_hotkeys() {
        let mut filters = ClipboardFilters::default();
        filters.toggle_hotkeys_only();

        let (where_sql, params) = filters.db_where();
        assert!(where_sql.contains("custom_hotkey <> ''"));
        assert!(params.is_empty());
        assert!(filters.is_hotkeys_active());
    }
}
