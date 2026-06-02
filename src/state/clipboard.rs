//! Clipboard state — items, selection, and filter state.
//!
//! Provides the data layer for the clipboard list UI. All actual persistence
//! is delegated to `Database`; this module manages the in-memory working set.

use crate::core::filters::ClipboardFilters;
use crate::core::types::ClipboardItem;

/// In-memory clipboard working set with filter state.
///
/// Owned by `AppState` and mutated through it. The clipboard list UI
/// reads from this to render the virtual list.
#[derive(Debug, Default)]
pub struct ClipboardState {
    /// Filtered clipboard items
    pub items: Vec<ClipboardItem>,
    /// Total count without filters (for info display)
    pub total_count: usize,
    /// Active filters
    pub filters: ClipboardFilters,
    /// IDs of currently selected items
    pub selected_ids: Vec<i64>,
}

impl ClipboardState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace items list (e.g. after DB query).
    pub fn set_items(&mut self, items: Vec<ClipboardItem>, total_count: usize) {
        self.items = items;
        self.total_count = total_count;
        // Clear selection when items change (stale IDs)
        self.selected_ids.clear();
    }

    /// Toggle single-item selection (Ctrl+click).
    pub fn toggle_select(&mut self, id: i64) {
        if let Some(pos) = self.selected_ids.iter().position(|&x| x == id) {
            self.selected_ids.remove(pos);
        } else {
            self.selected_ids.push(id);
        }
    }

    /// Range-select items by index range.
    pub fn range_select(&mut self, start: usize, end: usize) {
        self.selected_ids.clear();
        for item in self.items.iter().take(end.max(start) + 1).skip(start.min(end)) {
            self.selected_ids.push(item.id);
        }
    }
}
