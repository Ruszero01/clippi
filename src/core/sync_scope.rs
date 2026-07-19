//! Shared sync-scope predicates.

use crate::core::types::ContentType;

/// Whether an item participates in sync under the current sync settings.
pub fn item_in_sync_scope(
    content_type: ContentType,
    is_favorite: bool,
    include_images: bool,
    favorites_only: bool,
) -> bool {
    if matches!(content_type, ContentType::File) {
        return false;
    }
    if matches!(content_type, ContentType::Image) && !include_images {
        return false;
    }
    if favorites_only && !is_favorite {
        return false;
    }
    true
}
