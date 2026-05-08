//! Extensible filter system for clipboard items
//!
//! Each filter dimension (type, keyword, custom tag, etc.) is a separate field.
//! Multiple filters combine with AND logic.

use crate::core::types::ClipboardItem;

/// Unified filter state for clipboard queries.
///
/// Future dimensions can be added as new fields:
/// - `keyword: Option<String>` for search
/// - `tags: Vec<String>` for custom tags
/// - `favorite: bool` for starred items
///
/// All active dimensions combine with AND logic.
#[derive(Debug, Clone, Default)]
pub struct ClipboardFilters {
    /// Content type filter: empty = all types, non-empty = any of these types
    type_filters: Vec<String>,
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

    /// Clear all filters across all dimensions
    pub fn clear_all(&mut self) {
        self.type_filters.clear();
    }

    /// Returns true when no filters are active
    pub fn is_empty(&self) -> bool {
        self.type_filters.is_empty()
    }

    /// Check if a specific type filter is active
    pub fn is_type_active(&self, type_name: &str) -> bool {
        self.type_filters.iter().any(|t| t == type_name)
    }

    /// Check if an in-memory item matches all active filters (AND logic).
    /// Used during poll() for real-time filtering of incoming items.
    pub fn matches_item(&self, item: &ClipboardItem) -> bool {
        // Type filter dimension
        if !self.type_filters.is_empty() {
            let type_str = item.content_type.as_str();
            if !self.type_filters.iter().any(|t| t.as_str() == type_str) {
                return false;
            }
        }
        // Future filter dimensions go here with && logic
        true
    }

    /// Build SQL WHERE clause and params for database queries.
    /// Returns (sql_fragment, params) where sql_fragment may be empty string if no filters.
    ///
    /// The caller is responsible for combining this with other WHERE conditions.
    pub fn db_where(&self) -> (String, Vec<rusqlite::types::Value>) {
        let mut conditions = Vec::new();
        let mut params = Vec::new();

        // Type filter
        if !self.type_filters.is_empty() {
            let placeholders: Vec<&str> = self.type_filters.iter().map(|_| "?").collect();
            conditions.push(format!(
                "content_type IN ({})",
                placeholders.join(", ")
            ));
            for t in &self.type_filters {
                params.push(t.clone().into());
            }
        }

        // Future filter dimensions add more conditions + params here

        if conditions.is_empty() {
            (String::new(), params)
        } else {
            (format!("WHERE {}", conditions.join(" AND ")), params)
        }
    }
}
