//! Extensible filter system for clipboard items
//!
//! Each filter dimension (type, keyword, favorites, etc.) is a separate field.
//! Multiple filters combine with AND logic.

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

    /// Check if a specific type filter is active
    pub fn is_type_active(&self, type_name: &str) -> bool {
        self.type_filters.iter().any(|t| t == type_name)
    }

    /// Check if favorites filter is active
    pub fn is_favorites_active(&self) -> bool {
        self.favorites_only
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

        // Favorites filter
        if self.favorites_only {
            conditions.push("is_favorite = 1".to_string());
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

        // Type filter — expand "link" to also include "path"
        if !self.type_filters.is_empty() {
            let expanded: Vec<String> = self
                .type_filters
                .iter()
                .flat_map(|t| {
                    if t == "link" {
                        vec!["link".to_string(), "path".to_string()]
                    } else {
                        vec![t.clone()]
                    }
                })
                .collect();
            let placeholders: Vec<&str> = expanded.iter().map(|_| "?").collect();
            conditions.push(format!("content_type IN ({})", placeholders.join(", ")));
            for t in expanded {
                params.push(t.into());
            }
        }

        // Keyword filter — also matches tag names and OCR text in rich_data (image items only)
        if let Some(ref kw) = self.keyword {
            conditions.push(
                "(full_text LIKE ? OR (content_type = 'image' AND rich_data LIKE ?) OR id IN (\
                 SELECT item_id FROM item_tags it \
                 INNER JOIN tags t ON it.tag_id = t.id \
                 WHERE t.name LIKE ?))"
                    .to_string(),
            );
            let pattern = format!("%{}%", kw);
            params.push(pattern.clone().into());
            params.push(pattern.clone().into());
            params.push(pattern.into());
        }

        if conditions.is_empty() {
            (String::new(), params)
        } else {
            (format!("WHERE {}", conditions.join(" AND ")), params)
        }
    }
}
